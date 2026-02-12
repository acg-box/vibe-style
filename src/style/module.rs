use std::collections::{HashMap, HashSet};

use ast::{Item, ItemList, Module, Use};
use ra_ap_syntax::{
	AstNode,
	ast::{self, HasAttrs, HasModuleItem, HasName},
};

use super::shared::{self, Edit, FileContext, TopItem, TopKind, Violation, WORKSPACE_IMPORT_ROOTS};

#[derive(Clone)]
struct LeadingUseEntry {
	order: usize,
	start: usize,
	end: usize,
	origin: usize,
	is_super_glob: bool,
	is_super_specific: bool,
	block: String,
}

#[derive(Clone)]
struct ModuleReorderEntry {
	order: usize,
	line: usize,
	kind: TopKind,
	bucket: usize,
	is_pub: bool,
	is_async: bool,
	text: String,
}

#[derive(Default)]
struct ModuleReorderLines {
	mod001: Vec<usize>,
	mod002: Vec<usize>,
	mod003: Vec<usize>,
}

pub(crate) fn check_module_order(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	let items_for_order = ctx
		.top_items
		.iter()
		.filter(|item| !(item.kind == TopKind::Mod && is_cfg_test_attrs(&item.attrs)))
		.collect::<Vec<_>>();
	let (planned_reorder_edits, mod001_fixable_lines, mod002_fixable_lines, mod003_fixable_lines) =
		build_module_reorder_plans(ctx);

	if emit_edits {
		edits.extend(planned_reorder_edits);
	}

	let mut order_seen: Vec<usize> = Vec::new();

	for item in &items_for_order {
		let Some(order) = order_bucket(item.kind) else {
			continue;
		};

		if let Some(last) = order_seen.last().copied() {
			if order < last {
				shared::push_violation(
					violations,
					ctx,
					item.line,
					"RUST-STYLE-MOD-001",
					"Top-level module item order does not match rust.md order.",
					mod001_fixable_lines.contains(&item.line),
				);
			}
		}

		order_seen.push(order);
	}

	let mut non_pub_seen: HashMap<TopKind, bool> = HashMap::new();

	for item in &items_for_order {
		let seen_non_pub = non_pub_seen.get(&item.kind).copied().unwrap_or(false);

		if item.is_pub {
			if seen_non_pub {
				shared::push_violation(
					violations,
					ctx,
					item.line,
					"RUST-STYLE-MOD-002",
					"Place pub items before non-pub items within the same group.",
					mod002_fixable_lines.contains(&item.line),
				);
			}
		} else {
			non_pub_seen.insert(item.kind, true);
		}
	}

	let mut async_seen = HashMap::new();

	async_seen.insert(true, false);
	async_seen.insert(false, false);

	for item in &items_for_order {
		if item.kind != TopKind::Fn {
			continue;
		}

		let key = item.is_pub;

		if item.is_async {
			async_seen.insert(key, true);
		} else if async_seen.get(&key).copied().unwrap_or(false) {
			shared::push_violation(
				violations,
				ctx,
				item.line,
				"RUST-STYLE-MOD-003",
				"Place non-async functions before async functions at the same visibility.",
				mod003_fixable_lines.contains(&item.line),
			);
		}
	}

	let mut last_non_test_idx: Option<usize> = None;

	for (idx, item) in ctx.top_items.iter().enumerate() {
		if !(item.kind == TopKind::Mod && is_cfg_test_attrs(&item.attrs)) {
			last_non_test_idx = Some(idx);
		}
	}

	if let Some(last_non_test_idx) = last_non_test_idx {
		for (idx, item) in ctx.top_items.iter().enumerate() {
			if !(item.kind == TopKind::Mod && is_cfg_test_attrs(&item.attrs)) {
				continue;
			}
			if idx < last_non_test_idx {
				shared::push_violation(
					violations,
					ctx,
					item.line,
					"RUST-STYLE-MOD-001",
					"Place #[cfg(test)] modules after all non-test items.",
					false,
				);
			}
		}
	}

	check_top_level_const_group_spacing(ctx, violations, edits, emit_edits);
	check_top_level_visibility_batch_spacing(ctx, violations, edits, emit_edits);
	check_top_level_excess_blank_lines(ctx, violations, edits, emit_edits);
}

