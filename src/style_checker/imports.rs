use std::collections::{HashMap, HashSet};

use regex::Regex;

use super::shared::{
	Edit, FileContext, TopItem, TopKind, USE_RE, Violation, WORKSPACE_IMPORT_ROOTS,
	offset_from_line, push_violation, strip_string_and_line_comment,
};

fn extract_use_path_from_line(line: &str) -> Option<String> {
	USE_RE
		.captures(line)
		.and_then(|caps| caps.get(2).map(|capture| capture.as_str().trim().to_owned()))
}

fn extract_use_path_from_text(text: &str) -> Option<String> {
	find_use_path_range(text)
		.and_then(|(start, end)| text.get(start..end).map(|s| s.trim().to_owned()))
}

fn find_use_path_range(text: &str) -> Option<(usize, usize)> {
	for (idx, _) in text.match_indices("use") {
		let prev = text[..idx].chars().next_back();
		let next = text.get(idx + 3..).and_then(|tail| tail.chars().next());
		let is_prev_boundary = prev.is_none_or(|ch| !(ch.is_ascii_alphanumeric() || ch == '_'));
		let is_next_whitespace = next.is_some_and(char::is_whitespace);
		if !is_prev_boundary || !is_next_whitespace {
			continue;
		}

		let mut start = idx + 3;
		let bytes = text.as_bytes();
		while start < bytes.len() && bytes[start].is_ascii_whitespace() {
			start += 1;
		}
		let tail = text.get(start..)?;
		let semi = tail.find(';')?;
		return Some((start, start + semi));
	}
	None
}

fn extract_use_path(ctx: &FileContext, item: &TopItem) -> Option<String> {
	extract_use_path_from_text(&item.raw).or_else(|| {
		ctx.lines.get(item.line.saturating_sub(1)).and_then(|line| extract_use_path_from_line(line))
	})
}

fn imported_symbols_from_use_path(path: &str) -> Vec<String> {
	let compact = path.replace(' ', "");
	if compact.ends_with("::*") {
		return Vec::new();
	}

	fn normalize_symbol(segment: &str) -> Option<String> {
		let mut symbol = segment.trim().to_owned();
		if symbol.is_empty() {
			return None;
		}
		if let Some((left, _)) = symbol.split_once(" as ") {
			symbol = left.trim().to_owned();
		}
		if matches!(symbol.as_str(), "*" | "self" | "super" | "crate") {
			return None;
		}
		if let Some((_, right)) = symbol.rsplit_once("::") {
			symbol = right.to_owned();
		}
		if let Some(stripped) = symbol.strip_prefix("r#") {
			symbol = stripped.to_owned();
		}
		if symbol.is_empty() { None } else { Some(symbol) }
	}

	if path.contains('{') && path.contains('}') {
		let inside = path
			.split_once('{')
			.and_then(|(_, right)| right.rsplit_once('}').map(|(inside, _)| inside))
			.unwrap_or_default();
		let mut out = Vec::new();
		for segment in inside.split(',') {
			if let Some(symbol) = normalize_symbol(segment) {
				out.push(symbol);
			}
		}
		return out;
	}

	let tail = path.rsplit("::").next().unwrap_or(path);
	normalize_symbol(tail).into_iter().collect()
}

fn contains_unqualified_symbol_call(lines: &[String], symbol: &str, is_macro: bool) -> bool {
	let pattern = if is_macro {
		format!(r"\b{}!\s*\(", regex::escape(symbol))
	} else {
		format!(r"\b{}\s*\(", regex::escape(symbol))
	};
	let re = Regex::new(&pattern).unwrap();

	for line in lines {
		let code = strip_string_and_line_comment(line, false).0;
		for matched in re.find_iter(&code) {
			let prev =
				if matched.start() == 0 { None } else { code[..matched.start()].chars().last() };
			if prev != Some(':') {
				return true;
			}
		}
	}
	false
}

