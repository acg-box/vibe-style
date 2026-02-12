use std::path::Path;

use ra_ap_syntax::{
	AstNode,
	ast::{self, HasArgList, HasAttrs, HasModuleItem, HasName},
};
use regex::Regex;

use super::shared::{Edit, FileContext, SNAKE_CASE_RE, Violation};

const NUMERIC_SUFFIXES: [&str; 14] = [
	"usize", "isize", "u128", "i128", "u64", "i64", "u32", "i32", "u16", "i16", "u8", "i8", "f64",
	"f32",
];

pub(crate) fn check_std_macro_calls(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	const STD_MACROS: [&str; 7] =
		["vec", "format", "println", "eprintln", "dbg", "write", "writeln"];

	for macro_call in ctx.source_file.syntax().descendants().filter_map(ast::MacroCall::cast) {
		let Some(path) = macro_call.path() else {
			continue;
		};
		let path_text = path.syntax().text().to_string();
		let Some((prefix_start, prefix_end, macro_name)) = parse_std_macro_prefix(&path_text)
		else {
			continue;
		};

		if !STD_MACROS.contains(&macro_name.as_str()) {
			continue;
		}

		let range = path.syntax().text_range();
		let absolute_path_start = usize::from(range.start());
		let line = super::shared::line_from_offset(&ctx.line_starts, absolute_path_start);

		super::shared::push_violation(
			violations,
			ctx,
			line,
			"RUST-STYLE-IMPORT-006",
			"Do not qualify standard macros with std::.",
			true,
		);

		if emit_edits {
			edits.push(Edit {
				start: absolute_path_start + prefix_start,
				end: absolute_path_start + prefix_end,
				replacement: String::new(),
				rule: "RUST-STYLE-IMPORT-006",
			});
		}
	}
}

fn parse_std_macro_prefix(path_text: &str) -> Option<(usize, usize, String)> {
	fn skip_ws(text: &[u8], mut idx: usize) -> usize {
		while idx < text.len() && text[idx].is_ascii_whitespace() {
			idx += 1;
		}
		idx
	}

	fn consume_double_colon(text: &[u8], mut idx: usize) -> Option<usize> {
		idx = skip_ws(text, idx);
		if idx >= text.len() || text[idx] != b':' {
			return None;
		}

		idx += 1;
		idx = skip_ws(text, idx);

		if idx >= text.len() || text[idx] != b':' {
			return None;
		}

		Some(idx + 1)
	}

	fn consume_ident(text: &[u8], mut idx: usize) -> Option<(usize, String)> {
		idx = skip_ws(text, idx);
		let start = idx;

		if idx >= text.len() || !(text[idx].is_ascii_alphabetic() || text[idx] == b'_') {
			return None;
		}

		idx += 1;
		while idx < text.len() && (text[idx].is_ascii_alphanumeric() || text[idx] == b'_') {
			idx += 1;
		}

		Some((idx, String::from_utf8_lossy(&text[start..idx]).to_string()))
	}

	let bytes = path_text.as_bytes();
	let mut idx = skip_ws(bytes, 0);
	let mut prefix_start = idx;

	if let Some(next) = consume_double_colon(bytes, idx) {
		prefix_start = idx;
		idx = skip_ws(bytes, next);
	}

	let std_start = idx;
	let (next_idx, ident) = consume_ident(bytes, idx)?;

	if ident != "std" {
		return None;
	}

	let prefix_end = consume_double_colon(bytes, next_idx)?;
	let (next_idx, macro_name) = consume_ident(bytes, prefix_end)?;
	let next_after_macro = skip_ws(bytes, next_idx);

	if consume_double_colon(bytes, next_after_macro).is_some() {
		return None;
	}
	if prefix_start == std_start {
		prefix_start = std_start;
	}

	Some((prefix_start, prefix_end, macro_name))
}

