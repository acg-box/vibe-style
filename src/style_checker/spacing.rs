use std::collections::{BTreeSet, HashSet};

use regex::Regex;

use super::{
	quality,
	shared::{
		Edit, FileContext, Violation, offset_from_line, push_violation,
		strip_string_and_line_comment,
	},
};

fn normalize_statement_text(statement_lines: &[String]) -> String {
	let mut parts = Vec::new();
	let mut in_str = false;
	for raw in statement_lines {
		let (mut code, next_state) = strip_string_and_line_comment(raw, in_str);
		in_str = next_state;
		code = code.trim().to_owned();
		if code.is_empty() || code.starts_with('#') {
			continue;
		}
		parts.push(code);
	}
	parts.join(" ")
}

fn strip_turbofish(text: &str) -> String {
	let mut out = String::with_capacity(text.len());
	let mut idx = 0;
	let chars = text.chars().collect::<Vec<_>>();

	while idx < chars.len() {
		if idx + 2 < chars.len()
			&& chars[idx] == ':'
			&& chars[idx + 1] == ':'
			&& chars[idx + 2] == '<'
		{
			idx += 3;
			let mut depth = 1_i32;
			while idx < chars.len() && depth > 0 {
				if chars[idx] == '<' {
					depth += 1;
				} else if chars[idx] == '>' {
					depth -= 1;
				}
				idx += 1;
			}
			continue;
		}
		out.push(chars[idx]);
		idx += 1;
	}

	out
}

