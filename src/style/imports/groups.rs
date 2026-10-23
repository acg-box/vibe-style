use std::{
	collections::{BTreeMap, BTreeSet, HashMap, HashSet},
	iter, mem,
};

use crate::style::{
	imports::{ImportAnalysis, MixedUseGroup, UseEntry, paths, references, scopes},
	shared::{self, Edit, FileContext, TopItem, TopKind, Violation},
	syntax,
	workspace::WORKSPACE_IMPORT_ROOTS,
};

pub(super) fn apply_import002_normalization_rule(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	item: &TopItem,
	path: &str,
) -> bool {
	let Some(normalized) = normalize_mixed_self_child_use_path(ctx, path) else {
		return false;
	};

	shared::push_violation(
		violations,
		ctx,
		item.line,
		"RUST-STYLE-IMPORT-002",
		"Normalize imports like `use a::{b, b::c}` to `use a::{b::{self, c}}`.",
		true,
	);

	if !emit_edits {
		return true;
	}

	if let Some(edit) = paths::build_use_item_rewrite_edit(
		ctx,
		item,
		Some(normalized.as_str()),
		"RUST-STYLE-IMPORT-002",
	) {
		edits.push(edit);
	}

	true
}

pub(super) fn push_import_group_order_spacing_violations(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	use_runs: &[Vec<&TopItem>],
	fixable_import_group_lines: &HashSet<usize>,
	local_module_roots: &HashSet<String>,
) {
	for run in use_runs {
		for pair in run.windows(2) {
			let prev = pair[0];
			let curr = pair[1];
			let Some(prev_path) = paths::extract_use_path(ctx, prev) else {
				continue;
			};
			let Some(curr_path) = paths::extract_use_path(ctx, curr) else {
				continue;
			};
			let prev_origin = use_origin(&prev_path, local_module_roots);
			let curr_origin = use_origin(&curr_path, local_module_roots);
			let is_fixable = fixable_import_group_lines.contains(&curr.line);
			let between = separator_lines(ctx, prev, curr);
			let has_blank = between.iter().any(|line| line.trim().is_empty());
			let has_header_comment = between.iter().any(|line| line.trim_start().starts_with("//"));

			if curr_origin < prev_origin {
				shared::push_violation(
					violations,
					ctx,
					curr.line,
					"RUST-STYLE-IMPORT-001",
					"Import groups must be ordered: std, third-party, self/workspace.",
					is_fixable,
				);
			}
			if curr_origin != prev_origin && !has_blank {
				shared::push_violation(
					violations,
					ctx,
					curr.line,
					"RUST-STYLE-IMPORT-002",
					"Separate import groups with one blank line.",
					is_fixable,
				);
			}
			if curr_origin == prev_origin && has_blank {
				shared::push_violation(
					violations,
					ctx,
					curr.line,
					"RUST-STYLE-IMPORT-002",
					"Do not place blank lines inside an import group.",
					is_fixable,
				);
			}
			if has_header_comment {
				shared::push_violation(
					violations,
					ctx,
					curr.line,
					"RUST-STYLE-IMPORT-002",
					"Do not use header comments for import groups.",
					is_fixable,
				);
			}
		}
	}
}

pub(super) fn use_origin(path: &str, local_module_roots: &HashSet<String>) -> usize {
	let trimmed = path.replace("pub ", "");
	let root = trimmed
		.trim_start_matches(':')
		.split(|ch: char| ch == ':' || ch.is_whitespace())
		.next()
		.unwrap_or_default();
	let normalized_root = syntax::normalize_ident(root);

	if matches!(normalized_root, "std" | "core" | "alloc") {
		0
	} else if matches!(normalized_root, "crate" | "self" | "super")
		|| local_module_roots.contains(normalized_root)
		|| WORKSPACE_IMPORT_ROOTS.contains(normalized_root)
		|| WORKSPACE_IMPORT_ROOTS.contains(&normalized_root.replace('-', "_"))
	{
		2
	} else {
		1
	}
}

pub(super) fn collect_local_module_roots(ctx: &FileContext) -> HashSet<String> {
	ctx.top_items
		.iter()
		.filter(|item| item.kind == TopKind::Mod)
		.filter_map(|item| item.name.as_deref())
		.map(|name| syntax::normalize_ident(name).to_owned())
		.collect()
}

pub(super) fn normalize_mixed_self_child_use_path_preserve_self(
	ctx: &FileContext,
	path: &str,
) -> Option<String> {
	normalize_mixed_self_child_use_path_with_options(ctx, path, true)
}

pub(super) fn collect_non_pub_use_runs(ctx: &FileContext) -> Vec<Vec<&TopItem>> {
	let mut runs = Vec::new();
	let mut current = Vec::new();

	for item in &ctx.top_items {
		if item.kind == TopKind::Use && !item.is_pub {
			current.push(item);

			continue;
		}
		if !current.is_empty() {
			runs.push(mem::take(&mut current));
		}
	}

	if !current.is_empty() {
		runs.push(current);
	}

	runs
}

