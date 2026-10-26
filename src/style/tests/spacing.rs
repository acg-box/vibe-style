use std::path::Path;

use crate::style::{engine, fixes, rules, shared, spacing};

#[test]
fn spacing_keeps_macro_literal_contents_and_checks_surrounding_rust() {
	for literal in [
		"r\"\nif (true) {}\nconst value = 1;\n\"",
		"r#\"\nif (true) {}\nconst value = \"雪\";\n\"#",
		"r##\"\nif (true) {}\nconst value = \"#;\n\"##",
		"br#\"\nif (true) {}\nconst value = 1;\n\"#",
		"cr#\"\nif (true) {}\nconst value = 1;\n\"#",
		"\"\nif (true) {}\nconst value = 1;\n\"",
		"b\"\nif (true) {}\nconst value = 1;\n\"",
		"c\"\nif (true) {}\nconst value = 1;\n\"",
	] {
		let text = format!(
			"fn sample() {{\n\tlet _value = envelope!({{\"input\": {literal}}});\n\tif ready() {{}}\n\n\tfinish();\n}}\n"
		);
		let ctx = shared::read_file_context_from_text(Path::new("macro_literal.rs"), text.clone())
			.expect("Read the macro literal fixture.")
			.expect("Get the macro literal fixture context.");
		let expected_line = text.lines().position(|line| line.contains("if ready()")).unwrap() + 1;
		let mut violations = Vec::new();
		let mut edits = Vec::new();

		spacing::check_vertical_spacing(&ctx, &mut violations, &mut edits, true);

		assert_eq!(violations.len(), 1, "{literal}: {violations:?}");
		assert_eq!(violations[0].rule, "RUST-STYLE-SPACE-003");
		assert_eq!(violations[0].line, expected_line);

		let mut rewritten = text.clone();

		assert_eq!(
			fixes::apply_edits(&mut rewritten, edits).expect("Apply the outer Rust spacing edit."),
			1
		);
		assert_eq!(rewritten, text.replace("\n\tif ready()", "\n\n\tif ready()"), "{literal}");
	}
}

#[test]
fn spacing_checks_real_rust_inside_macro_braces() {
	let text = "fn sample() {\n\trun!({\n\t\tlet value = 1;\n\t\tif value > 0 {}\n\t\tfinish();\n\t});\n}\n";
	let ctx = shared::read_file_context_from_text(Path::new("macro_code.rs"), text.to_owned())
		.expect("Read the Rust macro fixture.")
		.expect("Get the Rust macro fixture context.");
	let mut violations = Vec::new();
	let mut edits = Vec::new();

	spacing::check_vertical_spacing(&ctx, &mut violations, &mut edits, true);

	assert!(
		violations
			.iter()
			.any(|violation| violation.rule == "RUST-STYLE-SPACE-003" && violation.line == 4)
	);
	assert!(edits.iter().any(|edit| edit.rule == "RUST-STYLE-SPACE-003"));
}

#[test]
fn space003_fix_removes_blank_lines_within_same_statement_type() {
	let original = r#"
fn sample() {
	let a = 1;

	let b = 2;
}
"#;
	let ctx = shared::read_file_context_from_text(Path::new("space_same.rs"), original.to_owned())
		.expect("context")
		.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-003"
		&& v.message == "Do not insert blank lines within the same statement type."
		&& v.fixable));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(rewritten.contains("let a = 1;\n\tlet b = 2;"));
}

#[test]
fn space003_fix_inserts_single_blank_line_between_different_statement_types() {
	let original = r#"
fn sample() {
	let a = 1;
	if a > 0 {
		let _ = a;
	}
}
"#;
	let ctx = shared::read_file_context_from_text(Path::new("space_diff.rs"), original.to_owned())
		.expect("context")
		.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-003"
		&& v.message == "Insert exactly one blank line between different statement types."
		&& v.fixable));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(rewritten.contains("let a = 1;\n\n\tif a > 0 {"));
}

