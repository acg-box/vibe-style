mod file;
mod fixes;
mod impls;
mod imports;
mod module;
mod quality;
mod shared;
mod spacing;

use std::{fs, path::PathBuf};

use crate::prelude::*;
pub(crate) use shared::RunSummary;
use shared::{Edit, FileContext, Violation};

pub(crate) fn run_check(requested_files: &[PathBuf]) -> Result<RunSummary> {
	let files = shared::resolve_files(requested_files)?;
	let mut violations: Vec<Violation> = Vec::new();

	for file in &files {
		if let Some(ctx) = shared::read_file_context(file)? {
			let (mut found, _edits) = collect_violations(&ctx, false);
			violations.append(&mut found);
		}
	}

	violations
		.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)).then(a.rule.cmp(b.rule)));

	let unfixable_count = violations.iter().filter(|v| !v.fixable).count();
	let output_lines = violations.into_iter().map(|v| v.format()).collect::<Vec<_>>();
	let violation_count = output_lines.len();

	Ok(RunSummary {
		file_count: files.len(),
		violation_count,
		unfixable_count,
		applied_fix_count: 0,
		output_lines,
	})
}

pub(crate) fn run_fix(requested_files: &[PathBuf]) -> Result<RunSummary> {
	let files = shared::resolve_files(requested_files)?;
	let mut output_lines = Vec::new();
	let mut total_applied = 0_usize;

	for file in &files {
		let mut text = match fs::read_to_string(file) {
			Ok(text) => text,
			Err(_) => continue,
		};

		let mut pass = 0;
		let mut changed = false;

		while pass < 8 {
			pass += 1;
			let Some(ctx) = shared::read_file_context_from_text(file, text.clone())? else {
				break;
			};
			let (_violations, edits) = collect_violations(&ctx, true);
			if edits.is_empty() {
				break;
			}

			let applied = fixes::apply_edits(&mut text, edits)?;
			if applied == 0 {
				break;
			}
			total_applied += applied;
			changed = true;
		}

		if changed {
			fs::write(file, text)?;
		}
	}

	let checked = run_check(requested_files)?;
	output_lines.extend(checked.output_lines);

	Ok(RunSummary {
		file_count: checked.file_count,
		violation_count: checked.violation_count,
		unfixable_count: checked.unfixable_count,
		applied_fix_count: total_applied,
		output_lines,
	})
}

pub(crate) fn print_coverage() {
	for rule in shared::STYLE_RULE_IDS {
		println!("{rule}\timplemented");
	}
}

fn collect_violations(ctx: &FileContext, with_fixes: bool) -> (Vec<Violation>, Vec<Edit>) {
	let mut violations = Vec::new();
	let mut edits = Vec::new();

	file::check_mod_rs(ctx, &mut violations);
	file::check_serde_option_default(ctx, &mut violations);
	file::check_error_rs_no_use(ctx, &mut violations);
	imports::check_import_rules(ctx, &mut violations, &mut edits, with_fixes);
	module::check_module_order(ctx, &mut violations, &mut edits, with_fixes);
	module::check_cfg_test_mod_tests_use_super(ctx, &mut violations, &mut edits, with_fixes);
	impls::check_impl_adjacency(ctx, &mut violations, &mut edits, with_fixes);
	impls::check_impl_rules(ctx, &mut violations, &mut edits, with_fixes);
	impls::check_inline_trait_bounds(ctx, &mut violations);
	quality::check_std_macro_calls(ctx, &mut violations, &mut edits, with_fixes);
	quality::check_logging_quality(ctx, &mut violations);
	quality::check_expect_unwrap(ctx, &mut violations);
	quality::check_numeric_literals(ctx, &mut violations, &mut edits, with_fixes);
	quality::check_function_length(ctx, &mut violations);
	spacing::check_vertical_spacing(ctx, &mut violations, &mut edits, with_fixes);
	quality::check_test_rules(ctx, &mut violations);

	(violations, edits)
}

#[cfg(test)]
fn violation_signature(violation: &Violation) -> (usize, &'static str, &str, bool) {
	(violation.line, violation.rule, violation.message.as_str(), violation.fixable)
}

#[cfg(test)]
mod tests {
	use std::path::Path;