pub(super) fn collect_pub_use_runs(ctx: &FileContext) -> Vec<Vec<&TopItem>> {
	let mut runs = Vec::new();
	let mut current = Vec::new();

	for item in &ctx.top_items {
		if item.kind == TopKind::Use && item.is_pub {
			current.push(item);

			continue;
		}
		if !current.is_empty() {
			runs.push(mem::take(&mut current));
		}
	}

	if !current.is_empty() {
		runs.push(current);
	}

	runs
}

pub(super) fn apply_pub_use_group_rules(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	let local_module_roots = collect_local_module_roots(ctx);
	let pub_use_runs = collect_pub_use_runs(ctx);

	for run in pub_use_runs {
		let mut applied_self_group_edit = false;

		for segment in collect_pub_use_self_group_segments(&run) {
			if let Some((line, start, end, replacement)) =
				build_pub_use_self_group_edit(ctx, segment, &local_module_roots)
			{
				shared::push_violation(
					violations,
					ctx,
					line,
					"RUST-STYLE-IMPORT-002",
					"Prefer converging local module re-exports into `pub use self::{...};`.",
					true,
				);

				if emit_edits {
					edits.push(Edit { start, end, replacement, rule: "RUST-STYLE-IMPORT-002" });
				}

				applied_self_group_edit = true;
			}
		}

		if applied_self_group_edit {
			continue;
		}

		for pair in run.windows(2) {
			let prev = pair[0];
			let curr = pair[1];
			let between = separator_lines(ctx, prev, curr);

			if prev.visibility.trim() != curr.visibility.trim()
				|| between.is_empty()
				|| !between.iter().all(|line| line.trim().is_empty())
			{
				continue;
			}

			shared::push_violation(
				violations,
				ctx,
				curr.line,
				"RUST-STYLE-IMPORT-002",
				"Do not place blank lines inside a pub use group.",
				true,
			);

			if !emit_edits {
				continue;
			}

			let Some((prev_start, prev_end)) =
				shared::item_text_range(ctx, prev.start_offset, prev.end_offset)
			else {
				continue;
			};
			let Some((curr_start, _curr_end)) =
				shared::item_text_range(ctx, curr.start_offset, curr.end_offset)
			else {
				continue;
			};

			if curr_start <= prev_end || prev_start >= prev_end {
				continue;
			}

			// `item_text_range` ends at the start of the line following the item, so removing the
			// in-between range collapses multiple blank lines into a single newline separator.
			edits.push(Edit {
				start: prev_end,
				end: curr_start,
				replacement: String::new(),
				rule: "RUST-STYLE-IMPORT-002",
			});
		}
	}
}

pub(super) fn run_text_range(
	ctx: &FileContext,
	first: &TopItem,
	last: &TopItem,
) -> Option<(usize, usize)> {
	shared::item_text_range(ctx, first.start_offset, last.end_offset)
}

pub(super) fn build_import_group_fix_plans(
	ctx: &FileContext,
	analysis: &ImportAnalysis<'_>,
	skip_lines: &HashSet<usize>,
) -> (Vec<Edit>, HashSet<usize>) {
	let mut planned_edits = Vec::new();
	let mut fixable_lines = HashSet::new();

	for run in &analysis.use_runs {
		if !is_use_run_rewrite_candidate(run, skip_lines) {
			continue;
		}
		if !use_run_has_blank_only_separators(ctx, run) {
			continue;
		}

		let Some(entries) = collect_use_run_entries(ctx, run, analysis) else {
			continue;
		};
		let Some((run_start, run_end)) = run_text_range(ctx, run[0], run[run.len() - 1]) else {
			continue;
		};
		let Some(original) = ctx.text.get(run_start..run_end) else {
			continue;
		};
		let replacement = build_use_run_replacement(original, &entries);

		if replacement == original {
			continue;
		}

		for entry in entries {
			fixable_lines.insert(entry.item.line);
		}

		planned_edits.push(Edit {
			start: run_start,
			end: run_end,
			replacement,
			rule: "RUST-STYLE-IMPORT-002",
		});
	}

	(planned_edits, fixable_lines)
}

pub(super) fn import006_scope_insert_indent(
	ctx: &FileContext,
	insert_pos: usize,
) -> Option<String> {
	if let Some(first_item) = ctx.top_items.first()
		&& let Some(line_text) = ctx.lines.get(first_item.line.saturating_sub(1))
	{
		return Some(
			line_text.chars().take_while(|ch| matches!(ch, ' ' | '\t')).collect::<String>(),
		);
	}

	let line = shared::line_from_offset(&ctx.line_starts, insert_pos);
	let line_count = ctx.lines.len();
	let current_line = ctx.lines.get(line)?;

	if current_line.trim().is_empty() {
		return Some(
			current_line.chars().take_while(|ch| matches!(ch, ' ' | '\t')).collect::<String>(),
		);
	}

	for candidate_line in iter::once(line).chain((line + 1)..line_count).chain((0..line).rev()) {
		let Some(line_text) = ctx.lines.get(candidate_line) else {
			continue;
		};

		if line_text.trim().is_empty() {
			continue;
		}

		return Some(
			line_text.chars().take_while(|ch| matches!(ch, ' ' | '\t')).collect::<String>(),
		);
	}

	Some(String::new())
}