pub(crate) fn check_cfg_test_mod_tests_use_super(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	let local_module_roots = collect_local_module_roots(ctx);

	for module in cfg_test_tests_modules(ctx) {
		let Some(item_list) = module.item_list() else {
			continue;
		};

		if item_list_has_super_glob(&item_list) {
			handle_cfg_test_module_with_super_use(
				ctx,
				violations,
				edits,
				emit_edits,
				&item_list,
				&local_module_roots,
			);
		} else {
			handle_cfg_test_module_missing_super_use(
				ctx,
				violations,
				edits,
				emit_edits,
				&module,
				&item_list,
				&local_module_roots,
			);
		}
	}
}

fn leading_whitespace(line: &str) -> String {
	line.chars().take_while(|ch| ch.is_whitespace()).collect()
}

fn cfg_test_tests_modules(ctx: &FileContext) -> Vec<Module> {
	ctx.source_file
		.items()
		.filter_map(|item| match item {
			ast::Item::Module(module) => Some(module),
			_ => None,
		})
		.filter(|module| {
			module
				.attrs()
				.any(|attr| attr.syntax().text().to_string().replace(' ', "").contains("cfg(test)"))
		})
		.filter(|module| {
			module.name().map(|name| name.text().to_string()) == Some("tests".to_owned())
		})
		.collect()
}

fn item_list_has_super_glob(item_list: &ItemList) -> bool {
	item_list.items().any(|nested| match nested {
		ast::Item::Use(use_item) => use_item
			.use_tree()
			.map(|tree| tree.syntax().text().to_string().replace(' ', ""))
			.is_some_and(|path| path == "super::*"),
		_ => false,
	})
}

fn module_items_indent(ctx: &FileContext, items: &[Item]) -> String {
	items
		.first()
		.and_then(|nested_item| {
			let nested_line = shared::line_from_offset(
				&ctx.line_starts,
				usize::from(nested_item.syntax().text_range().start()),
			);

			ctx.lines
				.get(nested_line.saturating_sub(1))
				.map(|line_text| leading_whitespace(line_text))
		})
		.unwrap_or_else(|| "\t".to_owned())
}

fn handle_cfg_test_module_missing_super_use(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	module: &Module,
	item_list: &ItemList,
	local_module_roots: &HashSet<String>,
) {
	let line = shared::line_from_offset(
		&ctx.line_starts,
		usize::from(module.syntax().text_range().start()),
	);

	shared::push_violation(
		violations,
		ctx,
		line,
		"RUST-STYLE-MOD-007",
		"#[cfg(test)] mod tests should include `use super::*;` unless it is a keep-alive module.",
		true,
	);

	if !emit_edits {
		return;
	}

	let items = item_list.items().collect::<Vec<_>>();
	let indent = module_items_indent(ctx, &items);
	let super_block = format!("{indent}use super::*;");
	let mut leading_use_end = None;
	let mut leading_use_origin = None;
	let mut first_non_use_start = None;

	for nested in &items {
		match nested {
			ast::Item::Use(use_item) if first_non_use_start.is_none() => {
				leading_use_end = Some(usize::from(nested.syntax().text_range().end()));
				leading_use_origin = Some(use_item_origin(use_item, local_module_roots));
			},
			_ => {
				first_non_use_start = Some(usize::from(nested.syntax().text_range().start()));

				break;
			},
		}
	}

	if let Some(edit) = build_missing_super_insert_edit(
		ctx,
		item_list,
		&items,
		&indent,
		&super_block,
		leading_use_end,
		leading_use_origin,
		first_non_use_start,
	) {
		edits.push(edit);
	}
}