fn use_origin(path: &str) -> usize {
	let trimmed = path.replace("pub ", "");
	let root = trimmed.trim_start_matches(':').split("::").next().unwrap_or_default();
	if matches!(root, "std" | "core" | "alloc") {
		0
	} else if matches!(root, "crate" | "self" | "super")
		|| WORKSPACE_IMPORT_ROOTS.contains(root)
		|| WORKSPACE_IMPORT_ROOTS.contains(&root.replace('-', "_"))
	{
		2
	} else {
		1
	}
}

fn split_top_level_csv(text: &str) -> Vec<String> {
	let mut out = Vec::new();
	let mut start = 0_usize;
	let mut depth_brace = 0_i32;
	let mut depth_angle = 0_i32;
	let chars = text.char_indices().collect::<Vec<_>>();
	for (idx, ch) in &chars {
		match ch {
			'{' => depth_brace += 1,
			'}' => depth_brace = (depth_brace - 1).max(0),
			'<' => depth_angle += 1,
			'>' => depth_angle = (depth_angle - 1).max(0),
			',' if depth_brace == 0 && depth_angle == 0 => {
				let segment = text[start..*idx].trim();
				if !segment.is_empty() {
					out.push(segment.to_owned());
				}
				start = *idx + 1;
			},
			_ => {},
		}
	}
	let tail = text[start..].trim();
	if !tail.is_empty() {
		out.push(tail.to_owned());
	}
	out
}

fn normalize_mixed_self_child_use_path(path: &str) -> Option<String> {
	let open = path.find('{')?;
	let mut depth = 0_i32;
	let mut close = None;
	for (idx, ch) in path.char_indices().skip(open) {
		if ch == '{' {
			depth += 1;
		} else if ch == '}' {
			depth -= 1;
			if depth == 0 {
				close = Some(idx);
				break;
			}
		}
	}
	let close = close?;
	if !path[close + 1..].trim().is_empty() {
		return None;
	}

	let prefix = &path[..open + 1];
	let inner = &path[open + 1..close];
	let segments = split_top_level_csv(inner);
	if segments.is_empty() {
		return None;
	}
	if segments.iter().any(|segment| segment.contains(" as ")) {
		return None;
	}

	#[derive(Default)]
	struct Group {
		indices: Vec<usize>,
		has_self: bool,
		children: Vec<String>,
	}

	let mut groups: HashMap<String, Group> = HashMap::new();
	let mut parsed = Vec::new();
	for (idx, segment) in segments.iter().enumerate() {
		if let Some((head, rest)) = segment.split_once("::") {
			let head = head.trim();
			if head.is_empty() || head.contains('{') || head.contains('}') {
				parsed.push(None);
				continue;
			}
			if rest.starts_with('{') && rest.ends_with('}') {
				let inner = &rest[1..rest.len().saturating_sub(1)];
				let child_parts = split_top_level_csv(inner);
				let group = groups.entry(head.to_owned()).or_default();
				group.indices.push(idx);
				for child in child_parts {
					if child == "self" {
						group.has_self = true;
					} else {
						group.children.push(child);
					}
				}
				parsed.push(Some((head.to_owned(), true)));
				continue;
			}
			let child = rest.trim();
			if child.is_empty() || child.contains("::") {
				parsed.push(None);
				continue;
			}
			let group = groups.entry(head.to_owned()).or_default();
			group.indices.push(idx);
			group.children.push(child.to_owned());
			parsed.push(Some((head.to_owned(), false)));
			continue;
		}

		let head = segment.trim();
		if head.is_empty() || head.contains('{') || head.contains('}') {
			parsed.push(None);
			continue;
		}
		let group = groups.entry(head.to_owned()).or_default();
		group.indices.push(idx);
		group.has_self = true;
		parsed.push(Some((head.to_owned(), false)));
	}

	let mut emit = vec![true; segments.len()];
	let mut merged = false;
	let mut rewritten = Vec::new();
	for (idx, segment) in segments.iter().enumerate() {
		if !emit[idx] {
			continue;
		}
		let Some((head, _)) = parsed.get(idx).cloned().flatten() else {
			rewritten.push(segment.to_owned());
			continue;
		};
		let Some(group) = groups.get(&head) else {
			rewritten.push(segment.to_owned());
			continue;
		};
		if !group.has_self || group.children.is_empty() {
			rewritten.push(segment.to_owned());
			continue;
		}
		if group.indices.first().copied() != Some(idx) {
			emit[idx] = false;
			continue;
		}

		let mut seen = HashSet::new();
		let mut children = Vec::new();
		for child in &group.children {
			if seen.insert(child.clone()) {
				children.push(child.clone());
			}
		}
		let combined = format!("{head}::{{self, {}}}", children.join(", "));
		rewritten.push(combined);
		merged = true;
		for original_idx in group.indices.iter().skip(1) {
			emit[*original_idx] = false;
		}
	}

	if !merged {
		return None;
	}
	let rewritten_path = format!("{prefix}{}{}", rewritten.join(", "), &path[close..=close]);
	if rewritten_path == path { None } else { Some(rewritten_path) }
}