pub(super) fn import006_indent_insert_block(block: &str, indent: &str) -> String {
	let mut rewritten = String::with_capacity(block.len() + indent.len());

	for segment in block.split_inclusive('\n') {
		let line = segment.strip_suffix('\n').unwrap_or(segment);

		if !line.is_empty() {
			rewritten.push_str(indent);
			rewritten.push_str(line);
		}
		if segment.ends_with('\n') {
			rewritten.push('\n');
		}
	}

	rewritten
}

pub(super) fn build_import008_insert_edit(
	ctx: &FileContext,
	use_runs: &[Vec<&TopItem>],
	local_module_roots: &HashSet<String>,
	pending_import_paths: &BTreeSet<String>,
	rule: &'static str,
) -> Option<(Edit, HashSet<usize>)> {
	if pending_import_paths.is_empty() {
		return None;
	}

	let mut grouped: BTreeMap<usize, Vec<String>> = BTreeMap::new();

	for path in pending_import_paths {
		grouped.entry(use_origin(path, local_module_roots)).or_default().push(path.clone());
	}
	for paths in grouped.values_mut() {
		paths.sort();
		paths.dedup();
	}

	let mut block = String::new();

	for (group_idx, paths) in grouped.values().enumerate() {
		if group_idx > 0 {
			block.push_str("\n\n");
		}

		for (idx, path) in paths.iter().enumerate() {
			if idx > 0 {
				block.push('\n');
			}

			block.push_str("use ");
			block.push_str(path);
			block.push(';');
		}
	}

	if block.is_empty() {
		return None;
	}

	block.push('\n');

	if let Some(run) = use_runs.first() {
		let (Some(first), Some(last)) = (run.first(), run.last()) else {
			return None;
		};
		let (_, run_end) = run_text_range(ctx, first, last)?;
		let insert_pos =
			paths::item_syntax_text_range(ctx, last).map(|(_, end)| end).unwrap_or(run_end);
		let last_origin =
			paths::extract_use_path(ctx, last).map(|path| use_origin(&path, local_module_roots));
		let first_new_origin = grouped.keys().next().copied();
		let mut replacement = String::from("\n");

		if let (Some(last_origin), Some(first_new_origin)) = (last_origin, first_new_origin)
			&& last_origin != first_new_origin
		{
			replacement.push('\n');
		}

		replacement.push_str(&block);

		if let Some(indent) = import006_scope_insert_indent(ctx, insert_pos)
			&& !indent.is_empty()
		{
			replacement = import006_indent_insert_block(&replacement, &indent);
		}

		return Some((
			Edit { start: insert_pos, end: insert_pos, replacement, rule },
			run.iter().map(|item| item.line).collect::<HashSet<_>>(),
		));
	}

	let insert_line = import008_insert_line(ctx);
	let insert_pos =
		shared::offset_from_line(&ctx.line_starts, insert_line).unwrap_or(ctx.text.len());
	let mut replacement = block;

	if let Some(indent) = import006_scope_insert_indent(ctx, insert_pos)
		&& !indent.is_empty()
	{
		replacement = import006_indent_insert_block(&replacement, &indent);
	}

	if !ctx.top_items.is_empty() {
		replacement.push('\n');
	}

	Some((Edit { start: insert_pos, end: insert_pos, replacement, rule }, HashSet::new()))
}

pub(super) fn apply_pending_module_import_path_edits(
	ctx: &FileContext,
	edits: &mut Vec<Edit>,
	use_runs: &[Vec<&TopItem>],
	local_module_roots: &HashSet<String>,
	pending_import_paths: &BTreeSet<String>,
	imported_full_paths_by_symbol: &HashMap<String, HashSet<String>>,
	rule: &'static str,
) -> HashSet<usize> {
	if pending_import_paths.is_empty() {
		return HashSet::new();
	}

	let (pending_after_merge, merged_lines) = merge_import008_into_existing_module_use_items(
		ctx,
		edits,
		use_runs,
		pending_import_paths,
		imported_full_paths_by_symbol,
		rule,
	);
	let mut touched_lines = merged_lines;

	if let Some((edit, inserted_lines)) =
		build_import008_insert_edit(ctx, use_runs, local_module_roots, &pending_after_merge, rule)
	{
		edits.push(edit);
		touched_lines.extend(inserted_lines);
	}

	touched_lines
}

fn normalize_mixed_self_child_use_path(ctx: &FileContext, path: &str) -> Option<String> {
	normalize_mixed_self_child_use_path_with_options(ctx, path, false)
}

fn normalize_mixed_self_child_use_path_with_options(
	ctx: &FileContext,
	path: &str,
	force_keep_self: bool,
) -> Option<String> {
	let (prefix, close, segments) = paths::parse_braced_path_parts_allow_alias(path)?;
	let root = prefix
		.trim()
		.trim_start_matches("pub ")
		.trim()
		.trim_end_matches('{')
		.trim()
		.trim_end_matches("::")
		.trim()
		.split("::")
		.next()
		.unwrap_or_default();
	let allow_drop_unused_self = matches!(root, "crate" | "self" | "super");
	let (groups, parsed_heads) = build_mixed_use_groups(&segments);
	let rewritten = rewrite_mixed_use_segments(
		ctx,
		&segments,
		&groups,
		&parsed_heads,
		allow_drop_unused_self,
		force_keep_self,
	)?;
	let rewritten_path = format!("{prefix}{}{}", rewritten.join(", "), &path[close..=close]);

	if rewritten_path == path { None } else { Some(rewritten_path) }
}

