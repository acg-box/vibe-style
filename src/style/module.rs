use std::collections::{HashMap, HashSet};

use crate::style::shared::{self, Edit, FileContext, TopItem, TopKind, Violation};

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
		if let Some(last) = last_bucket
			&& entry.bucket < last
		{
			out.mod001.push(entry.line);
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