#[allow(clippy::too_many_arguments)]
fn build_missing_super_insert_edit(
	ctx: &FileContext,
	item_list: &ItemList,
	items: &[Item],
	indent: &str,
	super_block: &str,
	leading_use_end: Option<usize>,
	leading_use_origin: Option<usize>,
	first_non_use_start: Option<usize>,
) -> Option<Edit> {
	if let Some(gap_start) = leading_use_end {
		let gap_end = first_non_use_start.unwrap_or(gap_start);
		let before_sep = if leading_use_origin == Some(2) { "\n" } else { "\n\n" };
		let gap = ctx.text.get(gap_start..gap_end).unwrap_or_default();

		if gap.chars().all(char::is_whitespace) {
			let after_sep = if let Some(next_start) = first_non_use_start {
				let next_line = shared::line_from_offset(&ctx.line_starts, next_start);
				let next_indent = ctx
					.lines
					.get(next_line.saturating_sub(1))
					.map(|line_text| leading_whitespace(line_text))
					.unwrap_or_else(|| indent.to_owned());

				format!("\n\n{next_indent}")
			} else {
				"\n".to_owned()
			};

			return Some(Edit {
				start: gap_start,
				end: gap_end,
				replacement: format!("{before_sep}{super_block}{after_sep}"),
				rule: "RUST-STYLE-MOD-007",
			});
		}

		return Some(Edit {
			start: gap_start,
			end: gap_start,
			replacement: format!("{before_sep}{super_block}"),
			rule: "RUST-STYLE-MOD-007",
		});
	}

	let list_start = usize::from(item_list.syntax().text_range().start());
	let insert_pos = ctx.text[list_start..]
		.find('\n')
		.map(|offset| list_start + offset + 1)
		.unwrap_or(list_start + 1);
	let trailing_newlines = if items.is_empty() { "\n" } else { "\n\n" };

	Some(Edit {
		start: insert_pos,
		end: insert_pos,
		replacement: format!("{super_block}{trailing_newlines}"),
		rule: "RUST-STYLE-MOD-007",
	})
}

fn handle_cfg_test_module_with_super_use(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	item_list: &ItemList,
	local_module_roots: &HashSet<String>,
) {
	let items = item_list.items().collect::<Vec<_>>();
	let Some((leading_uses, first_non_use_start)) =
		collect_leading_test_use_entries(ctx, &items, local_module_roots)
	else {
		return;
	};

	if leading_uses.len() < 2 {
		return;
	}

	let has_super_glob = leading_uses.iter().any(|entry| entry.is_super_glob);
	let has_super_specific = leading_uses.iter().any(|entry| entry.is_super_specific);
	let out_of_order = leading_uses.windows(2).position(|pair| pair[1].origin < pair[0].origin);
	let (line, message) = if let Some(bad_pair_idx) = out_of_order {
		(
			shared::line_from_offset(&ctx.line_starts, leading_uses[bad_pair_idx + 1].start),
			"In #[cfg(test)] mod tests, order imports as std, third-party, self/workspace.",
		)
	} else if has_super_glob && has_super_specific {
		(
			leading_uses
				.iter()
				.find(|entry| entry.is_super_specific)
				.map(|entry| shared::line_from_offset(&ctx.line_starts, entry.start))
				.unwrap_or_else(|| {
					shared::line_from_offset(&ctx.line_starts, leading_uses[0].start)
				}),
			"In #[cfg(test)] mod tests, prefer `use super::*;` and remove specific super imports.",
		)
	} else {
		return;
	};

	shared::push_violation(violations, ctx, line, "RUST-STYLE-MOD-007", message, true);

	if !emit_edits {
		return;
	}

	if let Some(edit) =
		build_cfg_test_use_reorder_edit(ctx, &leading_uses, first_non_use_start, has_super_glob)
	{
		edits.push(edit);
	}
}

fn collect_leading_test_use_entries(
	ctx: &FileContext,
	items: &[Item],
	local_module_roots: &HashSet<String>,
) -> Option<(Vec<LeadingUseEntry>, Option<usize>)> {
	let indent = module_items_indent(ctx, items);
	let mut entries = Vec::new();
	let mut first_non_use_start = None;

	for nested in items {
		match nested {
			ast::Item::Use(use_item) if first_non_use_start.is_none() => {
				let start = usize::from(nested.syntax().text_range().start());
				let end = usize::from(nested.syntax().text_range().end());
				let raw_block = ctx.text.get(start..end)?;
				let trimmed_block = raw_block.trim_end_matches('\n');
				let block = if trimmed_block.chars().next().is_some_and(char::is_whitespace) {
					trimmed_block.to_owned()
				} else {
					format!("{indent}{trimmed_block}")
				};
				let compact_path = use_item
					.use_tree()
					.map(|tree| tree.syntax().text().to_string().replace(' ', ""))
					.unwrap_or_default();

				entries.push(LeadingUseEntry {
					order: entries.len(),
					start,
					end,
					origin: use_item_origin(use_item, local_module_roots),
					is_super_glob: compact_path == "super::*",
					is_super_specific: compact_path.starts_with("super::")
						&& compact_path != "super::*",
					block,
				});
			},
			_ => {
				first_non_use_start = Some(usize::from(nested.syntax().text_range().start()));

				break;
			},
		}
	}

	Some((entries, first_non_use_start))
}

