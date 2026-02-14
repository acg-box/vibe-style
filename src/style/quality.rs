use std::path::Path;

use ra_ap_syntax::{
	AstNode,
	ast::{
		self, ArgList, Attr, Expr, HasArgList, HasAttrs, HasModuleItem, HasName, MacroCall,
		MethodCallExpr,
	},
};
use regex::Regex;

use crate::style::shared::{self, Edit, FileContext, SNAKE_CASE_RE, Violation};

const NUMERIC_SUFFIXES: [&str; 14] = [
	"usize", "isize", "u128", "i128", "u64", "i64", "u32", "i32", "u16", "i16", "u8", "i8", "f64",
	"f32",
];

#[derive(Debug, Default)]
struct ArgSplitState {
	paren: i32,
	brace: i32,
	bracket: i32,
	in_str: bool,
	escape: bool,
	in_char: bool,
	char_escape: bool,
	in_line_comment: bool,
	block_comment_depth: i32,
}

pub(crate) fn check_logging_quality(ctx: &FileContext, violations: &mut Vec<Violation>) {
	for macro_call in ctx.source_file.syntax().descendants().filter_map(MacroCall::cast) {
		let Some(path_text) = macro_path_text(&macro_call) else {
			continue;
		};
		let normalized = path_text.replace(' ', "");

		if !matches!(
			normalized.as_str(),
			"tracing::trace"
				| "tracing::debug"
				| "tracing::info"
				| "tracing::warn"
				| "tracing::error"
		) {
			continue;
		}

		let Some(tt) = macro_call.token_tree() else {
			continue;
		};
		let tt_text = tt.syntax().text().to_string();

		if tt_text.len() < 2 {
			continue;
		}

		let args = tt_text[1..tt_text.len() - 1].to_owned();
		let parts = split_top_level_args(&args);

		if parts.is_empty() {
			continue;
		}

		let message = parse_string_literal(parts.last().map(String::as_str).unwrap_or_default());
		let head_parts = if message.is_some() {
			parts[..parts.len().saturating_sub(1)].to_vec()
		} else {
			parts.clone()
		};
		let head_text = head_parts.join(", ");
		let line = shared::line_from_offset(
			&ctx.line_starts,
			usize::from(macro_call.syntax().text_range().start()),
		);

		if let Some(message) = message {
			if message.contains('{') || message.contains('}') {
				shared::push_violation(
					violations,
					ctx,
					line,
					"RUST-STYLE-LOG-002",
					"Do not interpolate dynamic values in log message strings; use structured fields.",
					false,
				);
			}
			if !is_sentence(&message) {
				shared::push_violation(
					violations,
					ctx,
					line,
					"RUST-STYLE-LOG-002",
					"Log messages should be complete sentences with capitalization and punctuation.",
					false,
				);
			}
		}

		if parts.len() > 1 && !has_structured_fields(&head_text) {
			shared::push_violation(
				violations,
				ctx,
				line,
				"RUST-STYLE-LOG-002",
				"Prefer structured logging fields for dynamic context values.",
				false,
			);
		}
	}
}

pub(crate) fn check_expect_unwrap(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	if is_test_file(&ctx.path) {
		return;
	}

	for method_call in ctx.source_file.syntax().descendants().filter_map(MethodCallExpr::cast) {
		if method_call_in_test_context(&method_call) {
			continue;
		}

		let Some(name) = method_call.name_ref().map(|name| name.text().to_string()) else {
			continue;
		};
		let line = method_call_line(ctx, &method_call);

		match name.as_str() {
			"unwrap" => handle_unwrap_call(ctx, violations, edits, emit_edits, &method_call, line),
			"expect" => handle_expect_call(ctx, violations, edits, emit_edits, &method_call, line),
			_ => {},
		}
	}
}