#[test]
fn spacing_fixes_preserve_metadata_and_literals() {
	for (source, anchor) in [
		(
			"fn sample() {\n let value = \"雪\";\n // Keep the check.\n #[cfg(test)]\n assert_eq!(value, \"雪\");\n}\n",
			" // Keep the check.",
		),
		(
			"fn sample() -> i32 {\n let value = 1;\n /* Keep this block\n  * and its layout. */\n return value;\n}\n",
			" /* Keep this block",
		),
		(
			"fn sample() -> i32 {\n let value = 1;\n // Keep the tail.\n #[cfg(all())]\n value\n}\n",
			" // Keep the tail.",
		),
	] {
		let ctx = shared::read_file_context_from_text(
			Path::new("metadata_spacing.rs"),
			source.to_owned(),
		)
		.expect("Read fixture.")
		.expect("Get fixture context.");
		let (_, edits) = rules::collect_violations(&ctx, true);
		let edits = edits
			.into_iter()
			.filter(|edit| matches!(edit.rule, "RUST-STYLE-SPACE-003" | "RUST-STYLE-SPACE-004"))
			.collect();
		let mut rewritten = source.to_owned();

		assert!(fixes::apply_edits(&mut rewritten, edits).expect("Apply spacing edits.") > 0);
		assert_eq!(rewritten, source.replace(anchor, &format!("\n{anchor}")));

		let ctx = shared::read_file_context_from_text(Path::new("metadata_spacing.rs"), rewritten)
			.expect("Read fixed fixture.")
			.expect("Get fixed context.");
		let (violations, _) = rules::collect_violations(&ctx, true);

		assert!(
			!violations
				.iter()
				.any(|v| matches!(v.rule, "RUST-STYLE-SPACE-003" | "RUST-STYLE-SPACE-004")),
			"{violations:?}"
		);
	}
}

#[test]
fn space003_match_guard_preserves_real_statement_checks() {
	let text = r#"
fn inspect(value: Item) -> bool {
	match value {
		Item::Ready { value } | Item::Pending { value }
			if value > 0 => {
			let ready = value > 1;
			if ready {
				return true;
			}

			false
		},
		_ => false,
	}
}
"#;
	let ctx = shared::read_file_context_from_text(Path::new("match_guard.rs"), text.into())
		.expect("context")
		.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);
	let guard_line = text.lines().position(|line| line.contains("if value > 0")).unwrap() + 1;
	let body_line = text.lines().position(|line| line.contains("if ready")).unwrap() + 1;

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-003" && v.line == guard_line));
	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-003" && v.line == body_line));
	assert!(edits.iter().any(|edit| edit.rule == "RUST-STYLE-SPACE-003"));
}

#[test]
fn space003_does_not_split_if_condition_from_body_brace_after_block_expression() {
	let text = r#"
fn lane_reference(
	metadata_file_type_is_symlink: bool,
	metadata_is_regular_file: bool,
	metadata_hard_link_count: u64,
	metadata_owner_uid: u32,
) -> bool {
	if metadata_file_type_is_symlink
		|| !metadata_is_regular_file
		|| metadata_hard_link_count != 1
		|| metadata_owner_uid != unsafe { libc::geteuid() }
	{
		return false;
	}

	true
}
"#;
	let path = Path::new("space_if_condition_block_expression.rs");
	let ctx = shared::read_file_context_from_text(path, text.to_owned())
		.expect("context")
		.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|violation| {
		violation.rule == "RUST-STYLE-SPACE-003"
			&& violation.message
				== "Insert exactly one blank line between different statement types."
	}));
	assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-SPACE-003"));

	let (rewritten, applied_count, _, _) =
		engine::apply_fix_passes(path, text, true).expect("apply fix passes");

	assert_eq!(applied_count, 0);
	assert_eq!(rewritten, text);
}

#[test]
fn space003_does_not_split_method_chain_after_struct_literal() {
	let text = r#"
fn sample() {
	let retry = Backoff {
		start_ms: args.retry_backoff_ms,
		cap_ms: args.retry_backoff_ms.saturating_mul(20).max(args.retry_backoff_ms),
		factor: 2,
		jitter_ratio: 0.2,
	}
	.retry_policy(args.retry_max);
}
"#;
	let ctx = shared::read_file_context_from_text(Path::new("space_chain.rs"), text.to_owned())
		.expect("context")
		.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-003"
		&& v.message == "Insert exactly one blank line between different statement types."));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-SPACE-003"));
}

#[test]
fn space003_fix_preserves_attributes_and_removes_only_blank_lines() {
	let original = r#"
fn sample() {
	#[derive(Debug)]
	struct A;


	#[derive(Debug)]
	struct B;
}
"#;
	let ctx = shared::read_file_context_from_text(Path::new("space_attr.rs"), original.to_owned())
		.expect("context")
		.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-SPACE-003"
			&& v.message == "Insert exactly one blank line between local item declarations."
			&& v.fixable
	}));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(rewritten.contains("struct A;\n\n\t#[derive(Debug)]\n\tstruct B;"));
}

