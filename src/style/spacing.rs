use std::{
	collections::{BTreeSet, HashSet},
	sync::LazyLock,
};

use regex::Regex;

use crate::style::{
	quality,
	shared::{self, Edit, FileContext, Violation},
};

type StatementSpan = (usize, usize, String);

static CONTROL_FLOW_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"^(if|if\s+let|match|for|while|loop|return|let)\b")
		.expect("Compile control-flow statement prefix regex.")
});
static STRUCT_FIELD_RE: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*\s*:\s*.+,?$")
		.expect("Compile struct field detection regex.")
});

#[derive(Clone, Copy, Debug, Default)]
struct CodeMaskState {
	in_block_comment_depth: usize,
	in_str: bool,
	str_escape: bool,
	in_char: bool,
	char_escape: bool,
	raw_hashes: Option<usize>,
}

struct StatementPair {
	blank_count: usize,
	can_autofix_blank_only: bool,
	curr_is_item: bool,
	next_is_item: bool,
	curr_is_pipe_continuation: bool,
	next_is_pipe_continuation: bool,
	curr_is_const_group: bool,
	next_is_const_group: bool,
}
impl StatementPair {
	fn from_statements(
		ctx: &FileContext,
		curr_start: usize,
		curr_end: usize,
		next_start: usize,
		next_end: usize,
	) -> Self {
		let between = &ctx.lines[curr_end + 1..next_start];

		Self {
			blank_count: between.iter().filter(|line| line.trim().is_empty()).count(),
			can_autofix_blank_only: between_is_blank_only(&ctx.lines, curr_end + 1, next_start),
			curr_is_item: is_item_like_statement(&ctx.lines[curr_start..=curr_end]),
			next_is_item: is_item_like_statement(&ctx.lines[next_start..=next_end]),
			curr_is_pipe_continuation: is_pipe_pattern_continuation_statement(
				&ctx.lines[curr_start..=curr_end],
			),
			next_is_pipe_continuation: is_pipe_pattern_continuation_statement(
				&ctx.lines[next_start..=next_end],
			),
			curr_is_const_group: is_const_group_statement(&ctx.lines[curr_start..=curr_end]),
			next_is_const_group: is_const_group_statement(&ctx.lines[next_start..=next_end]),
		}
	}
}

pub(crate) fn check_vertical_spacing(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	let mut visited_blocks: HashSet<(usize, usize)> = HashSet::new();

	for (start, end) in quality::function_ranges(ctx) {
		check_vertical_spacing_block(
			ctx,
			violations,
			edits,
			emit_edits,
			&mut visited_blocks,
			start,
			end,
		);
	}
}

fn check_vertical_spacing_block(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	visited_blocks: &mut HashSet<(usize, usize)>,
	start: usize,
	end: usize,
) {
	if end <= start || !visited_blocks.insert((start, end)) {
		return;
	}

	let statements = extract_top_level_statements(&ctx.lines, start, end);

	if statements.is_empty() {
		return;
	}

	let return_like_indices = collect_return_like_indices(ctx, &statements);

	check_statement_pair_spacing(
		ctx,
		violations,
		edits,
		emit_edits,
		&statements,
		&return_like_indices,
	);
	check_return_like_spacing(
		ctx,
		violations,
		edits,
		emit_edits,
		&statements,
		&return_like_indices,
	);
	recurse_spacing_child_blocks(
		ctx,
		violations,
		edits,
		emit_edits,
		visited_blocks,
		start,
		end,
		&statements,
	);
}

fn collect_return_like_indices(ctx: &FileContext, statements: &[StatementSpan]) -> BTreeSet<usize> {
	let mut out = BTreeSet::new();

	for (idx, (stmt_start, stmt_end, _)) in statements.iter().enumerate() {
		if is_explicit_return_statement(&ctx.lines[*stmt_start..=*stmt_end]) {
			out.insert(idx);
		}
	}

	let (last_start, last_end, _) = statements[statements.len() - 1].clone();
	let final_is_return_or_tail = is_return_or_tail_statement(&ctx.lines[last_start..=last_end]);

	if final_is_return_or_tail {
		out.insert(statements.len() - 1);
	}

	out
}