pub(crate) fn check_numeric_literals(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	for literal in ctx.source_file.syntax().descendants().filter_map(ast::Literal::cast) {
		let literal_text = literal.syntax().text().to_string();

		if literal_text.is_empty() || !literal_text.as_bytes()[0].is_ascii_digit() {
			continue;
		}

		let range = literal.syntax().text_range();
		let start = usize::from(range.start());
		let line = shared::line_from_offset(&ctx.line_starts, start);

		if let Some(suffix_start) = numeric_suffix_start(&literal_text) {
			let body = &literal_text[..suffix_start];

			if is_decimal_body(body) && !body.ends_with('_') {
				shared::push_violation(
					violations,
					ctx,
					line,
					"RUST-STYLE-NUM-001",
					"Numeric suffixes must be separated by an underscore (for example 10_f32).",
					true,
				);

				if emit_edits {
					edits.push(Edit {
						start: start + suffix_start,
						end: start + suffix_start,
						replacement: "_".to_owned(),
						rule: "RUST-STYLE-NUM-001",
					});
				}
			}
		}

		let Some(int_end) = decimal_integer_part_end(&literal_text) else {
			continue;
		};
		let int_part = &literal_text[..int_end];
		let digits_only = int_part.chars().all(|ch| ch.is_ascii_digit());

		if !digits_only {
			continue;
		}
		if int_part.len() < 4 || int_part.starts_with('0') {
			continue;
		}

		shared::push_violation(
			violations,
			ctx,
			line,
			"RUST-STYLE-NUM-002",
			"Integers with more than three digits must use underscore separators.",
			true,
		);

		if emit_edits {
			edits.push(Edit {
				start,
				end: start + int_end,
				replacement: add_numeric_grouping(int_part),
				rule: "RUST-STYLE-NUM-002",
			});
		}
	}
}

pub(crate) fn function_ranges(ctx: &FileContext) -> Vec<(usize, usize)> {
	let mut ranges = Vec::new();

	for function in ctx.source_file.syntax().descendants().filter_map(ast::Fn::cast) {
		let Some(body) = function.body() else {
			continue;
		};
		let (start_line, end_line) =
			shared::text_range_to_lines(&ctx.line_starts, body.syntax().text_range());

		ranges.push((start_line.saturating_sub(1), end_line.saturating_sub(1)));
	}

	ranges
}

pub(crate) fn check_function_length(ctx: &FileContext, violations: &mut Vec<Violation>) {
	for (start, end) in function_ranges(ctx) {
		if end < start {
			continue;
		}

		let length = end - start + 1;

		if length > 120 {
			shared::push_violation(
				violations,
				ctx,
				start + 1,
				"RUST-STYLE-READ-002",
				&format!("Function body has {length} lines; keep functions at or under 120 lines."),
				false,
			);
		}
	}
}

pub(crate) fn check_test_rules(ctx: &FileContext, violations: &mut Vec<Violation>) {
	for function in ctx.source_file.syntax().descendants().filter_map(ast::Fn::cast) {
		let is_test = function
			.attrs()
			.any(|attr| attr.as_simple_atom().map(|atom| atom.as_str() == "test").unwrap_or(false));

		if !is_test {
			continue;
		}

		let name = function.name().map(|name| name.text().to_string()).unwrap_or_default();

		if !SNAKE_CASE_RE.is_match(&name) || !name.contains('_') {
			let line = shared::line_from_offset(
				&ctx.line_starts,
				usize::from(function.syntax().text_range().start()),
			);

			shared::push_violation(
				violations,
				ctx,
				line,
				"RUST-STYLE-TEST-001",
				"Test function names should be descriptive snake_case.",
				false,
			);
		}
	}
	for item in ctx.source_file.items() {
		let ast::Item::Module(module) = item else {
			continue;
		};
		let Some(name) = module.name().map(|name| name.text().to_string()) else {
			continue;
		};

		if name != "_test" {
			continue;
		}
		if !module
			.attrs()
			.any(|attr| attr.syntax().text().to_string().replace(' ', "").contains("cfg(test)"))
		{
			continue;
		}

		let contains_behavior_tests = module.item_list().is_some_and(|list| {
			list.items().any(|item| {
				if let ast::Item::Fn(function) = item {
					function.attrs().any(|attr| {
						attr.as_simple_atom().map(|atom| atom.as_str() == "test").unwrap_or(false)
					})
				} else {
					false
				}
			})
		});

		if contains_behavior_tests {
			shared::push_violation(
				violations,
				ctx,
				1,
				"RUST-STYLE-TEST-002",
				"`#[cfg(test)] mod _test` is reserved for keep-alive imports and must not contain behavior tests.",
				false,
			);
		}
	}
}

