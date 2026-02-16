use std::collections::{HashMap, HashSet};

use ra_ap_syntax::{
	AstNode,
	ast::{self, HasAttrs, HasVisibility, Item, ItemList},
};
use regex::Regex;

use crate::style::shared::{self, Edit, FileContext, TopItem, TopKind, Violation};

#[derive(Clone)]
struct ModuleReorderEntry {
	order: usize,
	line: usize,
	kind: TopKind,
	bucket: usize,
	hoist_for_macro_scope: bool,
	is_pub: bool,
	visibility: String,
	is_async: bool,
	text: String,
}

#[derive(Clone)]
struct ScopeTopItem {
	kind: TopKind,
	line: usize,
	start_offset: usize,
	end_offset: usize,
	hoist_for_macro_scope: bool,
	is_pub: bool,
	visibility: String,
	is_async: bool,
	attrs: Vec<String>,
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
	let items_for_order = collect_non_cfg_test_top_items(ctx);
	let (planned_reorder_edits, mod001_fixable_lines, mod002_fixable_lines, mod003_fixable_lines) =
		build_module_reorder_plans(ctx);

	if emit_edits {
		edits.extend(planned_reorder_edits);
	}

	push_mod001_order_violations(ctx, violations, &items_for_order, &mod001_fixable_lines);
	push_mod002_visibility_violations(ctx, violations, &items_for_order, &mod002_fixable_lines);
	push_mod003_async_violations(ctx, violations, &items_for_order, &mod003_fixable_lines);
	push_cfg_test_module_placement_violations(ctx, violations);
	check_top_level_const_group_spacing(ctx, violations, edits, emit_edits);
	check_top_level_mod_group_spacing(ctx, violations, edits, emit_edits);
	check_top_level_visibility_batch_spacing(ctx, violations, edits, emit_edits);
	check_top_level_excess_blank_lines(ctx, violations, edits, emit_edits);
	check_nested_module_item_order(ctx, violations, edits, emit_edits);
}

fn collect_non_cfg_test_top_items(ctx: &FileContext) -> Vec<&TopItem> {
	ctx.top_items
		.iter()
		.filter(|item| !(item.kind == TopKind::Mod && is_cfg_test_attrs(&item.attrs)))
		.collect::<Vec<_>>()
}

fn push_mod001_order_violations(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	items_for_order: &[&TopItem],
	mod001_fixable_lines: &HashSet<usize>,
) {
	let mut order_seen: Vec<usize> = Vec::new();

	for item in items_for_order {
		let Some(order) = order_bucket(item.kind) else {
			continue;
		};

		if let Some(last) = order_seen.last().copied()
			&& order < last
		{
			shared::push_violation(
				violations,
				ctx,
				item.line,
				"RUST-STYLE-MOD-001",
				"Top-level module item order does not match rust.md order.",
				mod001_fixable_lines.contains(&item.line),
			);
		}

		order_seen.push(order);
	}
}

fn push_mod002_visibility_violations(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	items_for_order: &[&TopItem],
	mod002_fixable_lines: &HashSet<usize>,
) {
	let mut non_pub_seen: HashMap<TopKind, bool> = HashMap::new();
	let mut seen_visibility_batches: HashMap<TopKind, HashSet<String>> = HashMap::new();
	let mut active_visibility_batch: HashMap<TopKind, String> = HashMap::new();

	for item in items_for_order {
		let seen_non_pub = non_pub_seen.get(&item.kind).copied().unwrap_or(false);
		let mut has_mod002_violation = false;

		if item.is_pub && seen_non_pub {
			has_mod002_violation = true;
		}
		if !item.is_pub {
			non_pub_seen.insert(item.kind, true);
		}

		let previous_visibility = active_visibility_batch.get(&item.kind);

		if previous_visibility != Some(&item.visibility) {
			let seen_for_kind = seen_visibility_batches.entry(item.kind).or_default();

			if seen_for_kind.contains(&item.visibility) {
				has_mod002_violation = true;
			}

			seen_for_kind.insert(item.visibility.clone());
			active_visibility_batch.insert(item.kind, item.visibility.clone());
		}
		if has_mod002_violation {
			shared::push_violation(
				violations,
				ctx,
				item.line,
				"RUST-STYLE-MOD-002",
				"Place pub items before non-pub items within the same group.",
				mod002_fixable_lines.contains(&item.line),
			);
		}
	}
}

