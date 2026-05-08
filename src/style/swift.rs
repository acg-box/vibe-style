use std::{fs, path::Path, sync::LazyLock};

use color_eyre::Result;
use regex::Regex;

use crate::style::shared::Violation;

static SYMBOL_IMPORT_RE: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(
		r"^\s*(?:@testable\s+)?import\s+(?:class|struct|enum|protocol|func|var|let|typealias)\s+",
	)
	.expect("Compile Swift symbol import regex.")
});
static PURE_TYPEALIAS_RE: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(
		r"^\s*(?:(?:open|public|package|internal|fileprivate|private)\s+)?typealias\s+[A-Za-z_][A-Za-z0-9_]*\s*=\s*[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*\s*$",
	)
	.expect("Compile Swift pure typealias regex.")
});
static TRY_FORCE_RE: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"\btry\s*!").expect("Compile Swift try-force regex."));
static DECIMAL_INTEGER_RE: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"\b[0-9][0-9_]*\b").expect("Compile Swift decimal integer regex.")
});
static FUNC_RE: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"\bfunc\b").expect("Compile Swift function regex."));

#[derive(Clone, Copy, Debug, Default)]
struct MaskState {
	block_comment_depth: usize,
	in_string: bool,
	in_multiline_string: bool,
	escape: bool,
}

#[derive(Clone, Copy, Debug)]
struct ActiveFunction {
	start_line: usize,
	brace_depth: i32,
}

pub(crate) fn collect_violations_from_file(path: &Path) -> Result<Vec<Violation>> {
	let text = match fs::read_to_string(path) {
		Ok(text) => text,
		Err(_) => return Ok(Vec::new()),
	};

	Ok(collect_violations_from_text(path, &text))
}

fn collect_violations_from_text(path: &Path, text: &str) -> Vec<Violation> {
	let lines = text.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
	let masked_lines = mask_swift_code_lines(&lines);
	let mut violations = Vec::new();

	check_file_layout(path, &mut violations);
	check_symbol_imports(path, &masked_lines, &mut violations);
	check_pure_typealiases(path, &masked_lines, &mut violations);
	check_runtime_force_operators(path, &masked_lines, &mut violations);
	check_decimal_integer_grouping(path, &masked_lines, &mut violations);
	check_function_length(path, &masked_lines, &mut violations);

	violations
}

fn check_file_layout(path: &Path, violations: &mut Vec<Violation>) {
	if path.file_name().is_some_and(|name| name == "mod.swift") {
		push_violation(
			violations,
			path,
			1,
			"SWIFT-STYLE-FILE-001",
			"Do not use mod.swift. Use flat Swift entry files instead.",
		);
	}
}

fn check_symbol_imports(path: &Path, masked_lines: &[String], violations: &mut Vec<Violation>) {
	for (idx, line) in masked_lines.iter().enumerate() {
		if SYMBOL_IMPORT_RE.is_match(line) {
			push_violation(
				violations,
				path,
				idx + 1,
				"SWIFT-STYLE-IMPORT-004",
				"Do not import individual Swift symbols; import modules instead.",
			);
		}
	}
}

fn check_pure_typealiases(path: &Path, masked_lines: &[String], violations: &mut Vec<Violation>) {
	for (idx, line) in masked_lines.iter().enumerate() {
		if PURE_TYPEALIAS_RE.is_match(line) {
			push_violation(
				violations,
				path,
				idx + 1,
				"SWIFT-STYLE-TYPE-001",
				"Do not add typealias declarations that are only pure renames.",
			);
		}
	}
}

fn check_runtime_force_operators(
	path: &Path,
	masked_lines: &[String],
	violations: &mut Vec<Violation>,
) {
	if is_swift_test_file(path) {
		return;
	}

	for (idx, line) in masked_lines.iter().enumerate() {
		if TRY_FORCE_RE.is_match(line) || has_force_operator(line) {
			push_violation(
				violations,
				path,
				idx + 1,
				"SWIFT-STYLE-RUNTIME-001",
				"Do not use force unwraps, force casts, or try! in non-test Swift code.",
			);
		}
	}
}

