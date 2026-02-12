use std::collections::{HashMap, HashSet};

use ra_ap_syntax::{AstNode, ast};

use super::shared::{
	Edit, FileContext, SERDE_DEFAULT_RE, TopItem, TopKind, Violation, offset_from_line,
};

pub(crate) fn check_mod_rs(ctx: &FileContext, violations: &mut Vec<Violation>) {
	if ctx.path.file_name().is_some_and(|name| name == "mod.rs") {
		super::shared::push_violation(
			violations,
			ctx,
			1,
			"RUST-STYLE-FILE-001",
			"Do not use mod.rs. Use flat module files instead.",
			false,
		);
	}
}

pub(crate) fn check_serde_option_default(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	for (idx, line) in ctx.lines.iter().enumerate() {
		if !SERDE_DEFAULT_RE.is_match(line) {
			continue;
		}

		let Some(next_idx) = next_non_attribute_line(&ctx.lines, idx) else {
			continue;
		};

		if !ctx.lines[next_idx].contains(": Option<") {
			continue;
		}

		super::shared::push_violation(
			violations,
			ctx,
			idx + 1,
			"RUST-STYLE-SERDE-001",
			"Do not use #[serde(default)] on Option<T> fields.",
			true,
		);

		if !emit_edits {
			continue;
		}

		let Some(start) = offset_from_line(&ctx.line_starts, idx + 1) else {
			continue;
		};
		let Some(end) = offset_from_line(&ctx.line_starts, idx + 2) else {
			continue;
		};

		match rewrite_serde_default_attr_line(line) {
			Some(rewritten) => edits.push(Edit {
				start,
				end,
				replacement: format!("{rewritten}\n"),
				rule: "RUST-STYLE-SERDE-001",
			}),
			None => edits.push(Edit {
				start,
				end,
				replacement: String::new(),
				rule: "RUST-STYLE-SERDE-001",
			}),
		}
	}
}

pub(crate) fn check_error_rs_no_use(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	if ctx.path.file_name().is_none_or(|name| name != "error.rs") {
		return;
	}
	let use_items =
		ctx.top_items.iter().filter(|item| item.kind == TopKind::Use).collect::<Vec<_>>();

	if use_items.is_empty() {
		return;
	}

	let local_defined_symbols = ctx
		.top_items
		.iter()
		.filter_map(|item| item.name.as_deref())
		.map(|name| normalize_ident(name).to_owned())
		.collect::<HashSet<_>>();
	let mut symbol_paths: HashMap<String, HashSet<String>> = HashMap::new();
	let mut symbol_ranges: HashMap<String, Vec<(usize, usize, String)>> = HashMap::new();
	let mut parse_failed = false;

	for item in &use_items {
		let Some(path) = extract_use_path(item) else {
			parse_failed = true;

			continue;
		};
		let Some(bindings) = collect_import_bindings_from_use_path(&path) else {
			parse_failed = true;

			continue;
		};
		if bindings.is_empty() {
			parse_failed = true;

			continue;
		}

		for (symbol, full_path) in bindings {
			symbol_paths.entry(symbol).or_default().insert(full_path);
		}
	}

	let mut fixable = !parse_failed;

	for symbol in symbol_paths.keys() {
		let Some(full_path) =
			symbol_paths.get(symbol).and_then(|paths| paths.iter().next()).cloned()
		else {
			fixable = false;

			continue;
		};
		let mut ranges = unqualified_path_rewrites(ctx, symbol, &full_path);

		ranges.extend(unqualified_macro_call_rewrites(ctx, symbol, &full_path));
		ranges.sort_by_key(|(start, end, _)| (*start, *end));
		ranges.dedup();

		if !ranges.is_empty()
			&& (local_defined_symbols.contains(symbol)
				|| symbol_paths.get(symbol).is_none_or(|paths| paths.len() != 1))
		{
			fixable = false;
		}

		symbol_ranges.insert(symbol.clone(), ranges);
	}

	for item in &use_items {
		super::shared::push_violation(
			violations,
			ctx,
			item.line,
			"RUST-STYLE-IMPORT-005",
			"Do not add use imports in error.rs; use fully qualified paths.",
			fixable,
		);
	}

	if !emit_edits || !fixable {
		return;
	}

	for (_symbol, ranges) in symbol_ranges {
		if ranges.is_empty() {
			continue;
		}

		for (start, end, replacement) in ranges {
			edits.push(Edit { start, end, replacement, rule: "RUST-STYLE-IMPORT-005" });
		}
	}
	for item in use_items {
		let Some(start) = offset_from_line(&ctx.line_starts, item.start_line) else {
			continue;
		};
		let end = offset_from_line(&ctx.line_starts, item.end_line + 1).unwrap_or(ctx.text.len());

		edits.push(Edit { start, end, replacement: String::new(), rule: "RUST-STYLE-IMPORT-005" });
	}
}