#[test]
fn space003_does_not_remove_blank_between_local_items() {
	let text = r#"
fn schema() {
	static SCHEMA: usize = 1;

	fn build_schema() -> usize {
		SCHEMA
	}
}
"#;
	let ctx =
		shared::read_file_context_from_text(Path::new("space_items_keep.rs"), text.to_owned())
			.expect("context")
			.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-003"
		&& v.message == "Do not insert blank lines within the same statement type."));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-SPACE-003"));
}

#[test]
fn space003_fix_inserts_blank_between_local_items() {
	let original = r#"
fn schema() {
	static SCHEMA: usize = 1;
	fn build_schema() -> usize {
		SCHEMA
	}
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("space_items_insert.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-003"
		&& v.message == "Insert exactly one blank line between local item declarations."
		&& v.fixable));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(rewritten.contains("static SCHEMA: usize = 1;\n\n\tfn build_schema()"));
}

#[test]
fn space003_const_group_has_no_blank_lines() {
	let original = r#"
fn topic_limits() {
	const TOPIC_MAX: usize = 24;

	const TOPIC_RATIO_NUM: usize = 2;

	const TOPIC_RATIO_DEN: usize = 3;
}
"#;
	let ctx =
		shared::read_file_context_from_text(Path::new("space_const_group.rs"), original.to_owned())
			.expect("context")
			.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-003"
		&& v.message == "Do not insert blank lines within constant declaration groups."
		&& v.fixable));
	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-003"
		&& v.message == "Insert exactly one blank line between local item declarations."));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(rewritten.contains(
			"const TOPIC_MAX: usize = 24;\n\tconst TOPIC_RATIO_NUM: usize = 2;\n\tconst TOPIC_RATIO_DEN: usize = 3;"
		));
}

#[test]
fn space003_treats_assert_macros_as_same_group() {
	let original = r#"
fn sample() {
	assert_eq!(1, 1);

	assert!((0.0..=1.0).contains(&0.5), "ok");
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("space_assert_macros.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-003"
		&& v.message == "Do not insert blank lines within the same statement type."
		&& v.fixable));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(
		rewritten.contains("assert_eq!(1, 1);\n\tassert!((0.0..=1.0).contains(&0.5), \"ok\");")
	);
}

#[test]
fn space003_does_not_split_match_pattern_alternation() {
	let text = r#"
enum Pred {
	Tag { strength: i32 },
	Entity { strength: i32 },
}

fn pred_strength(pred: &Pred) -> i32 {
	match pred {
		Pred::Tag { strength }
		| Pred::Entity { strength } => *strength,
	}
}
"#;
	let ctx =
		shared::read_file_context_from_text(Path::new("space_match_or_keep.rs"), text.to_owned())
			.expect("context")
			.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-003"
		&& v.message == "Insert exactly one blank line between different statement types."));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-SPACE-003"));
}

#[test]
fn space003_fix_removes_blank_inside_match_pattern_alternation() {
	let original = r#"
enum Pred {
	Tag { strength: i32 },
	Entity { strength: i32 },
}

fn pred_strength(pred: &Pred) -> i32 {
	match pred {
		Pred::Tag { strength }

		| Pred::Entity { strength } => *strength,
	}
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("space_match_or_fix.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-003"
		&& v.message == "Do not insert blank lines inside a match pattern alternation."
		&& v.fixable));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(
		rewritten.contains("Pred::Tag { strength }\n\t\t| Pred::Entity { strength } => *strength,")
	);
}

#[test]
fn space004_fix_inserts_single_blank_before_return() {
	let original = r#"
fn sample(flag: bool) -> i32 {
	let value = 1;
	return if flag { value } else { 0 };
}
"#;
	let ctx =
		shared::read_file_context_from_text(Path::new("space004_return.rs"), original.to_owned())
			.expect("context")
			.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-SPACE-004"
			&& v.message == "Insert exactly one blank line before each return statement."
			&& v.fixable
	}));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(rewritten.contains("let value = 1;\n\n\treturn if flag { value } else { 0 };"));
}