fn build_cfg_test_use_reorder_edit(
	ctx: &FileContext,
	leading_uses: &[LeadingUseEntry],
	first_non_use_start: Option<usize>,
	drop_super_specific: bool,
) -> Option<Edit> {
	let mut ordered = Vec::new();
	let mut kept_super_glob = false;

	for entry in leading_uses {
		if drop_super_specific && entry.is_super_specific {
			continue;
		}
		if entry.is_super_glob {
			if kept_super_glob {
				continue;
			}

			kept_super_glob = true;
		}

		ordered.push(entry.clone());
	}

	if ordered.is_empty() {
		return None;
	}

	ordered.sort_by_key(|entry| (entry.origin, entry.order));

	let mut replacement = String::new();

	for (idx, entry) in ordered.iter().enumerate() {
		if idx > 0 {
			if ordered[idx - 1].origin == entry.origin {
				replacement.push('\n');
			} else {
				replacement.push_str("\n\n");
			}
		}

		replacement.push_str(&entry.block);
	}

	let edit_start = leading_uses
		.first()
		.and_then(|entry| {
			let line = shared::line_from_offset(&ctx.line_starts, entry.start);

			shared::offset_from_line(&ctx.line_starts, line)
		})
		.unwrap_or_default();
	let last_use_end = leading_uses.last().map(|entry| entry.end).unwrap_or(edit_start);
	let mut edit_end = last_use_end;

	if let Some(next_start) = first_non_use_start {
		let gap = ctx.text.get(last_use_end..next_start).unwrap_or_default();

		if gap.chars().all(char::is_whitespace) {
			replacement.push_str("\n\n");

			let next_line = shared::line_from_offset(&ctx.line_starts, next_start);

			edit_end = shared::offset_from_line(&ctx.line_starts, next_line).unwrap_or(next_start);
		}
	}

	Some(Edit { start: edit_start, end: edit_end, replacement, rule: "RUST-STYLE-MOD-007" })
}

fn order_bucket(kind: TopKind) -> Option<usize> {
	match kind {
		TopKind::Mod => Some(0),
		TopKind::Use => Some(1),
		TopKind::MacroRules => Some(2),
		TopKind::Type => Some(3),
		TopKind::Const => Some(4),
		TopKind::Static => Some(5),
		TopKind::Trait => Some(6),
		TopKind::Enum | TopKind::Struct | TopKind::Impl => Some(8),
		TopKind::Fn => Some(10),
		TopKind::Other => None,
	}
}

fn is_cfg_test_attrs(attrs: &[String]) -> bool {
	attrs.iter().any(|attr| attr.replace(' ', "").contains("#[cfg(test)]"))
}

fn use_origin(path: &str, local_module_roots: &HashSet<String>) -> usize {
	let root = path.trim_start_matches(':').split("::").next().unwrap_or_default();
	let normalized_root = root.strip_prefix("r#").unwrap_or(root);

	if matches!(root, "std" | "core" | "alloc") {
		0
	} else if matches!(root, "crate" | "self" | "super")
		|| local_module_roots.contains(normalized_root)
		|| WORKSPACE_IMPORT_ROOTS.contains(normalized_root)
		|| WORKSPACE_IMPORT_ROOTS.contains(&normalized_root.replace('-', "_"))
	{
		2
	} else {
		1
	}
}

fn use_item_origin(use_item: &Use, local_module_roots: &HashSet<String>) -> usize {
	let path = use_item.use_tree().map(|tree| tree.syntax().text().to_string()).unwrap_or_default();

	use_origin(&path.replace(' ', ""), local_module_roots)
}

fn collect_local_module_roots(ctx: &FileContext) -> HashSet<String> {
	ctx.top_items
		.iter()
		.filter(|item| item.kind == TopKind::Mod)
		.filter_map(|item| item.name.as_deref())
		.map(|name| name.strip_prefix("r#").unwrap_or(name).to_owned())
		.collect()
}