fn parse_ufcs_target_call(text: &str) -> Option<(String, String)> {
	if !text.starts_with('<') {
		return None;
	}
	let chars = text.chars().collect::<Vec<_>>();
	let mut depth = 0_i32;
	let mut close_idx = None;
	for (idx, ch) in chars.iter().enumerate() {
		if *ch == '<' {
			depth += 1;
		} else if *ch == '>' {
			depth -= 1;
			if depth == 0 {
				close_idx = Some(idx);
				break;
			}
		}
	}
	let close_idx = close_idx?;
	let body = text[1..close_idx].trim();
	let mut rest = text[close_idx + 1..].trim_start();
	if !rest.starts_with("::") {
		return None;
	}
	rest = &rest[2..];
	let fn_match = Regex::new(r"^(?P<func>[A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap().captures(rest)?;
	let func = fn_match.name("func")?.as_str().to_owned();
	let target = if let Some((_, right)) = body.split_once(" as ") {
		right.trim().to_owned()
	} else {
		body.to_owned()
	};
	if target.is_empty() { None } else { Some((target, func)) }
}

fn contains_assignment_operator(text: &str) -> bool {
	fn is_ident_char(ch: char) -> bool {
		ch.is_ascii_alphanumeric() || ch == '_'
	}

	for op in ["+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "<<=", ">>="] {
		if text.contains(op) {
			return true;
		}
	}

	let bytes = text.as_bytes();
	for idx in 0..bytes.len() {
		if bytes[idx] != b'=' {
			continue;
		}
		let prev = if idx > 0 { Some(bytes[idx - 1] as char) } else { None };
		let next = if idx + 1 < bytes.len() { Some(bytes[idx + 1] as char) } else { None };
		let prev_prev = if idx > 1 { Some(bytes[idx - 2] as char) } else { None };
		if prev == Some('=') || prev == Some('!') || prev == Some('<') || prev == Some('>') {
			continue;
		}
		if next == Some('=') || next == Some('>') {
			continue;
		}
		if prev == Some('.') && prev_prev == Some('.') {
			continue;
		}
		if prev.is_some_and(is_ident_char) && next.is_some_and(is_ident_char) {
			continue;
		}
		return true;
	}

	false
}

fn classify_statement_type(statement_lines: &[String]) -> String {
	let mut normalized = normalize_statement_text(statement_lines);
	if normalized.is_empty() {
		return "empty".to_owned();
	}
	normalized = strip_turbofish(&normalized);
	let first = normalized.as_str();

	if Regex::new(r"^let\b").unwrap().is_match(first) {
		return "let".to_owned();
	}
	if Regex::new(r"^if\s+let\b").unwrap().is_match(first) {
		return "if-let".to_owned();
	}
	if Regex::new(r"^if\b").unwrap().is_match(first) {
		return "if".to_owned();
	}
	if Regex::new(r"^match\b").unwrap().is_match(first) {
		return "match".to_owned();
	}
	if Regex::new(r"^for\b").unwrap().is_match(first) {
		return "for".to_owned();
	}
	if Regex::new(r"^while\b").unwrap().is_match(first) {
		return "while".to_owned();
	}
	if Regex::new(r"^loop\b").unwrap().is_match(first) {
		return "loop".to_owned();
	}
	if Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*(?:\.await)?\?\s*;?$")
		.unwrap()
		.is_match(first)
	{
		return "try-expr".to_owned();
	}
	if Regex::new(r"^(?P<name>[A-Za-z_][A-Za-z0-9_:]*)!\s*\(").unwrap().is_match(first) {
		let macro_name = Regex::new(r"^(?P<name>[A-Za-z_][A-Za-z0-9_:]*)!\s*\(")
			.unwrap()
			.captures(first)
			.and_then(|caps| caps.name("name"))
			.map(|value| value.as_str().to_owned())
			.unwrap_or_default();
		if macro_name.contains("::") {
			return "macro-path".to_owned();
		}
		return "macro".to_owned();
	}
	if contains_assignment_operator(first) {
		return "assign".to_owned();
	}
	if parse_ufcs_target_call(first).is_some() {
		return "path-call".to_owned();
	}
	if Regex::new(r"^(?P<target>[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)+)\s*\(")
		.unwrap()
		.is_match(first)
	{
		return "path-call".to_owned();
	}
	if Regex::new(r"^(?P<target>[A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap().is_match(first) {
		return "call".to_owned();
	}
	if Regex::new(r"^[^;]*\.(?P<method>[A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap().is_match(first) {
		return "method".to_owned();
	}

	let token = Regex::new(r"[\s({;]").unwrap().split(first).next().unwrap_or_default();
	if token.is_empty() { "other".to_owned() } else { format!("shape:{token}") }
}

fn extract_top_level_statements(
	lines: &[String],
	fn_start: usize,
	fn_end: usize,
) -> Vec<(usize, usize, String)> {
	let mut statements = Vec::new();
	let mut brace_depth = 1_i32;
	let mut paren_depth = 0_i32;
	let mut bracket_depth = 0_i32;
	let mut current_start: Option<usize> = None;

	for idx in (fn_start + 1)..fn_end {
		let raw_line = &lines[idx];
		let stripped = raw_line.trim();
		let code = strip_string_and_line_comment(raw_line, false).0;

		if current_start.is_none()
			&& brace_depth == 1
			&& !stripped.is_empty()
			&& !stripped.starts_with("//")
			&& !stripped.starts_with('#')
			&& stripped != "}"
		{
			current_start = Some(idx);
		}

		for ch in code.chars() {
			match ch {
				'(' => paren_depth += 1,
				')' => paren_depth = (paren_depth - 1).max(0),
				'[' => bracket_depth += 1,
				']' => bracket_depth = (bracket_depth - 1).max(0),
				'{' => brace_depth += 1,
				'}' => brace_depth = (brace_depth - 1).max(0),
				_ => {},
			}
		}

		let Some(current_start_value) = current_start else {
			continue;
		};

		let stripped_code = code.trim();
		let statement_closed = brace_depth == 1
			&& paren_depth == 0
			&& bracket_depth == 0
			&& !stripped_code.is_empty()
			&& (stripped_code.ends_with(';') || stripped_code.ends_with('}'));

		if statement_closed {
			let span_lines = lines[current_start_value..=idx].to_vec();
			statements.push((current_start_value, idx, classify_statement_type(&span_lines)));
			current_start = None;
		}
	}

	if let Some(current_start) = current_start {
		if fn_end > current_start {
			let span_lines = lines[current_start..fn_end].to_vec();
			statements.push((
				current_start,
				fn_end.saturating_sub(1),
				classify_statement_type(&span_lines),
			));
		}
	}

	statements
}

fn first_significant_statement_line(lines: &[String]) -> Option<String> {
	for line in lines {
		let stripped = line.trim();
		if stripped.is_empty() || stripped.starts_with("//") || stripped.starts_with('#') {
			continue;
		}
		return Some(stripped.to_owned());
	}
	None
}

fn last_significant_statement_line(lines: &[String]) -> Option<String> {
	for line in lines.iter().rev() {
		let stripped = line.trim();
		if stripped.is_empty() || stripped.starts_with("//") || stripped.starts_with('#') {
			continue;
		}
		return Some(stripped.to_owned());
	}
	None
}

fn is_return_or_tail_statement(statement_lines: &[String]) -> bool {
	let Some(first) = first_significant_statement_line(statement_lines) else {
		return false;
	};
	if Regex::new(r"^return\b").unwrap().is_match(&first) {
		return true;
	}
	let Some(last) = last_significant_statement_line(statement_lines) else {
		return false;
	};
	if Regex::new(r"^return\b").unwrap().is_match(&last) {
		return true;
	}
	if last.ends_with(';')
		|| last.ends_with('{')
		|| last.ends_with(',')
		|| matches!(last.as_str(), "}" | "};")
	{
		return false;
	}
	true
}

fn is_explicit_return_statement(statement_lines: &[String]) -> bool {
	first_significant_statement_line(statement_lines)
		.map(|first| Regex::new(r"^return\b").unwrap().is_match(&first))
		.unwrap_or(false)
}

fn extract_top_level_brace_blocks_in_span(
	lines: &[String],
	span_start: usize,
	span_end: usize,
) -> Vec<(usize, usize)> {
	let mut blocks = Vec::new();
	let mut depth = 0_i32;
	let mut current_start: Option<usize> = None;

	for idx in span_start..=span_end {
		let code = strip_string_and_line_comment(&lines[idx], false).0;
		for ch in code.chars() {
			if ch == '{' {
				depth += 1;
				if depth == 1 {
					current_start = Some(idx);
				}
			} else if ch == '}' {
				if depth == 1 {
					if let Some(start) = current_start {
						blocks.push((start, idx));
						current_start = None;
					}
				}
				depth = (depth - 1).max(0);
			}
		}
	}

	blocks
}

fn is_data_like_brace_block(lines: &[String], block_start: usize, block_end: usize) -> bool {
	let mut content = Vec::new();
	for line in lines.iter().take(block_end).skip(block_start + 1) {
		let code = strip_string_and_line_comment(line, false).0;
		let code = code.trim().to_owned();
		if code.is_empty() || code.starts_with('#') {
			continue;
		}
		content.push(code);
	}

	if content.is_empty() {
		return true;
	}

	for line in &content {
		if line.contains("=>") || line.contains(';') {
			return false;
		}
		if Regex::new(r"^(if|if\s+let|match|for|while|loop|return|let)\b").unwrap().is_match(line) {
			return false;
		}
	}

	for line in &content {
		if Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*\s*:\s*.+,?$").unwrap().is_match(line) {
			continue;
		}
		if line.ends_with(',') {
			continue;
		}
		return false;
	}

	true
}