fn rewrite_use_item_with_path(raw: &str, new_path: &str) -> Option<String> {
	let (start, end) = find_use_path_range(raw)?;
	let mut out = String::new();
	out.push_str(&raw[..start]);
	out.push_str(new_path);
	out.push_str(&raw[end..]);
	Some(out)
}

fn collect_non_pub_use_runs(ctx: &FileContext) -> Vec<Vec<&TopItem>> {
	let mut runs = Vec::new();
	let mut current = Vec::new();

	for item in &ctx.top_items {
		if item.kind == TopKind::Use && !item.is_pub {
			current.push(item);
			continue;
		}
		if !current.is_empty() {
			runs.push(std::mem::take(&mut current));
		}
	}
	if !current.is_empty() {
		runs.push(current);
	}

	runs
}

fn separator_lines<'a>(ctx: &'a FileContext, prev: &TopItem, curr: &TopItem) -> &'a [String] {
	let start = prev.end_line;
	let end = curr.start_line.saturating_sub(1);
	if start >= end || end > ctx.lines.len() { &[] } else { &ctx.lines[start..end] }
}

fn item_text_range(ctx: &FileContext, item: &TopItem) -> Option<(usize, usize)> {
	let start = offset_from_line(&ctx.line_starts, item.start_line)?;
	let end = offset_from_line(&ctx.line_starts, item.end_line + 1).unwrap_or(ctx.text.len());
	if end < start { None } else { Some((start, end)) }
}

fn run_text_range(ctx: &FileContext, first: &TopItem, last: &TopItem) -> Option<(usize, usize)> {
	let start = offset_from_line(&ctx.line_starts, first.start_line)?;
	let end = offset_from_line(&ctx.line_starts, last.end_line + 1).unwrap_or(ctx.text.len());
	if end < start { None } else { Some((start, end)) }
}