fn build_mixed_use_groups(
	segments: &[String],
) -> (HashMap<String, MixedUseGroup>, Vec<Option<String>>) {
	let mut groups: HashMap<String, MixedUseGroup> = HashMap::new();
	let mut parsed_heads = Vec::with_capacity(segments.len());

	for (idx, segment) in segments.iter().enumerate() {
		let parsed_head = parse_mixed_use_segment_into_group(idx, segment, &mut groups);

		parsed_heads.push(parsed_head);
	}

	(groups, parsed_heads)
}

fn parse_mixed_use_segment_into_group(
	idx: usize,
	segment: &str,
	groups: &mut HashMap<String, MixedUseGroup>,
) -> Option<String> {
	if let Some((head, rest)) = segment.split_once("::") {
		let head = head.trim();

		if head.is_empty() || head.contains('{') || head.contains('}') {
			return None;
		}
		if rest.starts_with('{') && rest.ends_with('}') {
			return parse_mixed_nested_group(idx, head, rest, groups);
		}

		let child = rest.trim();

		if child.is_empty() || child.contains("::") {
			return None;
		}

		let group = groups.entry(head.to_owned()).or_default();

		group.indices.push(idx);
		group.children.push(child.to_owned());

		return Some(head.to_owned());
	}

	let head = segment.trim();

	if head.is_empty() || head.contains('{') || head.contains('}') {
		return None;
	}

	let group = groups.entry(head.to_owned()).or_default();

	group.indices.push(idx);

	group.has_self = true;

	Some(head.to_owned())
}

fn parse_mixed_nested_group(
	idx: usize,
	head: &str,
	rest: &str,
	groups: &mut HashMap<String, MixedUseGroup>,
) -> Option<String> {
	let inner = &rest[1..rest.len().saturating_sub(1)];
	let child_parts = paths::split_top_level_csv(inner);
	let group = groups.entry(head.to_owned()).or_default();

	group.indices.push(idx);

	for child in child_parts {
		if child == "self" {
			group.has_self = true;
		} else {
			group.children.push(child);
		}
	}

	Some(head.to_owned())
}

fn rewrite_mixed_use_segments(
	ctx: &FileContext,
	segments: &[String],
	groups: &HashMap<String, MixedUseGroup>,
	parsed_heads: &[Option<String>],
	allow_drop_unused_self: bool,
	force_keep_self: bool,
) -> Option<Vec<String>> {
	let mut emit = vec![true; segments.len()];
	let mut merged = false;
	let mut rewritten = Vec::new();

	for (idx, segment) in segments.iter().enumerate() {
		if !emit[idx] {
			continue;
		}

		let Some(head) = parsed_heads.get(idx).cloned().flatten() else {
			rewritten.push(segment.to_owned());

			continue;
		};
		let Some(group) = groups.get(&head) else {
			rewritten.push(segment.to_owned());

			continue;
		};
		let keep_self = force_keep_self
			|| references::symbol_is_referenced_outside_use(ctx, &head)
			|| scopes::symbol_referenced_by_child_module(ctx, &head);

		if !can_merge_mixed_group(group, idx) {
			if allow_drop_unused_self
				&& group.indices.first().copied() == Some(idx)
				&& group.has_self
				&& !keep_self
				&& let Some(rewritten_segment) =
					drop_unused_self_from_nested_use_segment(segment, &head)
			{
				rewritten.push(rewritten_segment);

				merged = true;

				continue;
			}

			rewritten.push(segment.to_owned());

			continue;
		}

		let children = dedup_mixed_group_children(group);
		let combined = if keep_self {
			format!("{head}::{{self, {}}}", children.join(", "))
		} else {
			format!("{head}::{{{}}}", children.join(", "))
		};

		rewritten.push(combined);

		merged = true;

		for original_idx in group.indices.iter().skip(1) {
			emit[*original_idx] = false;
		}
	}

	if merged { Some(rewritten) } else { None }
}

fn can_merge_mixed_group(group: &MixedUseGroup, idx: usize) -> bool {
	group.has_self
		&& !group.children.is_empty()
		&& group.indices.len() > 1
		&& group.indices.first().copied() == Some(idx)
}

fn dedup_mixed_group_children(group: &MixedUseGroup) -> Vec<String> {
	let mut seen = HashSet::new();
	let mut children = Vec::new();

	for child in &group.children {
		if seen.insert(child.clone()) {
			children.push(child.clone());
		}
	}

	children
}

fn drop_unused_self_from_nested_use_segment(segment: &str, head: &str) -> Option<String> {
	let trimmed = segment.trim();
	let (nested_head, inner) = paths::parse_single_level_nested_use_segment(trimmed)?;

	if !syntax::is_same_ident(nested_head, head) {
		return None;
	}

	let mut children = paths::split_top_level_csv(inner);
	let original_len = children.len();

	children.retain(|child| child.trim() != "self");

	if children.len() == original_len {
		return None;
	}
	if children.is_empty() {
		Some(head.to_owned())
	} else {
		Some(format!("{head}::{{{}}}", children.join(", ")))
	}
}