fn between_is_blank_only(lines: &[String], start: usize, end: usize) -> bool {
	if start >= end {
		return true;
	}
	lines[start..end].iter().all(|line| line.trim().is_empty())
}

fn is_metadata_line(line: &str) -> bool {
	let trimmed = line.trim_start();
	trimmed.starts_with('#')
		|| trimmed.starts_with("//")
		|| trimmed.starts_with("/*")
		|| trimmed.starts_with('*')
}

fn between_same_type_can_autofix(lines: &[String], start: usize, end: usize) -> bool {
	if start >= end {
		return true;
	}
	lines[start..end].iter().all(|line| line.trim().is_empty() || is_metadata_line(line))
}

fn same_type_replacement_without_blank_lines(lines: &[String], start: usize, end: usize) -> String {
	if start >= end {
		return String::new();
	}
	let mut parts = Vec::new();
	for line in &lines[start..end] {
		if line.trim().is_empty() {
			continue;
		}
		parts.push(line.as_str());
	}
	if parts.is_empty() {
		String::new()
	} else {
		let mut out = parts.join("\n");
		out.push('\n');
		out
	}
}

fn is_item_like_statement(statement_lines: &[String]) -> bool {
	let Some(first) = first_significant_statement_line(statement_lines) else {
		return false;
	};
	Regex::new(
		r"^(?:pub(?:\([^)]*\))?\s+)?(?:(?:async|const|unsafe)\s+)*(?:fn|struct|enum|impl|trait|type|use|mod|static|const|macro_rules!|macro)\b",
	)
	.unwrap()
	.is_match(first.trim())
}

fn is_pipe_pattern_continuation_statement(statement_lines: &[String]) -> bool {
	first_significant_statement_line(statement_lines)
		.map(|line| line.trim_start().starts_with('|'))
		.unwrap_or(false)
}

fn is_const_group_statement(statement_lines: &[String]) -> bool {
	let Some(first) = first_significant_statement_line(statement_lines) else {
		return false;
	};
	Regex::new(r"^(?:pub(?:\([^)]*\))?\s+)?(?:const|static(?:\s+mut)?)\b")
		.unwrap()
		.is_match(first.trim())
}