fn is_cfg_test_mod_item(item: &TopItem) -> bool {
	item.kind == TopKind::Mod && is_cfg_test_attrs(&item.attrs)
}

fn separator_blank_only(ctx: &FileContext, prev: &TopItem, next: &TopItem) -> bool {
	if prev.end_line >= next.start_line.saturating_sub(1) {
		return true;
	}

	ctx.lines[prev.end_line..next.start_line.saturating_sub(1)]
		.iter()
		.all(|line| line.trim().is_empty())
}

fn item_text_range(ctx: &FileContext, item: &TopItem) -> Option<(usize, usize)> {
	let start = shared::offset_from_line(&ctx.line_starts, item.start_line)?;
	let end =
		shared::offset_from_line(&ctx.line_starts, item.end_line + 1).unwrap_or(ctx.text.len());

	if end < start { None } else { Some((start, end)) }
}

fn is_reorderable_kind(kind: TopKind) -> bool {
	matches!(
		kind,
		TopKind::Use
			| TopKind::Type
			| TopKind::Const
			| TopKind::Static
			| TopKind::Trait
			| TopKind::Enum
			| TopKind::Struct
			| TopKind::Impl
			| TopKind::Fn
	)
}

fn is_const_like_kind(kind: TopKind) -> bool {
	matches!(kind, TopKind::Const | TopKind::Static)
}

fn is_compact_const_group_pair(
	prev_kind: TopKind,
	prev_is_pub: bool,
	next_kind: TopKind,
	next_is_pub: bool,
) -> bool {
	prev_kind == next_kind && prev_is_pub == next_is_pub && is_const_like_kind(prev_kind)
}

fn check_top_level_const_group_spacing(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	for pair in ctx.top_items.windows(2) {
		let prev = &pair[0];
		let next = &pair[1];

		if !is_compact_const_group_pair(prev.kind, prev.is_pub, next.kind, next.is_pub) {
			continue;
		}
		if prev.end_line >= next.start_line.saturating_sub(1) {
			continue;
		}

		let between_start = prev.end_line;
		let between_end = next.start_line.saturating_sub(1);
		let between = &ctx.lines[between_start..between_end];
		let blank_count = between.iter().filter(|line| line.trim().is_empty()).count();

		if blank_count == 0 {
			continue;
		}

		let can_autofix = between.iter().all(|line| line.trim().is_empty());

		shared::push_violation(
			violations,
			ctx,
			next.line,
			"RUST-STYLE-SPACE-003",
			"Do not insert blank lines within constant declaration groups.",
			can_autofix,
		);

		if emit_edits && can_autofix {
			let Some(start) = shared::offset_from_line(&ctx.line_starts, prev.end_line + 1) else {
				continue;
			};
			let Some(end) = shared::offset_from_line(&ctx.line_starts, next.start_line) else {
				continue;
			};

			edits.push(Edit {
				start,
				end,
				replacement: String::new(),
				rule: "RUST-STYLE-SPACE-003",
			});
		}
	}
}

fn check_top_level_visibility_batch_spacing(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	for pair in ctx.top_items.windows(2) {
		let prev = &pair[0];
		let next = &pair[1];

		if prev.kind != next.kind || prev.is_pub == next.is_pub {
			continue;
		}

		let between_start = prev.end_line;
		let between_end = next.start_line.saturating_sub(1);
		let (blank_count, has_non_blank) =
			if between_start < between_end && between_end <= ctx.lines.len() {
				let between = &ctx.lines[between_start..between_end];

				(
					between.iter().filter(|line| line.trim().is_empty()).count(),
					between.iter().any(|line| !line.trim().is_empty()),
				)
			} else {
				(0, false)
			};

		if blank_count == 1 && !has_non_blank {
			continue;
		}

		let can_autofix = !has_non_blank;

		shared::push_violation(
			violations,
			ctx,
			next.line,
			"RUST-STYLE-MOD-002",
			"Insert exactly one blank line between pub and non-pub batches within the same item kind.",
			can_autofix,
		);

		if emit_edits && can_autofix {
			let Some(start) = shared::offset_from_line(&ctx.line_starts, prev.end_line + 1) else {
				continue;
			};
			let Some(end) = shared::offset_from_line(&ctx.line_starts, next.start_line) else {
				continue;
			};

			edits.push(Edit {
				start,
				end,
				replacement: "\n".to_owned(),
				rule: "RUST-STYLE-MOD-002",
			});
		}
	}
}