fn collect_pub_use_self_group_segments<'a>(run: &'a [&'a TopItem]) -> Vec<&'a [&'a TopItem]> {
	let mut segments = Vec::new();
	let mut start = 0_usize;

	while start < run.len() {
		let visibility = run[start].visibility.trim();

		if visibility.is_empty() {
			start += 1;

			continue;
		}

		let mut end = start + 1;

		while end < run.len() {
			let current = run[end];

			if current.visibility.trim() != visibility || current.attrs != run[start].attrs {
				break;
			}

			end += 1;
		}

		if end.saturating_sub(start) >= 2 {
			segments.push(&run[start..end]);
		}

		start = end;
	}

	segments
}

fn build_pub_use_self_group_edit(
	ctx: &FileContext,
	run: &[&TopItem],
	local_module_roots: &HashSet<String>,
) -> Option<(usize, usize, usize, String)> {
	if run.len() < 2 {
		return None;
	}
	if !run.windows(2).all(|pair| {
		separator_lines(ctx, pair[0], pair[1]).iter().all(|line| line.trim().is_empty())
	}) {
		return None;
	}

	let first_visibility = run[0].visibility.trim();
	let first_attrs = &run[0].attrs;

	if first_visibility.is_empty() {
		return None;
	}
	if run
		.iter()
		.any(|item| item.visibility.trim() != first_visibility || item.attrs != *first_attrs)
	{
		return None;
	}

	let mut grouped_paths = Vec::with_capacity(run.len());

	for item in run {
		let path = paths::extract_use_path(ctx, item)?;
		let root =
			path.trim_start_matches(':').split("::").next().map(str::trim).unwrap_or_default();
		let normalized_root = syntax::normalize_ident(root);

		if matches!(normalized_root, "crate" | "self" | "super")
			|| !local_module_roots.contains(normalized_root)
		{
			return None;
		}

		grouped_paths.push(path);
	}

	let (start, end) = run_text_range(ctx, run[0], run[run.len() - 1])?;
	let original = ctx.text.get(start..end)?;
	let attrs_prefix = if first_attrs.is_empty() {
		String::new()
	} else {
		let attrs = first_attrs
			.iter()
			.map(|attr| attr.trim())
			.filter(|attr| !attr.is_empty())
			.collect::<Vec<_>>();

		if attrs.is_empty() { String::new() } else { format!("{} ", attrs.join(" ")) }
	};
	let mut replacement =
		format!("{attrs_prefix}{first_visibility} use self::{{{}}};", grouped_paths.join(", "));

	if original.ends_with('\n') {
		replacement.push('\n');
	}
	if replacement == original {
		return None;
	}

	Some((run[0].line, start, end, replacement))
}

fn separator_lines<'a>(ctx: &'a FileContext, prev: &TopItem, curr: &TopItem) -> &'a [String] {
	let start = prev.end_line;
	let end = curr.start_line.saturating_sub(1);

	if start >= end || end > ctx.lines.len() { &[] } else { &ctx.lines[start..end] }
}

fn is_use_run_rewrite_candidate(run: &[&TopItem], skip_lines: &HashSet<usize>) -> bool {
	run.len() >= 2 && !run.iter().any(|item| skip_lines.contains(&item.line))
}

fn use_run_has_blank_only_separators(ctx: &FileContext, run: &[&TopItem]) -> bool {
	run.windows(2).all(|pair| {
		separator_lines(ctx, pair[0], pair[1]).iter().all(|line| line.trim().is_empty())
	})
}

fn collect_use_run_entries<'a>(
	ctx: &'a FileContext,
	run: &'a [&'a TopItem],
	analysis: &'a ImportAnalysis<'a>,
) -> Option<Vec<UseEntry<'a>>> {
	let mut entries = Vec::with_capacity(run.len());

	for (order, item) in run.iter().enumerate() {
		let use_item_analysis = analysis.use_item_analysis(item)?;
		let (start, end) = shared::item_text_range(ctx, item.start_offset, item.end_offset)?;
		let block = ctx.text.get(start..end)?;
		let normalized_block = normalize_use_item_block(block)?;

		entries.push(UseEntry {
			item,
			origin: use_origin(&use_item_analysis.path, &analysis.local_module_roots),
			order,
			block: normalized_block,
		});
	}

	Some(entries)
}

fn normalize_use_item_block(block: &str) -> Option<String> {
	let mut block_lines = Vec::new();
	let mut has_item_start = false;

	for line in block.lines() {
		let trimmed = line.trim();

		if !has_item_start {
			if trimmed.is_empty() {
				continue;
			}
			if line.trim_start().starts_with("//") {
				return None;
			}

			has_item_start = true;
		}

		block_lines.push(line);
	}

	if block_lines.is_empty() {
		return None;
	}

	let mut normalized_block = block_lines.join("\n");

	if block.ends_with('\n') {
		normalized_block.push('\n');
	}

	Some(normalized_block)
}