fn push_mod003_async_violations(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	items_for_order: &[&TopItem],
	mod003_fixable_lines: &HashSet<usize>,
) {
	let mut async_seen = HashMap::new();

	async_seen.insert(true, false);
	async_seen.insert(false, false);

	for item in items_for_order {
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
}

fn push_cfg_test_module_placement_violations(ctx: &FileContext, violations: &mut Vec<Violation>) {
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
		TopKind::Mod
			| TopKind::Use
			| TopKind::MacroRules
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

fn same_visibility_batch(prev_visibility: &str, next_visibility: &str) -> bool {
	prev_visibility == next_visibility
}

fn is_compact_const_group_pair(
	prev_kind: TopKind,
	prev_visibility: &str,
	next_kind: TopKind,
	next_visibility: &str,
) -> bool {
	prev_kind == next_kind
		&& same_visibility_batch(prev_visibility, next_visibility)
		&& is_const_like_kind(prev_kind)
}

fn is_compact_mod_group_pair(
	prev_kind: TopKind,
	prev_visibility: &str,
	next_kind: TopKind,
	next_visibility: &str,
) -> bool {
	prev_kind == TopKind::Mod
		&& next_kind == TopKind::Mod
		&& same_visibility_batch(prev_visibility, next_visibility)
}

fn is_compact_top_level_group_pair(
	prev_kind: TopKind,
	prev_visibility: &str,
	next_kind: TopKind,
	next_visibility: &str,
) -> bool {
	is_compact_const_group_pair(prev_kind, prev_visibility, next_kind, next_visibility)
		|| is_compact_mod_group_pair(prev_kind, prev_visibility, next_kind, next_visibility)
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

		if !is_compact_const_group_pair(prev.kind, &prev.visibility, next.kind, &next.visibility) {
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

fn check_top_level_mod_group_spacing(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	for pair in ctx.top_items.windows(2) {
		let prev = &pair[0];
		let next = &pair[1];

		if !is_compact_mod_group_pair(prev.kind, &prev.visibility, next.kind, &next.visibility) {
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
			"Do not insert blank lines within module declaration groups.",
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

		if prev.kind != next.kind || same_visibility_batch(&prev.visibility, &next.visibility) {
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
			"Insert exactly one blank line between visibility batches within the same item kind.",
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
		if is_compact_top_level_group_pair(prev.kind, &prev.visibility, next.kind, &next.visibility)
		{
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

fn check_nested_module_item_order(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	for module in ctx.source_file.syntax().descendants().filter_map(ast::Module::cast) {
		let Some(item_list) = module.item_list() else {
			continue;
		};
		let items = collect_scope_top_items(ctx, &item_list);

		if items.len() < 2 {
			continue;
		}

		check_scope_top_item_runs(ctx, &items, violations, edits, emit_edits);
	}
}

fn collect_scope_top_items(ctx: &FileContext, item_list: &ItemList) -> Vec<ScopeTopItem> {
	let mut items = Vec::new();

	for item in item_list.syntax().children().filter_map(Item::cast) {
		let text_range = item.syntax().text_range();
		let syntax_start = usize::from(text_range.start());
		let line = shared::line_from_offset(&ctx.line_starts, syntax_start);
		let start_offset = shared::offset_from_line(&ctx.line_starts, line).unwrap_or(syntax_start);
		let end_offset = usize::from(text_range.end());
		let visibility = scope_item_visibility_key(&item);
		let attrs = item.attrs().map(|attr| attr.syntax().text().to_string()).collect::<Vec<_>>();
		let hoist_for_macro_scope = scope_item_should_hoist_for_macro_scope(item_list, &item);

		items.push(ScopeTopItem {
			kind: scope_item_kind(&item),
			line,
			start_offset,
			end_offset,
			hoist_for_macro_scope,
			is_pub: !visibility.is_empty(),
			visibility,
			is_async: matches!(&item, Item::Fn(func) if func.async_token().is_some()),
			attrs,
		});
	}

	items
}

fn scope_item_kind(item: &Item) -> TopKind {
	match item {
		Item::Module(_) => TopKind::Mod,
		Item::Use(_) => TopKind::Use,
		Item::MacroRules(_) => TopKind::MacroRules,
		Item::MacroCall(_) => TopKind::MacroRules,
		Item::TypeAlias(_) => TopKind::Type,
		Item::Const(_) => TopKind::Const,
		Item::Static(_) => TopKind::Static,
		Item::Trait(_) => TopKind::Trait,
		Item::Enum(_) => TopKind::Enum,
		Item::Struct(_) => TopKind::Struct,
		Item::Impl(_) => TopKind::Impl,
		Item::Fn(_) => TopKind::Fn,
		_ => TopKind::Other,
	}
}

fn scope_item_should_hoist_for_macro_scope(item_list: &ItemList, item: &Item) -> bool {
	let Item::MacroRules(macro_rules) = item else {
		return false;
	};
	let Some(name) = macro_rules_name_text(macro_rules.syntax().text().to_string().as_str()) else {
		return false;
	};

	scope_has_macro_call(item_list, name.as_str())
}

fn macro_rules_name_text(text: &str) -> Option<String> {
	let re = Regex::new(r"^\s*macro_rules!\s*([A-Za-z_][A-Za-z0-9_]*)\b")
		.expect("Compile macro_rules name regex.");

	re.captures(text).and_then(|captures| captures.get(1).map(|name| name.as_str().to_owned()))
}

fn scope_has_macro_call(item_list: &ItemList, macro_name: &str) -> bool {
	for macro_call in item_list.syntax().descendants().filter_map(ast::MacroCall::cast) {
		let Some(path_text) = macro_call.path().map(|path| path.syntax().text().to_string()) else {
			continue;
		};
		let Some(last_segment) = path_text.rsplit("::").next() else {
			continue;
		};

		if !is_same_ident_local(last_segment.trim(), macro_name) {
			continue;
		}

		return true;
	}

	false
}

fn is_same_ident_local(lhs: &str, rhs: &str) -> bool {
	let lhs = lhs.strip_prefix("r#").unwrap_or(lhs);
	let rhs = rhs.strip_prefix("r#").unwrap_or(rhs);

	lhs == rhs
}

fn scope_item_visibility_key(item: &Item) -> String {
	scope_item_visibility_text(item)
		.map(|text| text.chars().filter(|ch| !ch.is_whitespace()).collect::<String>())
		.unwrap_or_default()
}

fn scope_item_visibility_text(item: &Item) -> Option<String> {
	match item {
		Item::Module(node) =>
			node.visibility().map(|visibility| visibility.syntax().text().to_string()),
		Item::Use(node) =>
			node.visibility().map(|visibility| visibility.syntax().text().to_string()),
		Item::TypeAlias(node) =>
			node.visibility().map(|visibility| visibility.syntax().text().to_string()),
		Item::Const(node) =>
			node.visibility().map(|visibility| visibility.syntax().text().to_string()),
		Item::Static(node) =>
			node.visibility().map(|visibility| visibility.syntax().text().to_string()),
		Item::Trait(node) =>
			node.visibility().map(|visibility| visibility.syntax().text().to_string()),
		Item::Enum(node) =>
			node.visibility().map(|visibility| visibility.syntax().text().to_string()),
		Item::Struct(node) =>
			node.visibility().map(|visibility| visibility.syntax().text().to_string()),
		Item::Fn(node) =>
			node.visibility().map(|visibility| visibility.syntax().text().to_string()),
		Item::Impl(node) =>
			node.visibility().map(|visibility| visibility.syntax().text().to_string()),
		_ => None,
	}
}

fn scope_separator_blank_only(ctx: &FileContext, prev: &ScopeTopItem, next: &ScopeTopItem) -> bool {
	if prev.end_offset >= next.start_offset {
		return true;
	}

	ctx.text
		.get(prev.end_offset..next.start_offset)
		.is_some_and(|between| between.trim().is_empty())
}

fn is_cfg_test_scope_mod_item(item: &ScopeTopItem) -> bool {
	item.kind == TopKind::Mod && is_cfg_test_attrs(&item.attrs)
}

fn check_scope_top_item_runs(
	ctx: &FileContext,
	items: &[ScopeTopItem],
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	let mut idx = 0_usize;

	while idx < items.len() {
		let end = scope_module_reorder_run_end(ctx, items, idx);
		let run = &items[idx..end];

		if let Some((edit, run_lines)) = build_scope_module_reorder_run_edit(ctx, run) {
			push_scope_reorder_violations(ctx, violations, &run_lines);

			if emit_edits {
				edits.push(edit);
			}
		}

		idx = end;
	}
}

fn scope_module_reorder_run_end(ctx: &FileContext, items: &[ScopeTopItem], start: usize) -> usize {
	let Some(first) = items.get(start) else {
		return start;
	};

	if is_cfg_test_scope_mod_item(first) || !is_reorderable_kind(first.kind) {
		return start + 1;
	}

	let mut end = start + 1;

	while end < items.len() {
		let previous = &items[end - 1];
		let candidate = &items[end];

		if is_cfg_test_scope_mod_item(candidate) || !is_reorderable_kind(candidate.kind) {
			break;
		}
		if !scope_separator_blank_only(ctx, previous, candidate) {
			break;
		}

		end += 1;
	}

	end
}

fn build_scope_module_reorder_run_edit(
	ctx: &FileContext,
	run: &[ScopeTopItem],
) -> Option<(Edit, ModuleReorderLines)> {
	if run.len() < 2
		|| run.windows(2).any(|pair| !scope_separator_blank_only(ctx, &pair[0], &pair[1]))
	{
		return None;
	}

	let entries = collect_scope_module_reorder_entries(ctx, run)?;
	let run_lines = collect_module_reorder_lines(&entries);
	let ordered = sorted_module_reorder_entries(&entries);
	let order_changed = entries.iter().map(|entry| entry.order).collect::<Vec<_>>()
		!= ordered.iter().map(|entry| entry.order).collect::<Vec<_>>();
	let run_start = run.first()?.start_offset;
	let run_end = run.last()?.end_offset;
	let original = ctx.text.get(run_start..run_end).unwrap_or_default();
	let replacement = build_module_reorder_replacement(original, &ordered);

	if !order_changed && replacement == original {
		return None;
	}

	let rule = if run_lines.mod001.is_empty()
		&& run_lines.mod002.is_empty()
		&& run_lines.mod003.is_empty()
	{
		"RUST-STYLE-SPACE-003"
	} else {
		select_module_reorder_rule(&run_lines)
	};

	Some((Edit { start: run_start, end: run_end, replacement, rule }, run_lines))
}

fn collect_scope_module_reorder_entries(
	ctx: &FileContext,
	run: &[ScopeTopItem],
) -> Option<Vec<ModuleReorderEntry>> {
	let mut entries = Vec::with_capacity(run.len());

	for (offset, item) in run.iter().enumerate() {
		let slice = ctx.text.get(item.start_offset..item.end_offset)?;

		entries.push(ModuleReorderEntry {
			order: offset,
			line: item.line,
			kind: item.kind,
			bucket: order_bucket(item.kind).unwrap_or(usize::MAX),
			hoist_for_macro_scope: item.hoist_for_macro_scope,
			is_pub: item.is_pub,
			visibility: item.visibility.clone(),
			is_async: item.is_async,
			text: slice.trim_end_matches('\n').to_owned(),
		});
	}

	Some(entries)
}

fn push_scope_reorder_violations(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	run_lines: &ModuleReorderLines,
) {
	for line in &run_lines.mod001 {
		shared::push_violation(
			violations,
			ctx,
			*line,
			"RUST-STYLE-MOD-001",
			"Top-level module item order does not match rust.md order.",
			true,
		);
	}
	for line in &run_lines.mod002 {
		shared::push_violation(
			violations,
			ctx,
			*line,
			"RUST-STYLE-MOD-002",
			"Place pub items before non-pub items within the same group.",
			true,
		);
	}
	for line in &run_lines.mod003 {
		shared::push_violation(
			violations,
			ctx,
			*line,
			"RUST-STYLE-MOD-003",
			"Place non-async functions before async functions at the same visibility.",
			true,
		);
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
			hoist_for_macro_scope: false,
			is_pub: item.is_pub,
			visibility: item.visibility.clone(),
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
		if let Some(last) = last_bucket
			&& entry.bucket < last
		{
			out.mod001.push(entry.line);
		}

		last_bucket = Some(entry.bucket);
	}
	for (idx, entry) in entries.iter().enumerate() {
		if entry.hoist_for_macro_scope && idx > 0 {
			out.mod001.push(entry.line);
		}
	}

	let mut seen_non_pub_by_kind = HashMap::new();
	let mut seen_visibility_batches_by_kind: HashMap<TopKind, HashSet<String>> = HashMap::new();
	let mut active_visibility_batch_by_kind: HashMap<TopKind, String> = HashMap::new();

	for entry in entries {
		let seen_non_pub = seen_non_pub_by_kind.get(&entry.kind).copied().unwrap_or(false);
		let mut has_mod002_violation = false;

		if entry.is_pub && seen_non_pub {
			has_mod002_violation = true;
		}
		if !entry.is_pub {
			seen_non_pub_by_kind.insert(entry.kind, true);
		}

		let previous_visibility = active_visibility_batch_by_kind.get(&entry.kind);

		if previous_visibility != Some(&entry.visibility) {
			let seen_for_kind = seen_visibility_batches_by_kind.entry(entry.kind).or_default();

			if seen_for_kind.contains(&entry.visibility) {
				has_mod002_violation = true;
			}

			seen_for_kind.insert(entry.visibility.clone());
			active_visibility_batch_by_kind.insert(entry.kind, entry.visibility.clone());
		}
		if has_mod002_violation {
			out.mod002.push(entry.line);
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
	let mut visibility_batch_order_by_kind: HashMap<(TopKind, String), usize> = HashMap::new();
	let mut next_visibility_order_by_kind: HashMap<TopKind, usize> = HashMap::new();

	for entry in entries {
		if !kind_order.contains_key(&entry.kind) {
			let next_order = kind_order.len();

			kind_order.insert(entry.kind, next_order);
		}

		let key = (entry.kind, entry.visibility.clone());

		visibility_batch_order_by_kind.entry(key).or_insert_with(|| {
			let next_order = next_visibility_order_by_kind.get(&entry.kind).copied().unwrap_or(0);

			next_visibility_order_by_kind.insert(entry.kind, next_order + 1);

			next_order
		});
	}

	let mut ordered = entries.to_owned();

	ordered.sort_by_key(|entry| {
		(
			if entry.hoist_for_macro_scope { 0 } else { 1 },
			entry.bucket,
			kind_order.get(&entry.kind).copied().unwrap_or(usize::MAX),
			if entry.is_pub { 0 } else { 1 },
			visibility_batch_order_by_kind
				.get(&(entry.kind, entry.visibility.clone()))
				.copied()
				.unwrap_or(usize::MAX),
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
			let is_compact_group = is_compact_top_level_group_pair(
				prev.kind,
				&prev.visibility,
				entry.kind,
				&entry.visibility,
			);

			if is_compact_group {
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