fn check_top_level_excess_blank_lines(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	for pair in ctx.top_items.windows(2) {
		let prev = &pair[0];
		let next = &pair[1];

		if prev.end_line >= next.start_line.saturating_sub(1) {
			continue;
		}
		if is_compact_const_group_pair(prev.kind, prev.is_pub, next.kind, next.is_pub) {
			continue;
		}

		let between_start = prev.end_line;
		let between_end = next.start_line.saturating_sub(1);
		let between = &ctx.lines[between_start..between_end];

		if !between.iter().all(|line| line.trim().is_empty()) {
			continue;
		}

		let blank_count = between.iter().filter(|line| line.trim().is_empty()).count();

		if blank_count <= 1 {
			continue;
		}

		shared::push_violation(
			violations,
			ctx,
			next.line,
			"RUST-STYLE-SPACE-003",
			"Do not insert extra blank lines between top-level items.",
			true,
		);

		if emit_edits {
			let Some(start) = shared::offset_from_line(&ctx.line_starts, prev.end_line + 1) else {
				continue;
			};
			let Some(end) = shared::offset_from_line(&ctx.line_starts, next.start_line) else {
				continue;
			};

			edits.push(Edit {
				start,
				end,
				replacement: "\n".to_owned(),
				rule: "RUST-STYLE-SPACE-003",
			});
		}
	}
}

fn build_module_reorder_plans(
	ctx: &FileContext,
) -> (Vec<Edit>, HashSet<usize>, HashSet<usize>, HashSet<usize>) {
	let mut edits = Vec::new();
	let mut mod001_fixable_lines = HashSet::new();
	let mut mod002_fixable_lines = HashSet::new();
	let mut mod003_fixable_lines = HashSet::new();
	let mut idx = 0_usize;

	while idx < ctx.top_items.len() {
		let end = module_reorder_run_end(ctx, idx);
		let run = &ctx.top_items[idx..end];
		let Some((edit, run_lines)) = build_module_reorder_run_edit(ctx, run) else {
			idx = end;

			continue;
		};

		for line in run_lines.mod001 {
			mod001_fixable_lines.insert(line);
		}
		for line in run_lines.mod002 {
			mod002_fixable_lines.insert(line);
		}
		for line in run_lines.mod003 {
			mod003_fixable_lines.insert(line);
		}

		edits.push(edit);

		idx = end;
	}

	(edits, mod001_fixable_lines, mod002_fixable_lines, mod003_fixable_lines)
}

fn module_reorder_run_end(ctx: &FileContext, start: usize) -> usize {
	let Some(first) = ctx.top_items.get(start) else {
		return start;
	};

	if is_cfg_test_mod_item(first) || !is_reorderable_kind(first.kind) {
		return start + 1;
	}

	let mut end = start + 1;

	while end < ctx.top_items.len() {
		let previous = &ctx.top_items[end - 1];
		let candidate = &ctx.top_items[end];

		if is_cfg_test_mod_item(candidate) || !is_reorderable_kind(candidate.kind) {
			break;
		}
		if !separator_blank_only(ctx, previous, candidate) {
			break;
		}

		end += 1;
	}

	end
}

fn build_module_reorder_run_edit(
	ctx: &FileContext,
	run: &[TopItem],
) -> Option<(Edit, ModuleReorderLines)> {
	if run.len() < 2 || run.windows(2).any(|pair| !separator_blank_only(ctx, &pair[0], &pair[1])) {
		return None;
	}

	let entries = collect_module_reorder_entries(ctx, run)?;
	let run_lines = collect_module_reorder_lines(&entries);

	if run_lines.mod001.is_empty() && run_lines.mod002.is_empty() && run_lines.mod003.is_empty() {
		return None;
	}

	let ordered = sorted_module_reorder_entries(&entries);

	if entries.iter().map(|entry| entry.order).collect::<Vec<_>>()
		== ordered.iter().map(|entry| entry.order).collect::<Vec<_>>()
	{
		return None;
	}

	let (run_start, _) = item_text_range(ctx, &run[0])?;
	let run_end = item_text_range(ctx, &run[run.len() - 1])
		.map(|(_, end_offset)| end_offset)
		.unwrap_or(run_start);
	let original = ctx.text.get(run_start..run_end).unwrap_or_default();
	let replacement = build_module_reorder_replacement(original, &ordered);
	let rule = select_module_reorder_rule(&run_lines);

	Some((Edit { start: run_start, end: run_end, replacement, rule }, run_lines))
}