fn build_use_run_replacement(original: &str, entries: &[UseEntry<'_>]) -> String {
	let mut ordered_entries = entries.iter().collect::<Vec<_>>();

	ordered_entries.sort_by_key(|entry| (entry.origin, entry.order));

	let mut replacement = String::new();

	for (idx, entry) in ordered_entries.iter().enumerate() {
		if idx > 0 {
			let prev_origin = ordered_entries[idx - 1].origin;

			if entry.origin == prev_origin {
				replacement.push('\n');
			} else {
				replacement.push_str("\n\n");
			}
		}

		replacement.push_str(entry.block.trim_end_matches('\n'));
	}

	if original.ends_with('\n') {
		replacement.push('\n');
	}

	replacement
}

fn merge_import008_into_existing_module_use_items(
	ctx: &FileContext,
	edits: &mut Vec<Edit>,
	use_runs: &[Vec<&TopItem>],
	pending_import_paths: &BTreeSet<String>,
	imported_full_paths_by_symbol: &HashMap<String, HashSet<String>>,
	rule: &'static str,
) -> (BTreeSet<String>, HashSet<usize>) {
	if pending_import_paths.is_empty() {
		return (BTreeSet::new(), HashSet::new());
	}

	#[derive(Clone)]
	struct MergeTarget {
		anchor_line: usize,
		root_full: String,
	}

	let mut remaining = BTreeSet::new();
	let mut plans: BTreeMap<usize, BTreeMap<String, BTreeSet<String>>> = BTreeMap::new();
	let mut touched_lines = HashSet::new();
	let mut use_items = HashMap::new();
	let mut merge_targets_by_pending: HashMap<String, MergeTarget> = HashMap::new();
	let mut successful_merge_roots: HashSet<(usize, String)> = HashSet::new();

	for item in use_runs.iter().flat_map(|run| run.iter().copied()) {
		use_items.insert(item.line, item);
	}
	for pending in pending_import_paths {
		let Some((root_full, child_tail)) =
			resolve_import008_merge_target_for_pending_path(pending, imported_full_paths_by_symbol)
		else {
			remaining.insert(pending.clone());

			continue;
		};
		let Some(anchor_line) = find_use_item_line_importing_full_path(ctx, use_runs, root_full)
		else {
			remaining.insert(pending.clone());

			continue;
		};

		plans
			.entry(anchor_line)
			.or_default()
			.entry(root_full.to_owned())
			.or_default()
			.insert(child_tail.to_owned());
		merge_targets_by_pending
			.insert(pending.clone(), MergeTarget { anchor_line, root_full: root_full.to_owned() });
	}
	for (line, root_plans) in plans {
		let Some(item) = use_items.get(&line).copied() else {
			continue;
		};
		let Some((start, end)) = paths::item_syntax_text_range(ctx, item) else {
			continue;
		};
		let Some(raw) = ctx.text.get(start..end) else {
			continue;
		};
		let Some(use_path) = paths::extract_use_path_from_text(raw) else {
			continue;
		};
		let mut merged_path = use_path;
		let mut merged_any = false;

		for (root_full, children) in root_plans {
			let Some(next_path) =
				merge_children_into_use_path_for_root(&merged_path, &root_full, &children)
			else {
				continue;
			};

			successful_merge_roots.insert((line, root_full));

			merged_any |= next_path != merged_path;
			merged_path = next_path;
		}

		if !merged_any {
			continue;
		}

		if let Some(rewritten) = paths::rewrite_use_item_with_path(raw, &merged_path)
			&& rewritten != raw
		{
			edits.push(Edit { start, end, replacement: rewritten, rule });
			touched_lines.insert(line);
		}
	}
	for pending in pending_import_paths {
		let Some(target) = merge_targets_by_pending.get(pending) else {
			remaining.insert(pending.clone());

			continue;
		};
		let can_consume =
			successful_merge_roots.contains(&(target.anchor_line, target.root_full.clone()));

		if !can_consume {
			remaining.insert(pending.clone());
		}
	}

	(remaining, touched_lines)
}

fn resolve_import008_merge_target_for_pending_path<'a>(
	pending: &'a str,
	imported_full_paths_by_symbol: &'a HashMap<String, HashSet<String>>,
) -> Option<(&'a str, &'a str)> {
	let pending = pending.trim();

	if pending.is_empty() {
		return None;
	}

	if let Some((root, child_tail)) = pending.split_once("::")
		&& let Some(root_full_paths) = imported_full_paths_by_symbol.get(root)
		&& root_full_paths.len() == 1
		&& let Some(root_full) = root_full_paths.iter().next()
	{
		let child_tail = child_tail.trim();

		if !child_tail.is_empty() {
			return Some((root_full.as_str(), child_tail));
		}
	}
	if let Some((parent, module_name)) = pending.rsplit_once("::") {
		let module_name = module_name.trim();
		let module_root = parent.trim();

		if syntax::is_same_ident(
			module_root.rsplit("::").next().unwrap_or(module_root).trim(),
			module_name,
		) {
			let module_compact = paths::compact_path_for_match(module_root);

			if imported_full_paths_by_symbol.values().any(|full_paths| {
				full_paths.iter().any(|full_path| {
					full_path.rsplit_once("::").is_some_and(|(parent_full, _)| {
						paths::compact_path_for_match(parent_full) == module_compact
					})
				})
			}) {
				return Some((module_root, "self"));
			}
		}
	}

	let pending_compact = paths::compact_path_for_match(pending);

	if imported_full_paths_by_symbol.values().any(|full_paths| {
		full_paths.iter().any(|full_path| {
			full_path.rsplit_once("::").is_some_and(|(parent_full, _)| {
				paths::compact_path_for_match(parent_full) == pending_compact
			})
		})
	}) {
		return Some((pending, "self"));
	}

	let mut best: Option<(&str, &str)> = None;

	for root_full in imported_full_paths_by_symbol.values().filter_map(|paths| {
		if paths.len() == 1 { paths.iter().next().map(String::as_str) } else { None }
	}) {
		let prefix = format!("{root_full}::");

		if !pending.starts_with(&prefix) {
			continue;
		}

		let child_tail = pending.strip_prefix(&prefix)?.trim();

		if child_tail.is_empty() {
			continue;
		}

		match best {
			Some((current_root, _)) if current_root.len() >= root_full.len() => {},
			_ => best = Some((root_full, child_tail)),
		}
	}

	best
}