fn check_statement_pair_spacing(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	statements: &[StatementSpan],
	return_like_indices: &BTreeSet<usize>,
) {
	for idx in 0..statements.len().saturating_sub(1) {
		if return_like_indices.contains(&(idx + 1)) {
			continue;
		}

		let (curr_start, curr_end, curr_type) = &statements[idx];
		let (next_start, next_end, next_type) = &statements[idx + 1];
		let pair =
			StatementPair::from_statements(ctx, *curr_start, *curr_end, *next_start, *next_end);

		apply_statement_pair_spacing_rule(
			ctx,
			violations,
			edits,
			emit_edits,
			curr_end + 1,
			*next_start,
			curr_type,
			next_type,
			&pair,
		);
	}
}

#[allow(clippy::too_many_arguments)]
fn apply_statement_pair_spacing_rule(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	between_start: usize,
	next_start: usize,
	curr_type: &str,
	next_type: &str,
	pair: &StatementPair,
) {
	if pair.curr_is_pipe_continuation || pair.next_is_pipe_continuation {
		push_spacing_violation_and_edit(
			ctx,
			violations,
			edits,
			emit_edits,
			next_start + 1,
			"RUST-STYLE-SPACE-003",
			"Do not insert blank lines inside a match pattern alternation.",
			pair.blank_count != 0,
			pair.can_autofix_blank_only,
			between_start,
			next_start,
			"",
		);

		return;
	}
	if pair.curr_is_const_group && pair.next_is_const_group {
		let can_autofix = between_same_type_can_autofix(&ctx.lines, between_start, next_start);
		let replacement =
			same_type_replacement_without_blank_lines(&ctx.lines, between_start, next_start);

		push_spacing_violation_and_edit(
			ctx,
			violations,
			edits,
			emit_edits,
			next_start + 1,
			"RUST-STYLE-SPACE-003",
			"Do not insert blank lines within constant declaration groups.",
			pair.blank_count != 0,
			can_autofix,
			between_start,
			next_start,
			&replacement,
		);

		return;
	}
	if pair.curr_is_item && pair.next_is_item {
		let can_autofix = between_same_type_can_autofix(&ctx.lines, between_start, next_start);
		let replacement =
			item_between_replacement_with_single_blank(&ctx.lines, between_start, next_start);

		push_spacing_violation_and_edit(
			ctx,
			violations,
			edits,
			emit_edits,
			next_start + 1,
			"RUST-STYLE-SPACE-003",
			"Insert exactly one blank line between local item declarations.",
			pair.blank_count != 1,
			can_autofix,
			between_start,
			next_start,
			&replacement,
		);

		return;
	}
	if curr_type == next_type {
		let can_autofix = between_same_type_can_autofix(&ctx.lines, between_start, next_start);
		let replacement =
			same_type_replacement_without_blank_lines(&ctx.lines, between_start, next_start);

		push_spacing_violation_and_edit(
			ctx,
			violations,
			edits,
			emit_edits,
			next_start + 1,
			"RUST-STYLE-SPACE-003",
			"Do not insert blank lines within the same statement type.",
			pair.blank_count != 0,
			can_autofix,
			between_start,
			next_start,
			&replacement,
		);

		return;
	}

	push_spacing_violation_and_edit(
		ctx,
		violations,
		edits,
		emit_edits,
		next_start + 1,
		"RUST-STYLE-SPACE-003",
		"Insert exactly one blank line between different statement types.",
		pair.blank_count != 1,
		pair.can_autofix_blank_only,
		between_start,
		next_start,
		"\n",
	);
}