fn method_call_line(ctx: &FileContext, method_call: &MethodCallExpr) -> usize {
	shared::line_from_offset(
		&ctx.line_starts,
		usize::from(method_call.syntax().text_range().start()),
	)
}

fn handle_unwrap_call(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	method_call: &MethodCallExpr,
	line: usize,
) {
	shared::push_violation(
		violations,
		ctx,
		line,
		"RUST-STYLE-RUNTIME-001",
		"Do not use unwrap() in non-test code.",
		true,
	);

	if !emit_edits {
		return;
	}

	let name_range = method_call.name_ref().map(|name_ref| name_ref.syntax().text_range());
	let arg_range = method_call.arg_list().map(|arg_list| arg_list.syntax().text_range());

	if let (Some(name_range), Some(arg_range)) = (name_range, arg_range) {
		edits.push(Edit {
			start: usize::from(name_range.start()),
			end: usize::from(arg_range.end()),
			replacement: r#"expect("Expected operation to succeed.")"#.to_owned(),
			rule: "RUST-STYLE-RUNTIME-001",
		});
	}
}

fn handle_expect_call(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	method_call: &MethodCallExpr,
	line: usize,
) {
	let Some(arg_list) = method_call.arg_list() else {
		report_expect_missing_arg_list(ctx, violations, edits, emit_edits, method_call, line);

		return;
	};
	let mut args = arg_list.args();
	let Some(first_arg) = args.next() else {
		report_expect_empty_arg_list(ctx, violations, edits, emit_edits, &arg_list, line);

		return;
	};
	let literal_message = first_arg
		.syntax()
		.descendants()
		.filter_map(ast::Literal::cast)
		.next()
		.and_then(|lit| parse_string_literal(&lit.syntax().text().to_string()));
	let Some(message) = literal_message else {
		shared::push_violation(
			violations,
			ctx,
			line,
			"RUST-STYLE-RUNTIME-002",
			"expect() must use a clear, user-actionable string literal message.",
			false,
		);

		return;
	};
	let message = message.trim().to_owned();

	if message.is_empty() {
		report_expect_empty_message(ctx, violations, edits, emit_edits, &first_arg, line);

		return;
	}

	let first = message.chars().next().unwrap_or('a');
	let last = message.chars().last().unwrap_or('.');

	if !first.is_uppercase() || !matches!(last, '.' | '!' | '?') {
		report_expect_non_sentence_message(
			ctx, violations, edits, emit_edits, &first_arg, &message, line,
		);
	}
}

fn report_expect_missing_arg_list(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	method_call: &MethodCallExpr,
	line: usize,
) {
	shared::push_violation(
		violations,
		ctx,
		line,
		"RUST-STYLE-RUNTIME-002",
		"expect() must use a clear, user-actionable string literal message.",
		true,
	);

	if !emit_edits {
		return;
	}

	let name_range = method_call.name_ref().map(|name_ref| name_ref.syntax().text_range());

	if let Some(name_range) = name_range {
		edits.push(Edit {
			start: usize::from(name_range.start()),
			end: usize::from(method_call.syntax().text_range().end()),
			replacement: r#"expect("Expected operation to succeed.")"#.to_owned(),
			rule: "RUST-STYLE-RUNTIME-002",
		});
	}
}

fn report_expect_empty_arg_list(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	arg_list: &ArgList,
	line: usize,
) {
	shared::push_violation(
		violations,
		ctx,
		line,
		"RUST-STYLE-RUNTIME-002",
		"expect() message must not be empty.",
		true,
	);

	if emit_edits {
		edits.push(Edit {
			start: usize::from(arg_list.syntax().text_range().start()),
			end: usize::from(arg_list.syntax().text_range().end()),
			replacement: r#"("Expected operation to succeed.")"#.to_owned(),
			rule: "RUST-STYLE-RUNTIME-002",
		});
	}
}

fn report_expect_empty_message(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	first_arg: &Expr,
	line: usize,
) {
	shared::push_violation(
		violations,
		ctx,
		line,
		"RUST-STYLE-RUNTIME-002",
		"expect() message must not be empty.",
		true,
	);

	if emit_edits {
		edits.push(Edit {
			start: usize::from(first_arg.syntax().text_range().start()),
			end: usize::from(first_arg.syntax().text_range().end()),
			replacement: r#""Expected operation to succeed.""#.to_owned(),
			rule: "RUST-STYLE-RUNTIME-002",
		});
	}
}