fn find_use_item_line_importing_full_path(
	ctx: &FileContext,
	use_runs: &[Vec<&TopItem>],
	target_path: &str,
) -> Option<usize> {
	let target_compact = paths::compact_path_for_match(target_path);
	let mut containing_line = None;

	for item in use_runs.iter().flat_map(|run| run.iter().copied()) {
		let Some(path) = paths::extract_use_path(ctx, item) else {
			continue;
		};
		let compact_path = paths::compact_path_for_match(&path);

		if let Some((prefix, _, _)) = paths::parse_braced_path_parts_allow_alias(&path) {
			let prefix_root = paths::compact_path_for_match(prefix.trim_end_matches('{'));
			let prefix_root = prefix_root.strip_suffix("::").unwrap_or(&prefix_root);

			if prefix_root == target_compact {
				containing_line.get_or_insert(item.line);
			}
		}

		if compact_path == target_compact {
			return Some(item.line);
		}
		if paths::imported_full_paths_from_use_path(&path).into_iter().any(|full_path| {
			let full_path_compact = paths::compact_path_for_match(&full_path);

			full_path_compact == target_compact
				|| full_path.rsplit_once("::").is_some_and(|(parent_full, _)| {
					paths::compact_path_for_match(parent_full) == target_compact
				})
		}) {
			containing_line.get_or_insert(item.line);
		}
	}

	containing_line
}

fn merge_children_into_use_path_for_root(
	use_path: &str,
	root_full: &str,
	children: &BTreeSet<String>,
) -> Option<String> {
	let mut current = use_path.to_owned();
	let mut changed = false;

	for child in children {
		let next = merge_single_child_into_use_path_for_root(&current, root_full, child)?;

		if next != current {
			changed = true;
			current = next;
		}
	}

	if changed { Some(current) } else { None }
}

fn merge_single_child_into_use_path_for_root(
	use_path: &str,
	root_full: &str,
	child_tail: &str,
) -> Option<String> {
	let root_compact = paths::compact_path_for_match(root_full);

	if paths::compact_path_for_match(use_path) == root_compact {
		return Some(format!("{root_full}::{{self, {child_tail}}}"));
	}

	if let Some(merged) =
		try_merge_child_into_simple_root_use_path(use_path, &root_compact, child_tail)
	{
		return Some(merged);
	}
	if let Some(merged) =
		try_merge_child_into_direct_root_braced_use_path(use_path, &root_compact, child_tail)
	{
		return Some(merged);
	}

	try_merge_child_into_parent_braced_use_path(use_path, root_full, child_tail)
}

fn try_merge_child_into_simple_root_use_path(
	use_path: &str,
	root_compact: &str,
	child_tail: &str,
) -> Option<String> {
	let (prefix, imported_symbol) = paths::simple_import_prefix_symbol(use_path)?;

	if paths::compact_path_for_match(&prefix) != *root_compact {
		return None;
	}
	if syntax::is_same_ident(&imported_symbol, child_tail) {
		return Some(use_path.to_owned());
	}

	Some(if child_tail == "self" {
		format!("{prefix}::{{self, {imported_symbol}}}")
	} else {
		format!("{prefix}::{{{imported_symbol}, {child_tail}}}")
	})
}

fn try_merge_child_into_direct_root_braced_use_path(
	use_path: &str,
	root_compact: &str,
	child_tail: &str,
) -> Option<String> {
	let (prefix, close, mut segments) = paths::parse_braced_path_parts_allow_alias(use_path)?;
	let prefix_root = prefix[..prefix.len().saturating_sub(1)].trim();
	let prefix_root = prefix_root.strip_suffix("::").unwrap_or(prefix_root).trim();

	if paths::compact_path_for_match(prefix_root) != *root_compact {
		return None;
	}
	if merge_child_tail_into_braced_segments(&mut segments, child_tail) {
		return Some(format!("{}{}{}", prefix, segments.join(", "), &use_path[close..=close]));
	}

	None
}