fn item_between_replacement_with_single_blank(
	lines: &[String],
	start: usize,
	end: usize,
) -> String {
	let mut parts = Vec::new();
	if start < end {
		for line in &lines[start..end] {
			if line.trim().is_empty() {
				continue;
			}
			parts.push(line.as_str());
		}
	}
	if parts.is_empty() {
		return "\n".to_owned();
	}
	let mut out = String::from("\n");
	out.push_str(&parts.join("\n"));
	out.push('\n');
	out
}

fn replace_between_lines_edit(
	ctx: &FileContext,
	start_line_zero_based: usize,
	end_line_zero_based_exclusive: usize,
	replacement: &str,
) -> Option<Edit> {
	replace_between_lines_edit_with_rule(
		ctx,
		start_line_zero_based,
		end_line_zero_based_exclusive,
		replacement,
		"RUST-STYLE-SPACE-003",
	)
}

fn replace_between_lines_edit_with_rule(
	ctx: &FileContext,
	start_line_zero_based: usize,
	end_line_zero_based_exclusive: usize,
	replacement: &str,
	rule: &'static str,
) -> Option<Edit> {
	let start = offset_from_line(&ctx.line_starts, start_line_zero_based + 1)?;
	let end = offset_from_line(&ctx.line_starts, end_line_zero_based_exclusive + 1)?;
	Some(Edit { start, end, replacement: replacement.to_owned(), rule })
}