fn normalize_ident(name: &str) -> &str {
	name.strip_prefix("r#").unwrap_or(name)
}

fn is_same_ident(lhs: &str, rhs: &str) -> bool {
	normalize_ident(lhs) == normalize_ident(rhs)
}

fn extract_use_path(item: &TopItem) -> Option<String> {
	find_use_path_range(&item.raw)
		.and_then(|(start, end)| item.raw.get(start..end).map(|s| s.trim().to_owned()))
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

fn collect_import_bindings_from_use_path(path: &str) -> Option<Vec<(String, String)>> {
	let mut out = Vec::new();

	if !collect_import_bindings_from_segment(path.trim(), &mut out) {
		return None;
	}

	Some(out)
}

fn collect_import_bindings_from_segment(segment: &str, out: &mut Vec<(String, String)>) -> bool {
	let trimmed = segment.trim();

	if trimmed.is_empty() {
		return true;
	}
	if trimmed.ends_with("::*") {
		return false;
	}

	let mut brace_start = None;
	let mut depth = 0_i32;
	let mut brace_end = None;

	for (idx, ch) in trimmed.char_indices() {
		if ch == '{' {
			if brace_start.is_none() {
				brace_start = Some(idx);
			}
			depth += 1;
		} else if ch == '}' {
			depth -= 1;
			if depth < 0 {
				return false;
			}
			if depth == 0 {
				brace_end = Some(idx);
			}
		}
	}

	if depth != 0 {
		return false;
	}

	if let (Some(open), Some(close)) = (brace_start, brace_end) {
		let prefix = trimmed[..open].trim();
		let inner = &trimmed[open + 1..close];
		let suffix = trimmed[close + 1..].trim();

		if !suffix.is_empty() {
			return false;
		}
		if !prefix.is_empty() && !prefix.ends_with("::") {
			return false;
		}

		let prefix = prefix.strip_suffix("::").unwrap_or(prefix).trim();

		for child in split_top_level_csv(inner) {
			let child = child.trim();

			if child.is_empty() {
				continue;
			}
			if child == "self" {
				if prefix.is_empty() {
					return false;
				}
				let symbol = prefix.rsplit("::").next().unwrap_or(prefix).trim();
				let symbol = normalize_ident(symbol);

				if symbol.is_empty() {
					return false;
				}

				out.push((symbol.to_owned(), prefix.replace(' ', "")));

				continue;
			}

			let expanded =
				if prefix.is_empty() { child.to_owned() } else { format!("{prefix}::{child}") };

			if !collect_import_bindings_from_segment(&expanded, out) {
				return false;
			}
		}

		return true;
	}

	let (base, alias) = if let Some((left, right)) = trimmed.split_once(" as ") {
		let alias = right.trim();

		if alias.is_empty() {
			return false;
		}

		(left.trim(), Some(alias))
	} else {
		(trimmed, None)
	};

	if base.is_empty() || matches!(base, "self" | "super" | "crate" | "*") || base.ends_with("::*")
	{
		return false;
	}
	if alias == Some("_") {
		return true;
	}

	let full_path = base.replace(' ', "");
	let symbol = alias.unwrap_or_else(|| base.rsplit("::").next().unwrap_or(base)).trim();
	let symbol = normalize_ident(symbol);

	if symbol.is_empty() || matches!(symbol, "self" | "super" | "crate" | "*") {
		return false;
	}

	out.push((symbol.to_owned(), full_path));

	true
}

fn unqualified_path_rewrites(
	ctx: &FileContext,
	symbol: &str,
	qualified_path: &str,
) -> Vec<(usize, usize, String)> {
	let mut rewrites = Vec::new();

	for path in ctx.source_file.syntax().descendants().filter_map(ast::Path::cast) {
		if path.qualifier().is_some() {
			continue;
		}
		if path.syntax().ancestors().any(|node| ast::Use::cast(node).is_some()) {
			continue;
		}

		let Some(segment) = path.segment() else {
			continue;
		};
		let Some(name_ref) = segment.name_ref() else {
			continue;
		};

		if !is_same_ident(name_ref.text().as_str(), symbol) {
			continue;
		}

		let segment_text = segment.syntax().text().to_string();
		let name_text = name_ref.text().to_string();
		let suffix = segment_text.strip_prefix(&name_text).unwrap_or_default();

		rewrites.push((
			usize::from(path.syntax().text_range().start()),
			usize::from(path.syntax().text_range().end()),
			format!("{qualified_path}{suffix}"),
		));
	}

	rewrites
}

fn unqualified_macro_call_rewrites(
	ctx: &FileContext,
	symbol: &str,
	qualified_path: &str,
) -> Vec<(usize, usize, String)> {
	let mut rewrites = Vec::new();

	for macro_call in ctx.source_file.syntax().descendants().filter_map(ast::MacroCall::cast) {
		let Some(path) = macro_call.path() else {
			continue;
		};

		if path.qualifier().is_some() {
			continue;
		}

		let Some(segment) = path.segment() else {
			continue;
		};
		let Some(name_ref) = segment.name_ref() else {
			continue;
		};

		if !is_same_ident(name_ref.text().as_str(), symbol) {
			continue;
		}

		rewrites.push((
			usize::from(path.syntax().text_range().start()),
			usize::from(path.syntax().text_range().end()),
			qualified_path.to_owned(),
		));
	}

	rewrites
}

fn next_non_attribute_line(lines: &[String], idx: usize) -> Option<usize> {
	let mut cursor = idx + 1;

	while cursor < lines.len() {
		let stripped = lines[cursor].trim();

		if stripped.is_empty()
			|| stripped.starts_with("#[")
			|| stripped.starts_with("///")
			|| stripped.starts_with("//!")
		{
			cursor += 1;

			continue;
		}

		return Some(cursor);
	}

	None
}

fn rewrite_serde_default_attr_line(line: &str) -> Option<String> {
	let open = line.find('(')?;
	let close = line.rfind(')')?;

	if close <= open {
		return None;
	}

	let leading = line.chars().take_while(|ch| ch.is_whitespace()).collect::<String>();
	let inner = &line[open + 1..close];
	let args = split_top_level_csv(inner);
	let kept = args
		.into_iter()
		.filter_map(|arg| {
			let trimmed = arg.trim();

			if trimmed.is_empty() {
				return None;
			}
			if trimmed.starts_with("default") {
				return None;
			}

			Some(trimmed.to_owned())
		})
		.collect::<Vec<_>>();

	if kept.is_empty() { None } else { Some(format!("{leading}#[serde({})]", kept.join(", "))) }
}

fn split_top_level_csv(text: &str) -> Vec<String> {
	let mut out = Vec::new();
	let mut start = 0_usize;
	let mut depth_paren = 0_i32;
	let mut depth_brace = 0_i32;
	let mut depth_bracket = 0_i32;
	let mut in_str = false;
	let mut escaped = false;

	for (idx, ch) in text.char_indices() {
		if in_str {
			if escaped {
				escaped = false;
			} else if ch == '\\' {
				escaped = true;
			} else if ch == '"' {
				in_str = false;
			}

			continue;
		}

		match ch {
			'"' => in_str = true,
			'(' => depth_paren += 1,
			')' => depth_paren -= 1,
			'{' => depth_brace += 1,
			'}' => depth_brace -= 1,
			'[' => depth_bracket += 1,
			']' => depth_bracket -= 1,
			',' if depth_paren == 0 && depth_brace == 0 && depth_bracket == 0 => {
				out.push(text[start..idx].to_owned());
				start = idx + 1;
			},
			_ => {},
		}
	}

	out.push(text[start..].to_owned());
	out
}