fn try_merge_child_into_parent_braced_use_path(
	use_path: &str,
	root_full: &str,
	child_tail: &str,
) -> Option<String> {
	let (parent_full, root_head) = root_full.rsplit_once("::")?;
	let (prefix, close, mut segments) = paths::parse_braced_path_parts_allow_alias(use_path)?;
	let prefix_root = prefix[..prefix.len().saturating_sub(1)].trim();
	let prefix_root = prefix_root.strip_suffix("::").unwrap_or(prefix_root).trim();

	if paths::compact_path_for_match(prefix_root) != paths::compact_path_for_match(parent_full) {
		return None;
	}

	let root_head_compact = paths::compact_path_for_match(root_head);
	let mut matched_root = false;
	let mut changed = false;

	for segment in &mut segments {
		let trimmed = segment.trim();

		if paths::compact_path_for_match(trimmed) == root_head_compact {
			*segment = format!("{root_head}::{{self, {child_tail}}}");
			matched_root = true;
			changed = true;

			break;
		}

		if let Some((head, child)) = trimmed.split_once("::")
			&& paths::compact_path_for_match(head) == root_head_compact
			&& !child.contains('{')
			&& !child.contains('}')
			&& child != "*"
		{
			if syntax::is_same_ident(child.trim(), child_tail) {
				matched_root = true;

				break;
			}

			*segment = if child_tail == "self" {
				format!("{head}::{{self, {}}}", child.trim())
			} else {
				format!("{head}::{{{}, {child_tail}}}", child.trim())
			};
			matched_root = true;
			changed = true;

			break;
		}

		let Some((head, inner)) = paths::parse_single_level_nested_use_segment(trimmed) else {
			continue;
		};

		if paths::compact_path_for_match(head) != root_head_compact {
			continue;
		}

		let mut nested_children = paths::split_top_level_csv(inner);
		let nested_changed =
			merge_child_tail_into_braced_segments(&mut nested_children, child_tail);

		if !nested_changed {
			matched_root = true;

			break;
		}

		*segment = format!("{head}::{{{}}}", nested_children.join(", "));
		matched_root = true;
		changed = true;

		break;
	}

	if !matched_root {
		return None;
	}
	if !changed {
		return Some(use_path.to_owned());
	}

	Some(format!("{}{}{}", prefix, segments.join(", "), &use_path[close..=close]))
}

fn merge_child_tail_into_braced_segments(segments: &mut Vec<String>, child_tail: &str) -> bool {
	let child_compact = paths::compact_path_for_match(child_tail);

	for segment in segments.iter_mut() {
		let trimmed = segment.trim();

		if paths::compact_path_for_match(trimmed) == child_compact {
			return false;
		}
		if is_keep_alive_alias_for_child(trimmed, child_tail) {
			*segment = child_tail.to_owned();

			return true;
		}

		let Some((head, inner)) = paths::parse_single_level_nested_use_segment(trimmed) else {
			continue;
		};
		let Some((child_head, child_rest)) = child_tail.split_once("::") else {
			continue;
		};

		if paths::compact_path_for_match(head) != paths::compact_path_for_match(child_head) {
			continue;
		}

		let child_rest_compact = paths::compact_path_for_match(child_rest);
		let mut children = paths::split_top_level_csv(inner);
		let mut found_exact = false;
		let mut replaced_alias = false;

		for child in children.iter_mut() {
			let child_trimmed = child.trim();

			if paths::compact_path_for_match(child_trimmed) == child_rest_compact {
				found_exact = true;

				break;
			}
			if is_keep_alive_alias_for_child(child_trimmed, child_rest) {
				*child = child_rest.to_owned();
				replaced_alias = true;

				break;
			}
		}

		if found_exact {
			return false;
		}
		if replaced_alias {
			*segment = format!("{head}::{{{}}}", children.join(", "));

			return true;
		}
		if child_rest_compact == "self" {
			children.insert(0, child_rest.to_owned());
		} else {
			children.push(child_rest.to_owned());
		}

		*segment = format!("{head}::{{{}}}", children.join(", "));

		return true;
	}

	if child_compact == "self" {
		segments.insert(0, child_tail.to_owned());
	} else {
		segments.push(child_tail.to_owned());
	}

	true
}

fn is_keep_alive_alias_for_child(segment: &str, child_tail: &str) -> bool {
	let Some((base, alias)) = paths::split_import_leaf_alias(segment) else {
		return false;
	};

	alias.is_some_and(|value| value == "_")
		&& paths::compact_path_for_match(base) == paths::compact_path_for_match(child_tail)
}

fn import008_insert_line(ctx: &FileContext) -> usize {
	if ctx.top_items.is_empty() {
		return 1;
	}

	let first_item_line = ctx.top_items[0].start_line;
	let mut after_leading_mods = first_item_line;
	let mut saw_leading_mod = false;

	for item in &ctx.top_items {
		if item.kind == TopKind::Mod && !paths::is_cfg_test_attrs(&item.attrs) {
			after_leading_mods = item.end_line + 1;
			saw_leading_mod = true;
		} else {
			break;
		}
	}

	if saw_leading_mod { after_leading_mods } else { first_item_line }
}