fn check_decimal_integer_grouping(
	path: &Path,
	masked_lines: &[String],
	violations: &mut Vec<Violation>,
) {
	for (idx, line) in masked_lines.iter().enumerate() {
		if DECIMAL_INTEGER_RE.find_iter(line).any(|found| {
			let number = found.as_str();

			number.len() > 3 && !number.contains('_') && !number.starts_with('0')
		}) {
			push_violation(
				violations,
				path,
				idx + 1,
				"SWIFT-STYLE-NUM-002",
				"Integers with more than three digits must use underscore separators.",
			);
		}
	}
}

fn check_function_length(path: &Path, masked_lines: &[String], violations: &mut Vec<Violation>) {
	let mut pending_func_line = None;
	let mut active_func: Option<ActiveFunction> = None;

	for (idx, line) in masked_lines.iter().enumerate() {
		if let Some(active) = active_func.as_mut() {
			active.brace_depth += brace_delta(line);

			if active.brace_depth <= 0 {
				let length = idx.saturating_sub(active.start_line) + 1;

				if length > 120 {
					push_violation(
						violations,
						path,
						active.start_line + 1,
						"SWIFT-STYLE-READ-002",
						&format!(
							"Function body has {length} lines; keep functions at or under 120 lines."
						),
					);
				}

				active_func = None;
			}

			continue;
		}

		if pending_func_line.is_none() && FUNC_RE.is_match(line) {
			pending_func_line = Some(idx);
		}
		if pending_func_line.is_some()
			&& let Some(open_brace) = line.find('{')
		{
			let delta = brace_delta(&line[open_brace..]);
			let active = ActiveFunction { start_line: idx, brace_depth: delta };

			if active.brace_depth <= 0 {
				let length = 1_usize;

				if length > 120 {
					push_violation(
						violations,
						path,
						active.start_line + 1,
						"SWIFT-STYLE-READ-002",
						&format!(
							"Function body has {length} lines; keep functions at or under 120 lines."
						),
					);
				}
			} else {
				active_func = Some(active);
			}

			pending_func_line = None;
		}
	}
}

fn mask_swift_code_lines(lines: &[String]) -> Vec<String> {
	let mut state = MaskState::default();

	lines.iter().map(|line| mask_swift_code_line(line, &mut state)).collect()
}

fn mask_swift_code_line(line: &str, state: &mut MaskState) -> String {
	let chars = line.chars().collect::<Vec<_>>();
	let mut masked = String::with_capacity(line.len());
	let mut idx = 0_usize;

	while idx < chars.len() {
		let ch = chars[idx];
		let next = chars.get(idx + 1).copied();
		let third = chars.get(idx + 2).copied();

		if state.block_comment_depth > 0 {
			if ch == '/' && next == Some('*') {
				state.block_comment_depth += 1;

				masked.push(' ');
				masked.push(' ');

				idx += 2;

				continue;
			}
			if ch == '*' && next == Some('/') {
				state.block_comment_depth = state.block_comment_depth.saturating_sub(1);

				masked.push(' ');
				masked.push(' ');

				idx += 2;

				continue;
			}

			masked.push(' ');

			idx += 1;

			continue;
		}
		if state.in_multiline_string {
			if ch == '"' && next == Some('"') && third == Some('"') {
				state.in_multiline_string = false;

				masked.push(' ');
				masked.push(' ');
				masked.push(' ');

				idx += 3;

				continue;
			}

			masked.push(' ');

			idx += 1;

			continue;
		}
		if state.in_string {
			if state.escape {
				state.escape = false;
			} else if ch == '\\' {
				state.escape = true;
			} else if ch == '"' {
				state.in_string = false;
			}

			masked.push(' ');

			idx += 1;

			continue;
		}
		if ch == '/' && next == Some('/') {
			break;
		}
		if ch == '/' && next == Some('*') {
			state.block_comment_depth += 1;

			masked.push(' ');
			masked.push(' ');

			idx += 2;

			continue;
		}
		if ch == '"' && next == Some('"') && third == Some('"') {
			state.in_multiline_string = true;

			masked.push(' ');
			masked.push(' ');
			masked.push(' ');

			idx += 3;

			continue;
		}
		if ch == '"' {
			state.in_string = true;

			masked.push(' ');

			idx += 1;

			continue;
		}

		masked.push(ch);

		idx += 1;
	}

	masked
}