	use super::*;

	#[test]
	fn suffix_rewrite_works() {
		let text = "let x = 10f32;\n";
		let ctx = shared::read_file_context_from_text(Path::new("a.rs"), text.to_owned())
			.expect("context")
			.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);
		assert!(!violations.is_empty());
		assert!(edits.iter().any(|edit| edit.rule == "RUST-STYLE-NUM-001"));
	}

	#[test]
	fn detects_cfg_test_super_use() {
		let text = "#[cfg(test)]\nmod tests {\n\t#[test]\n\tfn sample_case() {}\n}\n";
		let ctx = shared::read_file_context_from_text(Path::new("b.rs"), text.to_owned())
			.expect("context")
			.expect("has ctx");
		let (violations, _) = collect_violations(&ctx, false);
		assert!(violations.iter().any(|violation| violation.rule == "RUST-STYLE-MOD-007"));
	}

	#[test]
	fn fixes_cfg_test_super_use_with_allow_unused_imports() {
		let original = "#[cfg(test)]\nmod tests {\n\t#[test]\n\tfn sample_case() {}\n}\n";
		let ctx = shared::read_file_context_from_text(Path::new("mod007.rs"), original.to_owned())
			.expect("context")
			.expect("has ctx");
		let (_violations, edits) = collect_violations(&ctx, true);
		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		assert!(applied >= 1);
		assert!(rewritten.contains("#[allow(unused_imports)]"));
		assert!(rewritten.contains("use super::*;"));
		assert!(rewritten.contains("use super::*;\n\n\t#[test]"));
	}

	#[test]
	fn fixes_cfg_test_super_use_after_std_import_group() {
		let original = r#"
#[cfg(test)]
mod tests {
	use std::collections::HashSet;
	#[test]
	fn sample_case() {
		let _ = HashSet::<usize>::new();
	}
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod007_import_order.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (_violations, edits) = collect_violations(&ctx, true);
		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		assert!(applied >= 1);
		assert!(rewritten.contains(
			"use std::collections::HashSet;\n\n\t#[allow(unused_imports)]\n\tuse super::*;\n\n\t#[test]"
		));
	}

	#[test]
	fn fixes_cfg_test_super_use_when_existing_order_is_wrong() {
		let original = r#"
#[cfg(test)]
mod tests {
	#[allow(unused_imports)]
	use super::*;
	use std::collections::HashSet;
	use super::{ExtractedKeyphrase, topic_count};
	#[test]
	fn sample_case() {}
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod007_wrong_order_existing.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);
		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-MOD-007"
				&& (v.message
					== "In #[cfg(test)] mod tests, order imports as std, third-party, self/workspace."
					|| v.message
						== "In #[cfg(test)] mod tests, prefer `use super::*;` and remove specific super imports.")
				&& v.fixable
		}));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		assert!(applied >= 1);
		assert!(rewritten.contains(
			"use std::collections::HashSet;\n\n\t#[allow(unused_imports)]\n\tuse super::*;\n\n\t#[test]"
		));
		assert!(!rewritten.contains("use super::{ExtractedKeyphrase, topic_count};"));
	}

	#[test]
	fn does_not_apply_edits_inside_string_literals() {
		let mut text = r#"let spec = "ISO-8601 2025";"#.to_owned();
		let start = text.find("8601").expect("match");
		let end = start + "8601".len();
		let edits = vec![Edit { start, end, replacement: "8_601".to_owned(), rule: "TEST" }];
		let applied = fixes::apply_edits(&mut text, edits).expect("apply edits");
		assert_eq!(applied, 0);
		assert_eq!(text, r#"let spec = "ISO-8601 2025";"#);
	}

	#[test]
	fn applies_edits_after_lifetime_annotations() {
		let mut text = "fn f(index: &TextIndex<'_>) { let mut consumed = 0usize; }\n".to_owned();
		let start = text.find("usize").expect("usize");
		let edits = vec![Edit { start, end: start, replacement: "_".to_owned(), rule: "TEST" }];
		let applied = fixes::apply_edits(&mut text, edits).expect("apply edits");
		assert_eq!(applied, 1);
		assert!(text.contains("0_usize"));
	}

	#[test]
	fn check_and_fix_collect_same_violations() {
		let text = r#"
use crate::prelude::*;
use crate::foo::bar;

fn example() {
	let x = 10f32;
	let y = 10000;
	let spec = "ISO-8601, 2025";
	let _ = std::format!("{x}");
	println!("{spec}");
}
"#;
		let ctx = shared::read_file_context_from_text(Path::new("c.rs"), text.to_owned())
			.expect("context")
			.expect("has ctx");
		let (check_violations, _check_edits) = collect_violations(&ctx, false);
		let (fix_violations, _fix_edits) = collect_violations(&ctx, true);

		let mut check_set = check_violations.iter().map(violation_signature).collect::<Vec<_>>();
		let mut fix_set = fix_violations.iter().map(violation_signature).collect::<Vec<_>>();
		check_set.sort();
		fix_set.sort();

		assert_eq!(check_set, fix_set);
	}

	#[test]
	fn impl_fix_does_not_break_foreign_usage_paths() {
		let original = r#"
impl Usage {
	pub fn from_rig(local: Usage, usage: rig::completion::Usage) -> Usage {
		let _ = usage;
		local
	}
}
"#;
		let ctx = shared::read_file_context_from_text(Path::new("impl.rs"), original.to_owned())
			.expect("context")
			.expect("has ctx");
		let (_violations, edits) = collect_violations(&ctx, true);

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		assert!(applied > 0);
		assert!(rewritten.contains("local: Self"));
		assert!(rewritten.contains("usage: rig::completion::Usage"));
		assert!(rewritten.contains("-> Self"));
		assert!(!rewritten.contains(":: Self"));
	}

	#[test]
	fn numeric_rules_ignore_multiline_string_literals() {
		let text = r##"
const PROMPT: &str = r#"
- time_window values must be ISO-8601 durations.
- Absolute dates/years (example: 2025, by 2050) are not time_window.
"#;
"##;
		let ctx = shared::read_file_context_from_text(Path::new("num_prompt.rs"), text.to_owned())
			.expect("context")
			.expect("has ctx");
		let (violations, _edits) = collect_violations(&ctx, true);
		assert!(
			!violations
				.iter()
				.any(|v| matches!(v.rule, "RUST-STYLE-NUM-001" | "RUST-STYLE-NUM-002"))
		);
	}

	#[test]
	fn numeric_fix_applies_to_usize_and_float_integer_part() {
		let original = r#"
fn sample() {
	let a = 0usize;
	let b = 80000.0;
}
"#;
		let ctx = shared::read_file_context_from_text(Path::new("num_fix.rs"), original.to_owned())
			.expect("context")
			.expect("has ctx");
		let (_violations, edits) = collect_violations(&ctx, true);
		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 2);
		assert!(rewritten.contains("0_usize"));
		assert!(rewritten.contains("80_000.0"));
	}

	#[test]
	fn import_group_fix_normalizes_spacing_without_reordering_groups() {
		let original = r#"
use std::collections::HashSet;


use anyhow::Result;
use crate::z::Z;
"#;
		let ctx =
			shared::read_file_context_from_text(Path::new("import_fix.rs"), original.to_owned())
				.expect("context")
				.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);
		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-001"));
		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-002" && v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		assert!(applied >= 1);
		assert_eq!(
			rewritten,
			r#"
use std::collections::HashSet;

use anyhow::Result;

use crate::z::Z;
"#
		);
	}

	#[test]
	fn import_group_fix_does_not_rewrite_unknown_separator_comments() {
		let original = r#"
use crate::z::Z;
// keep this comment
use std::collections::HashSet;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import_fix_unknown_comment.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);
		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-001" && !v.fixable));
		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-002" && !v.fixable));
		assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-001"));
	}

	#[test]
	fn import_group_treats_workspace_members_as_self_group_for_spacing() {
		let original = r#"
use anyhow::Result;
use vibe_mono::internal::Alpha;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import_workspace_member.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);
		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-001"));
		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-002" && v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		assert!(applied >= 1);
		assert_eq!(
			rewritten,
			r#"
use anyhow::Result;

use vibe_mono::internal::Alpha;
"#
		);
	}

	#[test]
	fn import_fix_normalizes_mixed_self_child_use_tree() {
		let original = r#"
use crate::alpha::{beta, beta::Gamma};
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import_self_child.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);
		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-002"
				&& v.message
					== "Normalize imports like `use a::{b, b::c}` to `use a::{b::{self, c}}`."
				&& v.fixable
		}));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		assert!(applied >= 1);
		assert!(rewritten.contains("use crate::alpha::{beta::{self, Gamma}};"));
	}

	#[test]
	fn import_check_reports_ambiguous_symbol_imports() {
		let text = r#"
use foo::Client;
use bar::Client;
"#;
		let ctx =
			shared::read_file_context_from_text(Path::new("import_ambiguous.rs"), text.to_owned())
				.expect("context")
				.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);
		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-004"
				&& v.message
					== "Ambiguous imported symbol `Client` is not allowed; use fully qualified paths."
				&& !v.fixable
		}));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-004"));
	}

	#[test]
	fn space003_fix_removes_blank_lines_within_same_statement_type() {
		let original = r#"
fn sample() {
	let a = 1;

	let b = 2;
}
"#;
		let ctx =
			shared::read_file_context_from_text(Path::new("space_same.rs"), original.to_owned())
				.expect("context")
				.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);
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
		let ctx =
			shared::read_file_context_from_text(Path::new("space_diff.rs"), original.to_owned())
				.expect("context")
				.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);
		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-003"
			&& v.message == "Insert exactly one blank line between different statement types."
			&& v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		assert!(applied >= 1);
		assert!(rewritten.contains("let a = 1;\n\n\tif a > 0 {"));
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
		let ctx =
			shared::read_file_context_from_text(Path::new("space_attr.rs"), original.to_owned())
				.expect("context")
				.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);
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
		let (violations, edits) = collect_violations(&ctx, true);
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
		let (violations, edits) = collect_violations(&ctx, true);
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
		let ctx = shared::read_file_context_from_text(
			Path::new("space_const_group.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);
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
		let (violations, edits) = collect_violations(&ctx, true);
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
		let ctx = shared::read_file_context_from_text(
			Path::new("space_match_or_keep.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);
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
		let (violations, edits) = collect_violations(&ctx, true);
		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-003"
			&& v.message == "Do not insert blank lines inside a match pattern alternation."
			&& v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		assert!(applied >= 1);
		assert!(
			rewritten
				.contains("Pred::Tag { strength }\n\t\t| Pred::Entity { strength } => *strength,")
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
		let ctx = shared::read_file_context_from_text(
			Path::new("space004_return.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);
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
	fn mod005_fix_removes_blank_lines_between_type_and_impl() {
		let original = r#"
struct RuntimeEvent {
	id: usize,
}

impl RuntimeEvent {
	fn id(&self) -> usize {
		self.id
	}
}
"#;
		let ctx = shared::read_file_context_from_text(Path::new("mod005.rs"), original.to_owned())
			.expect("context")
			.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);
		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-005" && v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		assert!(applied >= 1);
		assert!(rewritten.contains("}\nimpl RuntimeEvent {"));
	}

	#[test]
	fn mod002_fix_reorders_pub_items_before_non_pub_items() {
		let original = r#"
fn internal() -> usize {
	1
}

pub fn external() -> usize {
	2
}
"#;
		let ctx = shared::read_file_context_from_text(Path::new("mod002.rs"), original.to_owned())
			.expect("context")
			.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);
		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-002" && v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		assert!(applied >= 1);
		assert!(rewritten.contains("pub fn external() -> usize"));
		assert!(
			rewritten.find("pub fn external").unwrap_or_default()
				< rewritten.find("fn internal").unwrap_or_default()
		);
	}

	#[test]
	fn mod003_fix_reorders_non_async_before_async_with_same_visibility() {
		let original = r#"
pub async fn pull() -> usize {
	1
}

pub fn plan() -> usize {
	2
}
"#;
		let ctx = shared::read_file_context_from_text(Path::new("mod003.rs"), original.to_owned())
			.expect("context")
			.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);
		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-003" && v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		assert!(applied >= 1);
		assert!(
			rewritten.find("pub fn plan").unwrap_or_default()
				< rewritten.find("pub async fn pull").unwrap_or_default()
		);
	}
}