fn build_import_group_fix_plans(
	ctx: &FileContext,
	use_runs: &[Vec<&TopItem>],
	import007_lines: &HashSet<usize>,
) -> (Vec<Edit>, HashSet<usize>) {
	struct UseEntry<'a> {
		item: &'a TopItem,
		origin: usize,
		block: String,
	}

	let mut planned_edits = Vec::new();
	let mut fixable_lines = HashSet::new();

	for run in use_runs {
		if run.len() < 2 {
			continue;
		}
		if run.iter().any(|item| import007_lines.contains(&item.line)) {
			continue;
		}

		let mut safe_to_rewrite = true;
		for pair in run.windows(2) {
			for line in separator_lines(ctx, pair[0], pair[1]) {
				let trimmed = line.trim();
				if trimmed.is_empty() {
					continue;
				}
				safe_to_rewrite = false;
				break;
			}
			if !safe_to_rewrite {
				break;
			}
		}
		if !safe_to_rewrite {
			continue;
		}

		let mut entries = Vec::with_capacity(run.len());
		for item in run {
			let Some(path) = extract_use_path(ctx, item) else {
				safe_to_rewrite = false;
				break;
			};
			let Some((start, end)) = item_text_range(ctx, item) else {
				safe_to_rewrite = false;
				break;
			};
			let Some(block) = ctx.text.get(start..end) else {
				safe_to_rewrite = false;
				break;
			};

			let mut block_lines = Vec::new();
			let mut has_item_start = false;
			for line in block.lines() {
				let trimmed = line.trim();
				if !has_item_start {
					if trimmed.is_empty() {
						continue;
					}
					if line.trim_start().starts_with("//") {
						safe_to_rewrite = false;
						break;
					}
					has_item_start = true;
				}
				block_lines.push(line);
			}
			if !safe_to_rewrite || block_lines.is_empty() {
				safe_to_rewrite = false;
				break;
			}

			let mut normalized_block = block_lines.join("\n");
			if block.ends_with('\n') {
				normalized_block.push('\n');
			}
			entries.push(UseEntry { item, origin: use_origin(&path), block: normalized_block });
		}
		if !safe_to_rewrite {
			continue;
		}
		if entries.windows(2).any(|pair| pair[1].origin < pair[0].origin) {
			continue;
		}

		let Some((run_start, run_end)) = run_text_range(ctx, run[0], run[run.len() - 1]) else {
			continue;
		};
		let Some(original) = ctx.text.get(run_start..run_end) else {
			continue;
		};

		let ordered_entries = entries.iter().collect::<Vec<_>>();

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

pub(crate) fn check_import_rules(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	let use_runs = collect_non_pub_use_runs(ctx);
	let use_items = use_runs.iter().flat_map(|run| run.iter().copied()).collect::<Vec<_>>();

	let mut has_prelude_glob = false;
	let mut import007_lines = HashSet::new();
	for item in &use_items {
		if let Some(path) = extract_use_path(ctx, item) {
			if path.replace(' ', "") == "crate::prelude::*" {
				has_prelude_glob = true;
			}
		}
	}

	for item in &use_items {
		let Some(path) = extract_use_path(ctx, item) else {
			continue;
		};
		if let Some(normalized) = normalize_mixed_self_child_use_path(&path) {
			push_violation(
				violations,
				ctx,
				item.line,
				"RUST-STYLE-IMPORT-002",
				"Normalize imports like `use a::{b, b::c}` to `use a::{b::{self, c}}`.",
				true,
			);
			if emit_edits {
				if let Some((start, end)) = item_text_range(ctx, item) {
					if let Some(raw) = ctx.text.get(start..end) {
						if let Some(rewritten) = rewrite_use_item_with_path(raw, &normalized) {
							edits.push(Edit {
								start,
								end,
								replacement: rewritten,
								rule: "RUST-STYLE-IMPORT-002",
							});
						}
					}
				}
			}
		}

		if let Some(alias_caps) =
			Regex::new(r"\bas\s+([A-Za-z_][A-Za-z0-9_]*)\b").unwrap().captures(&path)
		{
			if alias_caps.get(1).map(|m| m.as_str()) != Some("_") {
				push_violation(
					violations,
					ctx,
					item.line,
					"RUST-STYLE-IMPORT-003",
					"Import aliases are not allowed except `as _` in test keep-alive modules.",
					false,
				);
			}
		}

		let compact_path = path.replace(' ', "");
		if has_prelude_glob
			&& compact_path.starts_with("crate::")
			&& compact_path != "crate::prelude::*"
		{
			import007_lines.insert(item.line);
			push_violation(
				violations,
				ctx,
				item.line,
				"RUST-STYLE-IMPORT-007",
				"Avoid redundant crate imports when crate::prelude::* is imported.",
				true,
			);
			if emit_edits {
				if let (Some(start), Some(next)) = (
					offset_from_line(&ctx.line_starts, item.start_line),
					offset_from_line(&ctx.line_starts, item.end_line + 1),
				) {
					edits.push(Edit {
						start,
						end: next,
						replacement: String::new(),
						rule: "RUST-STYLE-IMPORT-007",
					});
				}
			}
		}

		if path.contains("::") {
			let imported_symbols = imported_symbols_from_use_path(&path);
			for symbol in imported_symbols {
				if symbol.is_empty() || !symbol.chars().next().is_some_and(char::is_lowercase) {
					continue;
				}

				let local_fn_def_re = Regex::new(&format!(
					r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:const\s+)?(?:unsafe\s+)?fn\s+{}\b",
					regex::escape(&symbol)
				))
				.unwrap();
				let local_macro_def_re = Regex::new(&format!(
					r"^\s*(?:macro_rules!\s*{}\b|macro\s+{}\b)",
					regex::escape(&symbol),
					regex::escape(&symbol),
				))
				.unwrap();

				let local_fn_defined = ctx.lines.iter().any(|line| {
					let code = strip_string_and_line_comment(line, false).0;
					local_fn_def_re.is_match(&code)
				});
				let local_macro_defined = ctx.lines.iter().any(|line| {
					let code = strip_string_and_line_comment(line, false).0;
					local_macro_def_re.is_match(&code)
				});
				let called_fn_unqualified =
					contains_unqualified_symbol_call(&ctx.lines, &symbol, false);
				let called_macro_unqualified =
					contains_unqualified_symbol_call(&ctx.lines, &symbol, true);

				if (called_fn_unqualified && !local_fn_defined)
					|| (called_macro_unqualified && !local_macro_defined)
				{
					push_violation(
						violations,
						ctx,
						item.line,
						"RUST-STYLE-IMPORT-004",
						"Do not import free functions or macros into scope; prefer qualified module paths.",
						false,
					);
					break;
				}
			}
		}
	}

	let mut imported_symbol_paths: HashMap<String, HashSet<String>> = HashMap::new();
	let mut imported_symbol_lines: HashMap<String, HashSet<usize>> = HashMap::new();
	for item in &use_items {
		let Some(path) = extract_use_path(ctx, item) else {
			continue;
		};
		for symbol in imported_symbols_from_use_path(&path) {
			imported_symbol_paths.entry(symbol.clone()).or_default().insert(path.clone());
			imported_symbol_lines.entry(symbol).or_default().insert(item.line);
		}
	}
	for (symbol, paths) in imported_symbol_paths {
		if paths.len() <= 1 {
			continue;
		}
		for line in imported_symbol_lines.get(&symbol).into_iter().flat_map(|lines| lines.iter()) {
			push_violation(
				violations,
				ctx,
				*line,
				"RUST-STYLE-IMPORT-004",
				&format!(
					"Ambiguous imported symbol `{symbol}` is not allowed; use fully qualified paths."
				),
				false,
			);
		}
	}

	let (planned_import_group_edits, fixable_import_group_lines) =
		build_import_group_fix_plans(ctx, &use_runs, &import007_lines);
	if emit_edits {
		edits.extend(planned_import_group_edits);
	}

	for run in &use_runs {
		for pair in run.windows(2) {
			let prev = pair[0];
			let curr = pair[1];
			let Some(prev_path) = extract_use_path(ctx, prev) else {
				continue;
			};
			let Some(curr_path) = extract_use_path(ctx, curr) else {
				continue;
			};

			let prev_origin = use_origin(&prev_path);
			let curr_origin = use_origin(&curr_path);
			let is_fixable = fixable_import_group_lines.contains(&curr.line);
			if curr_origin < prev_origin {
				push_violation(
					violations,
					ctx,
					curr.line,
					"RUST-STYLE-IMPORT-001",
					"Import groups must be ordered: std, third-party, self/workspace.",
					is_fixable,
				);
			}

			let between = separator_lines(ctx, prev, curr);
			let has_blank = between.iter().any(|line| line.trim().is_empty());
			let has_header_comment = between.iter().any(|line| line.trim_start().starts_with("//"));

			if curr_origin != prev_origin && !has_blank {
				push_violation(
					violations,
					ctx,
					curr.line,
					"RUST-STYLE-IMPORT-002",
					"Separate import groups with one blank line.",
					is_fixable,
				);
			}
			if curr_origin == prev_origin && has_blank {
				push_violation(
					violations,
					ctx,
					curr.line,
					"RUST-STYLE-IMPORT-002",
					"Do not place blank lines inside an import group.",
					is_fixable,
				);
			}
			if has_header_comment {
				push_violation(
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