fn has_force_operator(line: &str) -> bool {
	let chars = line.chars().collect::<Vec<_>>();

	for (idx, ch) in chars.iter().enumerate() {
		if *ch != '!' {
			continue;
		}
		if chars.get(idx + 1).is_some_and(|next| *next == '=') {
			continue;
		}

		let previous = chars[..idx].iter().rev().find(|candidate| !candidate.is_whitespace());

		if previous.is_some_and(|prev| {
			prev.is_ascii_alphanumeric() || matches!(prev, '_' | ')' | ']' | '}')
		}) {
			return true;
		}
	}

	false
}

fn brace_delta(line: &str) -> i32 {
	line.chars().fold(0_i32, |acc, ch| match ch {
		'{' => acc + 1,
		'}' => acc - 1,
		_ => acc,
	})
}

fn is_swift_test_file(path: &Path) -> bool {
	let text = path.to_string_lossy().replace('\\', "/");
	let file_name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();

	text.contains("/Tests/")
		|| text.contains("/tests/")
		|| file_name.ends_with("Tests.swift")
		|| file_name.ends_with("Test.swift")
}

fn push_violation(
	violations: &mut Vec<Violation>,
	path: &Path,
	line: usize,
	rule: &'static str,
	message: &str,
) {
	violations.push(Violation {
		file: path.to_path_buf(),
		line,
		rule,
		message: message.to_owned(),
		fixable: false,
	});
}

#[cfg(test)]
mod tests {
	use std::{collections::BTreeSet, path::Path};

	fn rules_for(path: &Path, text: &str) -> BTreeSet<&'static str> {
		super::collect_violations_from_text(path, text)
			.into_iter()
			.map(|violation| violation.rule)
			.collect()
	}

	#[test]
	fn reports_first_batch_swift_violations() {
		let rules = rules_for(
			Path::new("Sources/App/mod.swift"),
			r#"
import struct Foundation.UUID
typealias UserId = UUID

func load() {
	let id = maybeId!
	let forced = try! make()
	let count = 10000
}
"#,
		);

		assert!(rules.contains("SWIFT-STYLE-FILE-001"));
		assert!(rules.contains("SWIFT-STYLE-IMPORT-004"));
		assert!(rules.contains("SWIFT-STYLE-TYPE-001"));
		assert!(rules.contains("SWIFT-STYLE-RUNTIME-001"));
		assert!(rules.contains("SWIFT-STYLE-NUM-002"));
	}

	#[test]
	fn ignores_force_operators_in_swift_tests_and_strings() {
		let rules = rules_for(
			Path::new("Tests/AppTests.swift"),
			r#"
func testExample() {
	let text = "try! value!"
	let ok = value != other
	let forced = try! make()
}
"#,
		);

		assert!(!rules.contains("SWIFT-STYLE-RUNTIME-001"));
	}

	#[test]
	fn reports_long_swift_function_body() {
		let body = (0..121).map(|idx| format!("\tlet value{idx} = {idx}\n")).collect::<String>();
		let text = format!("func tooLong() {{\n{body}}}\n");
		let rules = rules_for(Path::new("Sources/App/Long.swift"), &text);

		assert!(rules.contains("SWIFT-STYLE-READ-002"));
	}
}