pub(crate) fn check_logging_quality(ctx: &FileContext, violations: &mut Vec<Violation>) {
	for macro_call in ctx.source_file.syntax().descendants().filter_map(ast::MacroCall::cast) {
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
		let line = super::shared::line_from_offset(
			&ctx.line_starts,
			usize::from(macro_call.syntax().text_range().start()),
		);

		if let Some(message) = message {
			if message.contains('{') || message.contains('}') {
				super::shared::push_violation(
					violations,
					ctx,
					line,
					"RUST-STYLE-LOG-002",
					"Do not interpolate dynamic values in log message strings; use structured fields.",
					false,
				);
			}
			if !is_sentence(&message) {
				super::shared::push_violation(
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
			super::shared::push_violation(
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

	for method_call in ctx.source_file.syntax().descendants().filter_map(ast::MethodCallExpr::cast)
	{
		if method_call_in_test_context(&method_call) {
			continue;
		}

		let Some(name) = method_call.name_ref().map(|name| name.text().to_string()) else {
			continue;
		};
		let line = super::shared::line_from_offset(
			&ctx.line_starts,
			usize::from(method_call.syntax().text_range().start()),
		);

		if name == "unwrap" {
			super::shared::push_violation(
				violations,
				ctx,
				line,
				"RUST-STYLE-RUNTIME-001",
				"Do not use unwrap() in non-test code.",
				true,
			);

			if emit_edits {
				let name_range =
					method_call.name_ref().map(|name_ref| name_ref.syntax().text_range());
				let arg_range =
					method_call.arg_list().map(|arg_list| arg_list.syntax().text_range());

				if let (Some(name_range), Some(arg_range)) = (name_range, arg_range) {
					edits.push(Edit {
						start: usize::from(name_range.start()),
						end: usize::from(arg_range.end()),
						replacement: r#"expect("Expected operation to succeed.")"#.to_owned(),
						rule: "RUST-STYLE-RUNTIME-001",
					});
				}
			}

			continue;
		}
		if name != "expect" {
			continue;
		}

		let Some(arg_list) = method_call.arg_list() else {
			super::shared::push_violation(
				violations,
				ctx,
				line,
				"RUST-STYLE-RUNTIME-002",
				"expect() must use a clear, user-actionable string literal message.",
				true,
			);

			if emit_edits {
				let name_range =
					method_call.name_ref().map(|name_ref| name_ref.syntax().text_range());

				if let Some(name_range) = name_range {
					edits.push(Edit {
						start: usize::from(name_range.start()),
						end: usize::from(method_call.syntax().text_range().end()),
						replacement: r#"expect("Expected operation to succeed.")"#.to_owned(),
						rule: "RUST-STYLE-RUNTIME-002",
					});
				}
			}

			continue;
		};
		let mut args = arg_list.args();
		let Some(first_arg) = args.next() else {
			super::shared::push_violation(
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

			continue;
		};
		let literal = first_arg
			.syntax()
			.descendants()
			.filter_map(ast::Literal::cast)
			.next()
			.and_then(|lit| parse_string_literal(&lit.syntax().text().to_string()));
		let Some(message) = literal else {
			super::shared::push_violation(
				violations,
				ctx,
				line,
				"RUST-STYLE-RUNTIME-002",
				"expect() must use a clear, user-actionable string literal message.",
				false,
			);

			continue;
		};
		let message = message.trim().to_owned();

		if message.is_empty() {
			super::shared::push_violation(
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

			continue;
		}

		let first = message.chars().next().unwrap_or('a');
		let last = message.chars().last().unwrap_or('.');

		if !first.is_uppercase() || !matches!(last, '.' | '!' | '?') {
			super::shared::push_violation(
				violations,
				ctx,
				line,
				"RUST-STYLE-RUNTIME-002",
				"expect() message should start with a capital letter and end with punctuation.",
				true,
			);

			if emit_edits {
				let normalized = normalize_expect_message(&message);

				edits.push(Edit {
					start: usize::from(first_arg.syntax().text_range().start()),
					end: usize::from(first_arg.syntax().text_range().end()),
					replacement: format!("{normalized:?}"),
					rule: "RUST-STYLE-RUNTIME-002",
				});
			}
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
		let line = super::shared::line_from_offset(&ctx.line_starts, start);

		if let Some(suffix_start) = numeric_suffix_start(&literal_text) {
			let body = &literal_text[..suffix_start];

			if is_decimal_body(body) && !body.ends_with('_') {
				super::shared::push_violation(
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

		super::shared::push_violation(
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
			super::shared::text_range_to_lines(&ctx.line_starts, body.syntax().text_range());

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
			super::shared::push_violation(
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
			let line = super::shared::line_from_offset(
				&ctx.line_starts,
				usize::from(function.syntax().text_range().start()),
			);

			super::shared::push_violation(
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
			super::shared::push_violation(
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

fn split_top_level_args(args: &str) -> Vec<String> {
	let mut parts = Vec::new();
	let mut start = 0_usize;
	let mut paren = 0_i32;
	let mut brace = 0_i32;
	let mut bracket = 0_i32;
	let mut in_str = false;
	let mut escape = false;
	let mut in_char = false;
	let mut char_escape = false;
	let mut in_line_comment = false;
	let mut block_comment_depth = 0_i32;
	let chars = args.char_indices().collect::<Vec<_>>();
	let mut idx = 0_usize;

	while idx < chars.len() {
		let (offset, ch) = chars[idx];
		let next = if idx + 1 < chars.len() { Some(chars[idx + 1].1) } else { None };

		if in_line_comment {
			if ch == '\n' {
				in_line_comment = false;
			}

			idx += 1;

			continue;
		}
		if block_comment_depth > 0 {
			if ch == '/' && next == Some('*') {
				block_comment_depth += 1;
				idx += 2;

				continue;
			}
			if ch == '*' && next == Some('/') {
				block_comment_depth -= 1;
				idx += 2;

				continue;
			}

			idx += 1;

			continue;
		}
		if in_str {
			if escape {
				escape = false;
			} else if ch == '\\' {
				escape = true;
			} else if ch == '"' {
				in_str = false;
			}

			idx += 1;

			continue;
		}
		if in_char {
			if char_escape {
				char_escape = false;
			} else if ch == '\\' {
				char_escape = true;
			} else if ch == '\'' {
				in_char = false;
			}

			idx += 1;

			continue;
		}
		if ch == '/' && next == Some('/') {
			in_line_comment = true;
			idx += 2;

			continue;
		}
		if ch == '/' && next == Some('*') {
			block_comment_depth += 1;
			idx += 2;

			continue;
		}
		if ch == '"' {
			in_str = true;
			escape = false;
			idx += 1;

			continue;
		}
		if ch == '\'' {
			in_char = true;
			char_escape = false;
			idx += 1;

			continue;
		}

		match ch {
			'(' => paren += 1,
			')' => paren = (paren - 1).max(0),
			'{' => brace += 1,
			'}' => brace = (brace - 1).max(0),
			'[' => bracket += 1,
			']' => bracket = (bracket - 1).max(0),
			',' if paren == 0 && brace == 0 && bracket == 0 => {
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

fn macro_path_text(macro_call: &ast::MacroCall) -> Option<String> {
	macro_call.path().map(|path| path.syntax().text().to_string())
}

fn is_test_file(path: &Path) -> bool {
	let text = path.to_string_lossy().replace('\\', "/");

	text.contains("/tests/") || text.ends_with("_test.rs")
}

fn has_attr_text(mut attrs: impl Iterator<Item = ast::Attr>, needle: &str) -> bool {
	let compact = needle.replace(' ', "");

	attrs.any(|attr| attr.syntax().text().to_string().replace(' ', "").contains(&compact))
}

fn method_call_in_test_context(call: &ast::MethodCallExpr) -> bool {
	for node in call.syntax().ancestors() {
		if let Some(module) = ast::Module::cast(node.clone()) {
			if has_attr_text(module.attrs(), "cfg(test)") {
				return true;
			}
		}
		if let Some(function) = ast::Fn::cast(node) {
			if has_attr_text(function.attrs(), "test") {
				return true;
			}
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