fn report_expect_non_sentence_message(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	first_arg: &Expr,
	message: &str,
	line: usize,
) {
	shared::push_violation(
		violations,
		ctx,
		line,
		"RUST-STYLE-RUNTIME-002",
		"expect() message should start with a capital letter and end with punctuation.",
		true,
	);

	if emit_edits {
		let normalized = normalize_expect_message(message);

		edits.push(Edit {
			start: usize::from(first_arg.syntax().text_range().start()),
			end: usize::from(first_arg.syntax().text_range().end()),
			replacement: format!("{normalized:?}"),
			rule: "RUST-STYLE-RUNTIME-002",
		});
	}
}

fn split_top_level_args(args: &str) -> Vec<String> {
	let mut parts = Vec::new();
	let mut start = 0_usize;
	let mut state = ArgSplitState::default();
	let chars = args.char_indices().collect::<Vec<_>>();
	let mut idx = 0_usize;

	while idx < chars.len() {
		let (offset, ch) = chars[idx];
		let next = if idx + 1 < chars.len() { Some(chars[idx + 1].1) } else { None };

		if let Some(step) = consume_split_context(&mut state, ch, next) {
			idx += step;

			continue;
		}
		if let Some(step) = enter_split_context(&mut state, ch, next) {
			idx += step;

			continue;
		}

		match ch {
			'(' => state.paren += 1,
			')' => state.paren = (state.paren - 1).max(0),
			'{' => state.brace += 1,
			'}' => state.brace = (state.brace - 1).max(0),
			'[' => state.bracket += 1,
			']' => state.bracket = (state.bracket - 1).max(0),
			',' if state.paren == 0 && state.brace == 0 && state.bracket == 0 => {
				let segment = args[start..offset].trim();

				if !segment.is_empty() {
					parts.push(segment.to_owned());
				}

				start = offset + 1;
			},
			_ => {},
		}

		idx += 1;
	}

	let tail = args[start..].trim();

	if !tail.is_empty() {
		parts.push(tail.to_owned());
	}

	parts
}

fn consume_split_context(state: &mut ArgSplitState, ch: char, next: Option<char>) -> Option<usize> {
	if state.in_line_comment {
		if ch == '\n' {
			state.in_line_comment = false;
		}

		return Some(1);
	}
	if state.block_comment_depth > 0 {
		if ch == '/' && next == Some('*') {
			state.block_comment_depth += 1;

			return Some(2);
		}
		if ch == '*' && next == Some('/') {
			state.block_comment_depth -= 1;

			return Some(2);
		}

		return Some(1);
	}
	if state.in_str {
		if state.escape {
			state.escape = false;
		} else if ch == '\\' {
			state.escape = true;
		} else if ch == '"' {
			state.in_str = false;
		}

		return Some(1);
	}
	if state.in_char {
		if state.char_escape {
			state.char_escape = false;
		} else if ch == '\\' {
			state.char_escape = true;
		} else if ch == '\'' {
			state.in_char = false;
		}

		return Some(1);
	}

	None
}

fn enter_split_context(state: &mut ArgSplitState, ch: char, next: Option<char>) -> Option<usize> {
	if ch == '/' && next == Some('/') {
		state.in_line_comment = true;

		return Some(2);
	}
	if ch == '/' && next == Some('*') {
		state.block_comment_depth += 1;

		return Some(2);
	}
	if ch == '"' {
		state.in_str = true;
		state.escape = false;

		return Some(1);
	}
	if ch == '\'' {
		state.in_char = true;
		state.char_escape = false;

		return Some(1);
	}

	None
}