#[test]
fn space003_fix_removes_blank_lines_between_top_level_const_items() {
	let original = r#"
const CONTENT_CHAR_LIMIT: usize = 500;

const INSIGHTS_PER_FEED_LIMIT: usize = 3;

const PROD_PUBLIC_WEB_BASE_URL: &str = "https://pubfi.ai";
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("space003_top_const_group.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-SPACE-003"
			&& v.message == "Do not insert blank lines within constant declaration groups."
			&& v.fixable
	}));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(rewritten.contains(
			"const CONTENT_CHAR_LIMIT: usize = 500;\nconst INSIGHTS_PER_FEED_LIMIT: usize = 3;\nconst PROD_PUBLIC_WEB_BASE_URL: &str = \"https://pubfi.ai\";"
		));
}

#[test]
fn space003_fix_removes_extra_blank_lines_between_top_level_items() {
	let original = r#"
struct RoundGateGuard;

impl Drop for RoundGateGuard {
	fn drop(&mut self) {}
}



/// Bootstrap and run all crawlers.
pub async fn run() {}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("space003_top_level_extra_blank.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-SPACE-003"
			&& v.message == "Do not insert extra blank lines between top-level items."
			&& v.fixable
	}));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(rewritten.contains("}\n\n/// Bootstrap and run all crawlers."));
	assert!(!rewritten.contains("}\n\n\n/// Bootstrap and run all crawlers."));
}

#[test]
fn space_rules_ignore_statements_inside_raw_string_literals() {
	let text = r##"
fn sample() {
	let prompt = r#"
	#[cfg(test)]
	mod tests {
		use super::*;
		fn fake_case() {}
	}
	"#;

	println!("{prompt}");
}
"##;
	let ctx =
		shared::read_file_context_from_text(Path::new("space_raw_string.rs"), text.to_owned())
			.expect("context")
			.expect("has ctx");
	let (violations, _edits) = rules::collect_violations(&ctx, true);

	assert!(
		!violations
			.iter()
			.any(|v| v.rule == "RUST-STYLE-SPACE-003" || v.rule == "RUST-STYLE-SPACE-004")
	);
}

#[test]
fn space004_keeps_binary_expression_continuations() {
	let text = r#"
fn safe_tail(flag: bool) -> bool {
    flag
        && unsafe { check() }
        && other()
}
fn real_gap(flag: bool) -> bool {
    record();
    flag
        && unsafe { check() }
        && other()
}
"#;
	let ctx = shared::read_file_context_from_text(Path::new("binary_tail.rs"), text.to_owned())
		.expect("context")
		.expect("has ctx");
	let (violations, _) = rules::collect_violations(&ctx, true);
	let tail_lines = violations
		.iter()
		.filter(|v| v.rule == "RUST-STYLE-SPACE-004")
		.map(|v| v.line)
		.collect::<Vec<_>>();

	assert_eq!(tail_lines, vec![9]);
}

#[test]
fn space004_fix_remains_autofixable_with_char_literal_conditions() {
	let original = r#"
fn classify(ch: char) -> usize {
	if ch == '\'' {
		let value = 1;
		return value;
	}

	0
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("space_char_literal.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-004" && v.fixable));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(rewritten.contains("let value = 1;\n\n\t\treturn value;"));
}

#[test]
fn spacing_fix_preserves_cfg_literals_and_unicode() {
	let original = "fn main() {\n    println!(\"界\");\n\n    #[cfg(target_os = \"macos\")]\n    println!(r#\"literal\n\n界\"#);\n}\n";
	let path = Path::new("spacing_cfg.rs");
	let (rewritten, count, _, _) =
		engine::apply_fix_passes(path, original, true).expect("Apply fixes.");
	let expected = original.replace(";\n\n    #[cfg", ";\n    #[cfg");

	assert!(count > 0);
	assert_eq!(rewritten, expected);

	let (repeated, _, _, _) =
		engine::apply_fix_passes(path, &rewritten, true).expect("Repeat fixes.");

	assert_eq!(rewritten, repeated);
}

#[test]
fn space003_handles_unicode_ufcs_targets() {
	let source = "fn sample() {\n <类型>::make();\n <类型>::make();\n}\n";
	let ctx = shared::read_file_context_from_text(Path::new("unicode_ufcs.rs"), source.to_owned())
		.expect("Read fixture.")
		.expect("Get fixture context.");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|violation| violation.rule == "RUST-STYLE-SPACE-003"));
	assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-SPACE-003"));
}