fn collect_module_reorder_entries(
	ctx: &FileContext,
	run: &[TopItem],
) -> Option<Vec<ModuleReorderEntry>> {
	let mut entries = Vec::with_capacity(run.len());

	for (offset, item) in run.iter().enumerate() {
		let (start, end_offset) = item_text_range(ctx, item)?;
		let slice = ctx.text.get(start..end_offset)?;

		entries.push(ModuleReorderEntry {
			order: offset,
			line: item.line,
			kind: item.kind,
			bucket: order_bucket(item.kind).unwrap_or(usize::MAX),
			is_pub: item.is_pub,
			is_async: item.is_async,
			text: slice.trim_end_matches('\n').to_owned(),
		});
	}

	Some(entries)
}

fn collect_module_reorder_lines(entries: &[ModuleReorderEntry]) -> ModuleReorderLines {
	let mut out = ModuleReorderLines::default();
	let mut last_bucket = None;

	for entry in entries {
		if let Some(last) = last_bucket {
			if entry.bucket < last {
				out.mod001.push(entry.line);
			}
		}

		last_bucket = Some(entry.bucket);
	}

	let mut seen_non_pub_by_kind = HashMap::new();

	for entry in entries {
		let seen_non_pub = seen_non_pub_by_kind.get(&entry.kind).copied().unwrap_or(false);

		if entry.is_pub {
			if seen_non_pub {
				out.mod002.push(entry.line);
			}
		} else {
			seen_non_pub_by_kind.insert(entry.kind, true);
		}
	}

	let mut seen_async_by_visibility = HashMap::new();

	seen_async_by_visibility.insert(true, false);
	seen_async_by_visibility.insert(false, false);

	for entry in entries {
		if entry.kind != TopKind::Fn {
			continue;
		}
		if entry.is_async {
			seen_async_by_visibility.insert(entry.is_pub, true);
		} else if seen_async_by_visibility.get(&entry.is_pub).copied().unwrap_or(false) {
			out.mod003.push(entry.line);
		}
	}

	out
}

fn sorted_module_reorder_entries(entries: &[ModuleReorderEntry]) -> Vec<ModuleReorderEntry> {
	let mut kind_order = HashMap::new();

	for entry in entries {
		if !kind_order.contains_key(&entry.kind) {
			let next_order = kind_order.len();

			kind_order.insert(entry.kind, next_order);
		}
	}

	let mut ordered = entries.to_owned();

	ordered.sort_by_key(|entry| {
		(
			entry.bucket,
			kind_order.get(&entry.kind).copied().unwrap_or(usize::MAX),
			if entry.is_pub { 0 } else { 1 },
			if entry.kind == TopKind::Fn && entry.is_async { 1 } else { 0 },
			entry.order,
		)
	});

	ordered
}

fn build_module_reorder_replacement(original: &str, ordered: &[ModuleReorderEntry]) -> String {
	let mut replacement = String::new();

	for (position, entry) in ordered.iter().enumerate() {
		if position > 0 {
			let prev = &ordered[position - 1];
			let is_compact_const_group =
				is_compact_const_group_pair(prev.kind, prev.is_pub, entry.kind, entry.is_pub);

			if is_compact_const_group {
				replacement.push('\n');
			} else {
				replacement.push_str("\n\n");
			}
		}

		replacement.push_str(&entry.text);
	}

	if original.ends_with('\n') {
		replacement.push('\n');
	}

	replacement
}

fn select_module_reorder_rule(lines: &ModuleReorderLines) -> &'static str {
	let has_mod001_violation = !lines.mod001.is_empty();
	let has_mod002_violation = !lines.mod002.is_empty();
	let has_mod003_violation = !lines.mod003.is_empty();

	if has_mod001_violation {
		"RUST-STYLE-MOD-001"
	} else if has_mod003_violation && !has_mod002_violation {
		"RUST-STYLE-MOD-003"
	} else {
		"RUST-STYLE-MOD-002"
	}
}