fn parse_string_literal(text: &str) -> Option<String> {
	let stripped = text.trim();

	if stripped.len() >= 2 && stripped.starts_with('"') && stripped.ends_with('"') {
		return Some(stripped[1..stripped.len() - 1].to_owned());
	}
	if !stripped.starts_with('r') {
		return None;
	}

	let bytes = stripped.as_bytes();
	let mut quote_idx = 1_usize;

	while quote_idx < bytes.len() && bytes[quote_idx] == b'#' {
		quote_idx += 1;
	}

	if quote_idx >= bytes.len() || bytes[quote_idx] != b'"' {
		return None;
	}

	let hash_count = quote_idx.saturating_sub(1);
	let body_start = quote_idx + 1;
	let suffix = format!("\"{}", "#".repeat(hash_count));

	if !stripped.ends_with(&suffix) {
		return None;
	}

	let body_end = stripped.len().saturating_sub(suffix.len());

	if body_end < body_start {
		return None;
	}

	Some(stripped[body_start..body_end].to_owned())
}

fn is_sentence(text: &str) -> bool {
	let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");

	if normalized.is_empty() {
		return false;
	}

	let first = normalized.chars().next().unwrap_or('a');
	let last = normalized.chars().last().unwrap_or('.');

	first.is_uppercase() && matches!(last, '.' | '!' | '?')
}

fn has_structured_fields(text: &str) -> bool {
	Regex::new(r"\b[A-Za-z_][A-Za-z0-9_]*\s*=")
		.expect("Expected operation to succeed.")
		.is_match(text)
		|| Regex::new(r"[%?]\s*[A-Za-z_][A-Za-z0-9_:]*")
			.expect("Expected operation to succeed.")
			.is_match(text)
}

fn macro_path_text(macro_call: &MacroCall) -> Option<String> {
	macro_call.path().map(|path| path.syntax().text().to_string())
}

fn is_test_file(path: &Path) -> bool {
	let text = path.to_string_lossy().replace('\\', "/");

	text.contains("/tests/") || text.ends_with("_test.rs")
}

fn has_attr_text(mut attrs: impl Iterator<Item = Attr>, needle: &str) -> bool {
	let compact = needle.replace(' ', "");

	attrs.any(|attr| attr.syntax().text().to_string().replace(' ', "").contains(&compact))
}

fn method_call_in_test_context(call: &MethodCallExpr) -> bool {
	for node in call.syntax().ancestors() {
		if let Some(module) = ast::Module::cast(node.clone())
			&& has_attr_text(module.attrs(), "cfg(test)")
		{
			return true;
		}
		if let Some(function) = ast::Fn::cast(node)
			&& has_attr_text(function.attrs(), "test")
		{
			return true;
		}
	}

	false
}

fn normalize_expect_message(message: &str) -> String {
	let mut normalized = message.trim().to_owned();

	if normalized.is_empty() {
		return "Expected operation to succeed.".to_owned();
	}

	let mut chars = normalized.chars();
	let first = chars.next().unwrap_or('E');

	if !first.is_uppercase() {
		let mut rewritten = first.to_uppercase().collect::<String>();

		rewritten.push_str(chars.as_str());

		normalized = rewritten;
	}
	if !matches!(normalized.chars().last().unwrap_or('.'), '.' | '!' | '?') {
		normalized.push('.');
	}

	normalized
}

fn add_numeric_grouping(number: &str) -> String {
	let mut rev = String::new();

	for (idx, ch) in number.chars().rev().enumerate() {
		if idx > 0 && idx % 3 == 0 {
			rev.push('_');
		}

		rev.push(ch);
	}

	rev.chars().rev().collect()
}

fn numeric_suffix_start(literal: &str) -> Option<usize> {
	for suffix in NUMERIC_SUFFIXES {
		if literal.ends_with(suffix) {
			return Some(literal.len().saturating_sub(suffix.len()));
		}
	}

	None
}

fn is_decimal_body(body: &str) -> bool {
	if body.is_empty() || body.starts_with('.') || body.ends_with('.') || body.contains("..") {
		return false;
	}

	let dot_count = body.bytes().filter(|byte| *byte == b'.').count();

	if dot_count > 1 {
		return false;
	}

	body.bytes().all(|byte| byte.is_ascii_digit() || byte == b'_' || byte == b'.')
}

fn decimal_integer_part_end(literal: &str) -> Option<usize> {
	let mut end = 0_usize;

	for (idx, ch) in literal.char_indices() {
		if ch.is_ascii_digit() || ch == '_' {
			end = idx + ch.len_utf8();
		} else {
			break;
		}
	}

	if end == 0 { None } else { Some(end) }
}