pub(crate) fn check_vertical_spacing(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	let mut visited_blocks: HashSet<(usize, usize)> = HashSet::new();

	fn check_block(
		ctx: &FileContext,
		violations: &mut Vec<Violation>,
		edits: &mut Vec<Edit>,
		emit_edits: bool,
		visited_blocks: &mut HashSet<(usize, usize)>,
		start: usize,
		end: usize,
	) {
		if end <= start {
			return;
		}
		if !visited_blocks.insert((start, end)) {
			return;
		}

		let statements = extract_top_level_statements(&ctx.lines, start, end);
		if statements.is_empty() {
			return;
		}

		let (last_start, last_end, _) = statements[statements.len() - 1].clone();
		let final_is_return_or_tail =
			is_return_or_tail_statement(&ctx.lines[last_start..=last_end]);

		let mut return_like_indices: BTreeSet<usize> = BTreeSet::new();
		for (idx, (stmt_start, stmt_end, _)) in statements.iter().enumerate() {
			if is_explicit_return_statement(&ctx.lines[*stmt_start..=*stmt_end]) {
				return_like_indices.insert(idx);
			}
		}
		if final_is_return_or_tail {
			return_like_indices.insert(statements.len() - 1);
		}

		for idx in 0..statements.len().saturating_sub(1) {
			let (curr_start, curr_end, curr_type) = &statements[idx];
			let (next_start, next_end, next_type) = &statements[idx + 1];

			if return_like_indices.contains(&(idx + 1)) {
				continue;
			}

			let between = &ctx.lines[curr_end + 1..*next_start];
			let blank_count = between.iter().filter(|line| line.trim().is_empty()).count();
			let can_autofix_blank_only =
				between_is_blank_only(&ctx.lines, curr_end + 1, *next_start);
			let curr_is_item = is_item_like_statement(&ctx.lines[*curr_start..=*curr_end]);
			let next_is_item = is_item_like_statement(&ctx.lines[*next_start..=*next_end]);
			let curr_is_pipe_continuation =
				is_pipe_pattern_continuation_statement(&ctx.lines[*curr_start..=*curr_end]);
			let next_is_pipe_continuation =
				is_pipe_pattern_continuation_statement(&ctx.lines[*next_start..=*next_end]);
			let curr_is_const_group = is_const_group_statement(&ctx.lines[*curr_start..=*curr_end]);
			let next_is_const_group = is_const_group_statement(&ctx.lines[*next_start..=*next_end]);

			if curr_is_pipe_continuation || next_is_pipe_continuation {
				if blank_count != 0 {
					push_violation(
						violations,
						ctx,
						next_start + 1,
						"RUST-STYLE-SPACE-003",
						"Do not insert blank lines inside a match pattern alternation.",
						can_autofix_blank_only,
					);
					if emit_edits && can_autofix_blank_only {
						if let Some(edit) =
							replace_between_lines_edit(ctx, curr_end + 1, *next_start, "")
						{
							edits.push(edit);
						}
					}
				}
				continue;
			}

			if curr_is_const_group && next_is_const_group {
				let can_autofix =
					between_same_type_can_autofix(&ctx.lines, curr_end + 1, *next_start);
				if blank_count != 0 {
					push_violation(
						violations,
						ctx,
						next_start + 1,
						"RUST-STYLE-SPACE-003",
						"Do not insert blank lines within constant declaration groups.",
						can_autofix,
					);
					if emit_edits && can_autofix {
						let replacement = same_type_replacement_without_blank_lines(
							&ctx.lines,
							curr_end + 1,
							*next_start,
						);
						if let Some(edit) =
							replace_between_lines_edit(ctx, curr_end + 1, *next_start, &replacement)
						{
							edits.push(edit);
						}
					}
				}
				continue;
			}

			if curr_is_item && next_is_item {
				let can_autofix =
					between_same_type_can_autofix(&ctx.lines, curr_end + 1, *next_start);
				if blank_count != 1 {
					push_violation(
						violations,
						ctx,
						next_start + 1,
						"RUST-STYLE-SPACE-003",
						"Insert exactly one blank line between local item declarations.",
						can_autofix,
					);
					if emit_edits && can_autofix {
						let replacement = item_between_replacement_with_single_blank(
							&ctx.lines,
							curr_end + 1,
							*next_start,
						);
						if let Some(edit) =
							replace_between_lines_edit(ctx, curr_end + 1, *next_start, &replacement)
						{
							edits.push(edit);
						}
					}
				}
				continue;
			}

			if curr_type == next_type {
				let can_autofix =
					between_same_type_can_autofix(&ctx.lines, curr_end + 1, *next_start);
				if blank_count != 0 {
					push_violation(
						violations,
						ctx,
						next_start + 1,
						"RUST-STYLE-SPACE-003",
						"Do not insert blank lines within the same statement type.",
						can_autofix,
					);
					if emit_edits && can_autofix {
						let replacement = same_type_replacement_without_blank_lines(
							&ctx.lines,
							curr_end + 1,
							*next_start,
						);
						if let Some(edit) =
							replace_between_lines_edit(ctx, curr_end + 1, *next_start, &replacement)
						{
							edits.push(edit);
						}
					}
				}
			} else if blank_count != 1 {
				push_violation(
					violations,
					ctx,
					next_start + 1,
					"RUST-STYLE-SPACE-003",
					"Insert exactly one blank line between different statement types.",
					can_autofix_blank_only,
				);
				if emit_edits && can_autofix_blank_only {
					if let Some(edit) =
						replace_between_lines_edit(ctx, curr_end + 1, *next_start, "\n")
					{
						edits.push(edit);
					}
				}
			}
		}

		for idx in return_like_indices {
			if idx == 0 {
				continue;
			}
			let (_prev_start, prev_end, _) = &statements[idx - 1];
			let (ret_start, ret_end, _) = &statements[idx];
			let between = &ctx.lines[prev_end + 1..*ret_start];
			let blank_count = between.iter().filter(|line| line.trim().is_empty()).count();
			let can_autofix = between_is_blank_only(&ctx.lines, prev_end + 1, *ret_start);
			if blank_count != 1 {
				let stmt_lines = &ctx.lines[*ret_start..=*ret_end];
				let message = if is_explicit_return_statement(stmt_lines) {
					"Insert exactly one blank line before each return statement."
				} else {
					"Insert exactly one blank line before the final tail expression."
				};
				push_violation(
					violations,
					ctx,
					ret_start + 1,
					"RUST-STYLE-SPACE-004",
					message,
					can_autofix,
				);
				if emit_edits && can_autofix {
					if let Some(edit) = replace_between_lines_edit_with_rule(
						ctx,
						prev_end + 1,
						*ret_start,
						"\n",
						"RUST-STYLE-SPACE-004",
					) {
						edits.push(edit);
					}
				}
			}
		}

		for (stmt_start, stmt_end, _) in statements {
			for (child_start, child_end) in
				extract_top_level_brace_blocks_in_span(&ctx.lines, stmt_start, stmt_end)
			{
				if child_start == start && child_end == end {
					continue;
				}
				if is_data_like_brace_block(&ctx.lines, child_start, child_end) {
					continue;
				}
				check_block(
					ctx,
					violations,
					edits,
					emit_edits,
					visited_blocks,
					child_start,
					child_end,
				);
			}
		}
	}

	for (start, end) in quality::function_ranges(ctx) {
		check_block(ctx, violations, edits, emit_edits, &mut visited_blocks, start, end);
	}
}