#[allow(clippy::too_many_arguments)]
fn push_spacing_violation_and_edit(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	line: usize,
	rule: &'static str,
	message: &'static str,
	should_report: bool,
	can_autofix: bool,
	start_line: usize,
	end_line: usize,
	replacement: &str,
) {
	if !should_report {
		return;
	}

	shared::push_violation(violations, ctx, line, rule, message, can_autofix);

	if emit_edits
		&& can_autofix
		&& let Some(edit) = replace_between_lines_edit(ctx, start_line, end_line, replacement)
	{
		edits.push(edit);
	}
}

fn check_return_like_spacing(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	statements: &[StatementSpan],
	return_like_indices: &BTreeSet<usize>,
) {
	for idx in return_like_indices {
		if *idx == 0 {
			continue;
		}

		let (_prev_start, prev_end, _) = &statements[idx - 1];
		let (ret_start, ret_end, _) = &statements[*idx];
		let between = &ctx.lines[prev_end + 1..*ret_start];
		let blank_count = between.iter().filter(|line| line.trim().is_empty()).count();
		let can_autofix = between_is_blank_only(&ctx.lines, prev_end + 1, *ret_start);

		if blank_count == 1 {
			continue;
		}

		let stmt_lines = &ctx.lines[*ret_start..=*ret_end];
		let message = if is_explicit_return_statement(stmt_lines) {
			"Insert exactly one blank line before each return statement."
		} else {
			"Insert exactly one blank line before the final tail expression."
		};

		shared::push_violation(
			violations,
			ctx,
			ret_start + 1,
			"RUST-STYLE-SPACE-004",
			message,
			can_autofix,
		);

		if emit_edits
			&& can_autofix
			&& let Some(edit) = replace_between_lines_edit_with_rule(
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

#[allow(clippy::too_many_arguments)]
fn recurse_spacing_child_blocks(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	visited_blocks: &mut HashSet<(usize, usize)>,
	start: usize,
	end: usize,
	statements: &[StatementSpan],
) {
	for (stmt_start, stmt_end, _) in statements {
		for (child_start, child_end) in
			extract_top_level_brace_blocks_in_span(&ctx.lines, *stmt_start, *stmt_end)
		{
			if child_start == start && child_end == end {
				continue;
			}
			if is_data_like_brace_block(&ctx.lines, child_start, child_end) {
				continue;
			}

			check_vertical_spacing_block(
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

fn normalize_statement_text(statement_lines: &[String]) -> String {
	let mut parts = Vec::new();
	let mut state = CodeMaskState::default();

	for raw in statement_lines {
		let mut code = mask_code_line(raw, &mut state);

		code = code.trim().to_owned();

		if code.is_empty() || code.starts_with('#') {
			continue;
		}

		parts.push(code);
	}

	parts.join(" ")
}

fn is_ident_char(ch: char) -> bool {
	ch.is_ascii_alphanumeric() || ch == '_'
}

fn is_lifetime_start(chars: &[char], idx: usize) -> bool {
	if idx + 1 >= chars.len() {
		return false;
	}

	let next = chars[idx + 1];

	if !(next.is_ascii_alphabetic() || next == '_') {
		return false;
	}
	if idx + 2 >= chars.len() {
		return true;
	}

	chars[idx + 2] != '\''
}

fn raw_string_start(chars: &[char], idx: usize) -> Option<(usize, usize)> {
	if idx >= chars.len() {
		return None;
	}
	if idx > 0 && is_ident_char(chars[idx - 1]) {
		return None;
	}

	let mut cursor = idx;

	if chars[cursor] == 'b' {
		if cursor + 1 >= chars.len() || chars[cursor + 1] != 'r' {
			return None;
		}

		cursor += 1;
	}
	if chars[cursor] != 'r' {
		return None;
	}

	cursor += 1;

	let mut hash_count = 0_usize;

	while cursor < chars.len() && chars[cursor] == '#' {
		hash_count += 1;
		cursor += 1;
	}

	if cursor >= chars.len() || chars[cursor] != '"' {
		return None;
	}

	Some((cursor - idx + 1, hash_count))
}

fn mask_code_line(line: &str, state: &mut CodeMaskState) -> String {
	let chars = line.chars().collect::<Vec<_>>();
	let mut out = String::with_capacity(line.len());
	let mut idx = 0_usize;

	while idx < chars.len() {
		if consume_masked_block_comment(&chars, &mut idx, &mut out, state) {
			continue;
		}
		if consume_masked_raw_string(&chars, &mut idx, &mut out, state) {
			continue;
		}
		if consume_masked_normal_string(&chars, &mut idx, &mut out, state) {
			continue;
		}
		if consume_masked_char_literal(&chars, &mut idx, &mut out, state) {
			continue;
		}
		if starts_line_comment(&chars, idx) {
			break;
		}
		if consume_block_comment_start(&chars, &mut idx, &mut out, state) {
			continue;
		}
		if consume_raw_string_start(&chars, &mut idx, &mut out, state) {
			continue;
		}
		if consume_string_or_char_start(&chars, &mut idx, &mut out, state) {
			continue;
		}

		out.push(chars[idx]);

		idx += 1;
	}

	out
}

fn consume_masked_block_comment(
	chars: &[char],
	idx: &mut usize,
	out: &mut String,
	state: &mut CodeMaskState,
) -> bool {
	if state.in_block_comment_depth == 0 {
		return false;
	}

	let ch = chars[*idx];
	let next = chars.get(*idx + 1).copied();

	if ch == '/' && next == Some('*') {
		state.in_block_comment_depth += 1;

		out.push_str("  ");

		*idx += 2;

		return true;
	}
	if ch == '*' && next == Some('/') {
		state.in_block_comment_depth = state.in_block_comment_depth.saturating_sub(1);

		out.push_str("  ");

		*idx += 2;

		return true;
	}

	out.push(' ');

	*idx += 1;

	true
}

fn consume_masked_raw_string(
	chars: &[char],
	idx: &mut usize,
	out: &mut String,
	state: &mut CodeMaskState,
) -> bool {
	let Some(hash_count) = state.raw_hashes else {
		return false;
	};

	if chars[*idx] == '"' && raw_hash_suffix_matches(chars, *idx, hash_count) {
		out.push(' ');

		for _ in 0..hash_count {
			out.push(' ');
		}

		*idx += 1 + hash_count;
		state.raw_hashes = None;

		return true;
	}

	out.push(' ');

	*idx += 1;

	true
}

fn raw_hash_suffix_matches(chars: &[char], idx: usize, hash_count: usize) -> bool {
	(0..hash_count).all(|offset| {
		let pos = idx + 1 + offset;

		pos < chars.len() && chars[pos] == '#'
	})
}

fn consume_masked_normal_string(
	chars: &[char],
	idx: &mut usize,
	out: &mut String,
	state: &mut CodeMaskState,
) -> bool {
	if !state.in_str {
		return false;
	}

	let ch = chars[*idx];

	out.push(' ');

	if state.str_escape {
		state.str_escape = false;
	} else if ch == '\\' {
		state.str_escape = true;
	} else if ch == '"' {
		state.in_str = false;
	}

	*idx += 1;

	true
}

fn consume_masked_char_literal(
	chars: &[char],
	idx: &mut usize,
	out: &mut String,
	state: &mut CodeMaskState,
) -> bool {
	if !state.in_char {
		return false;
	}

	let ch = chars[*idx];

	out.push(' ');

	if state.char_escape {
		state.char_escape = false;
	} else if ch == '\\' {
		state.char_escape = true;
	} else if ch == '\'' {
		state.in_char = false;
	}

	*idx += 1;

	true
}

fn starts_line_comment(chars: &[char], idx: usize) -> bool {
	chars[idx] == '/' && chars.get(idx + 1).copied() == Some('/')
}

fn consume_block_comment_start(
	chars: &[char],
	idx: &mut usize,
	out: &mut String,
	state: &mut CodeMaskState,
) -> bool {
	if chars[*idx] != '/' || chars.get(*idx + 1).copied() != Some('*') {
		return false;
	}

	state.in_block_comment_depth += 1;

	out.push_str("  ");

	*idx += 2;

	true
}

fn consume_raw_string_start(
	chars: &[char],
	idx: &mut usize,
	out: &mut String,
	state: &mut CodeMaskState,
) -> bool {
	let Some((prefix_len, hash_count)) = raw_string_start(chars, *idx) else {
		return false;
	};

	for _ in 0..prefix_len {
		out.push(' ');
	}

	*idx += prefix_len;
	state.raw_hashes = Some(hash_count);

	true
}

fn consume_string_or_char_start(
	chars: &[char],
	idx: &mut usize,
	out: &mut String,
	state: &mut CodeMaskState,
) -> bool {
	let ch = chars[*idx];

	if ch == '"' {
		state.in_str = true;
		state.str_escape = false;

		out.push(' ');

		*idx += 1;

		return true;
	}
	if ch == '\'' && !is_lifetime_start(chars, *idx) {
		state.in_char = true;
		state.char_escape = false;

		out.push(' ');

		*idx += 1;

		return true;
	}

	false
}

fn strip_turbofish(text: &str) -> String {
	let chars = text.chars().collect::<Vec<_>>();
	let mut out = String::with_capacity(text.len());
	let mut idx = 0;

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

	let fn_match = Regex::new(r"^(?P<func>[A-Za-z_][A-Za-z0-9_]*)\s*\(")
		.expect("Compile UFCS function extraction regex.")
		.captures(rest)?;
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

	if Regex::new(r"^let\b").expect("Compile let-statement classification regex.").is_match(first) {
		return "let".to_owned();
	}
	if Regex::new(r"^if\s+let\b")
		.expect("Compile if-let statement classification regex.")
		.is_match(first)
	{
		return "if-let".to_owned();
	}
	if Regex::new(r"^if\b").expect("Compile if-statement classification regex.").is_match(first) {
		return "if".to_owned();
	}
	if Regex::new(r"^match\b")
		.expect("Compile match-statement classification regex.")
		.is_match(first)
	{
		return "match".to_owned();
	}
	if Regex::new(r"^for\b").expect("Compile for-loop classification regex.").is_match(first) {
		return "for".to_owned();
	}
	if Regex::new(r"^while\b").expect("Compile while-loop classification regex.").is_match(first) {
		return "while".to_owned();
	}
	if Regex::new(r"^loop\b").expect("Compile loop classification regex.").is_match(first) {
		return "loop".to_owned();
	}
	if Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*(?:\.await)?\?\s*;?$")
		.expect("Compile try-expression classification regex.")
		.is_match(first)
	{
		return "try-expr".to_owned();
	}
	if Regex::new(r"^(?P<name>[A-Za-z_][A-Za-z0-9_:]*)!\s*\(")
		.expect("Compile macro invocation classification regex.")
		.is_match(first)
	{
		let macro_name = Regex::new(r"^(?P<name>[A-Za-z_][A-Za-z0-9_:]*)!\s*\(")
			.expect("Compile macro name extraction regex.")
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
		.expect("Compile qualified path call classification regex.")
		.is_match(first)
	{
		return "path-call".to_owned();
	}
	if Regex::new(r"^(?P<target>[A-Za-z_][A-Za-z0-9_]*)\s*\(")
		.expect("Compile direct function call classification regex.")
		.is_match(first)
	{
		return "call".to_owned();
	}
	if Regex::new(r"^[^;]*\.(?P<method>[A-Za-z_][A-Za-z0-9_]*)\s*\(")
		.expect("Compile method call classification regex.")
		.is_match(first)
	{
		return "method".to_owned();
	}

	let token = Regex::new(r"[\s({;]")
		.expect("Compile statement token split regex.")
		.split(first)
		.next()
		.unwrap_or_default();

	if token.is_empty() { "other".to_owned() } else { format!("shape:{token}") }
}

fn extract_top_level_statements(
	lines: &[String],
	fn_start: usize,
	fn_end: usize,
) -> Vec<(usize, usize, String)> {
	fn next_significant_line_starts_with_dot(
		lines: &[String],
		mut mask_state: CodeMaskState,
		from_idx: usize,
		fn_end: usize,
	) -> bool {
		for line in lines.iter().take(fn_end).skip(from_idx + 1) {
			let code = mask_code_line(line, &mut mask_state);
			let stripped = code.trim();

			if stripped.is_empty() {
				continue;
			}
			// Attributes apply to the next statement. Treat them as a hard boundary.
			if stripped.starts_with('#') {
				return false;
			}

			return stripped.starts_with('.');
		}

		false
	}

	let mut statements = Vec::new();
	let mut brace_depth = 1_i32;
	let mut paren_depth = 0_i32;
	let mut bracket_depth = 0_i32;
	let mut current_start: Option<usize> = None;
	let mut mask_state = CodeMaskState::default();

	for idx in (fn_start + 1)..fn_end {
		let raw_line = &lines[idx];
		let code = mask_code_line(raw_line, &mut mask_state);
		let stripped = code.trim();

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
			&& (stripped_code.ends_with(';')
				|| (stripped_code.ends_with('}')
					&& !next_significant_line_starts_with_dot(lines, mask_state, idx, fn_end)));

		if statement_closed {
			let span_lines = lines[current_start_value..=idx].to_vec();

			statements.push((current_start_value, idx, classify_statement_type(&span_lines)));

			current_start = None;
		}
	}

	if let Some(current_start) = current_start
		&& fn_end > current_start
	{
		let span_lines = lines[current_start..fn_end].to_vec();

		statements.push((
			current_start,
			fn_end.saturating_sub(1),
			classify_statement_type(&span_lines),
		));
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

	if Regex::new(r"^return\b").expect("Compile return statement detection regex.").is_match(&first)
	{
		return true;
	}

	let Some(last) = last_significant_statement_line(statement_lines) else {
		return false;
	};

	if Regex::new(r"^return\b")
		.expect("Compile trailing return statement detection regex.")
		.is_match(&last)
	{
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
		.map(|first| {
			Regex::new(r"^return\b")
				.expect("Compile explicit return statement detection regex.")
				.is_match(&first)
		})
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
	let mut mask_state = CodeMaskState::default();

	for (idx, line) in lines.iter().enumerate().take(span_end + 1).skip(span_start) {
		let code = mask_code_line(line, &mut mask_state);

		for ch in code.chars() {
			if ch == '{' {
				depth += 1;

				if depth == 1 {
					current_start = Some(idx);
				}
			} else if ch == '}' {
				if depth == 1
					&& let Some(start) = current_start
				{
					blocks.push((start, idx));

					current_start = None;
				}

				depth = (depth - 1).max(0);
			}
		}
	}

	blocks
}

fn is_data_like_brace_block(lines: &[String], block_start: usize, block_end: usize) -> bool {
	let mut content = Vec::new();
	let mut mask_state = CodeMaskState::default();

	for line in lines.iter().take(block_end).skip(block_start + 1) {
		let code = mask_code_line(line, &mut mask_state);
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
		if CONTROL_FLOW_PREFIX_RE.is_match(line) {
			return false;
		}
	}
	for line in &content {
		if STRUCT_FIELD_RE.is_match(line) {
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
	.expect("Compile item-like statement detection regex.")
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
		.expect("Compile const/static statement detection regex.")
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
	let start = shared::offset_from_line(&ctx.line_starts, start_line_zero_based + 1)?;
	let end = shared::offset_from_line(&ctx.line_starts, end_line_zero_based_exclusive + 1)?;

	Some(Edit { start, end, replacement: replacement.to_owned(), rule })
}
