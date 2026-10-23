use std::{
	env, fs,
	path::Path,
	process,
	time::{SystemTime, UNIX_EPOCH},
};

use crate::style::{
	engine, fixes, rules,
	shared::{self, Edit},
	types,
};

#[test]
fn suffix_rewrite_works() {
	let text = "let x = 10f32;\n";
	let ctx = shared::read_file_context_from_text(Path::new("a.rs"), text.to_owned())
		.expect("context")
		.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.is_empty());
	assert!(edits.iter().any(|edit| edit.rule == "RUST-STYLE-NUM-001"));
}

#[test]
fn runtime_reports_unwrap_without_autofix() {
	let original = "fn demo_case(value: Option<usize>) -> usize {\n\tvalue.unwrap()\n}\n";
	let ctx = shared::read_file_context_from_text(
		Path::new("runtime_unwrap_check.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(
		violations
			.iter()
			.any(|violation| violation.rule == "RUST-STYLE-RUNTIME-001" && !violation.fixable)
	);
	assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-RUNTIME-001"));
}

#[test]
fn runtime_does_not_enforce_expect_sentence_style() {
	let original =
		"fn demo_case(value: Option<usize>) -> usize {\n\tvalue.expect(\"missing value\")\n}\n";
	let ctx = shared::read_file_context_from_text(
		Path::new("runtime_expect_fix.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|violation| violation.rule == "RUST-STYLE-RUNTIME-002"));
	assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-RUNTIME-002"));
}

#[test]
fn runtime_reports_expect_empty_message_without_autofix() {
	let original = "fn demo_case(value: Option<usize>) -> usize {\n\tvalue.expect(\"\")\n}\n";
	let ctx = shared::read_file_context_from_text(
		Path::new("runtime_expect_empty_message.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|violation| {
		violation.rule == "RUST-STYLE-RUNTIME-002"
			&& violation.message == "expect() message must not be empty."
			&& !violation.fixable
	}));
	assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-RUNTIME-002"));
}

#[test]
fn log_rule_does_not_enforce_sentence_style() {
	let text = "fn run() {\n\ttracing::info!(\"missing value\")\n}\n";
	let ctx =
		shared::read_file_context_from_text(Path::new("log_sentence_style.rs"), text.to_owned())
			.expect("context")
			.expect("has ctx");
	let (violations, _edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|violation| {
		violation.rule == "RUST-STYLE-LOG-002"
			&& violation.message
				== "Log messages should be complete sentences with capitalization and punctuation."
	}));
}

#[test]
fn runtime_rules_ignore_cfg_test_module_calls() {
	let text = "#[cfg(test)]\nmod tests {\n\t#[test]\n\tfn sample_case() {\n\t\tlet value = Some(1usize);\n\t\tlet _ = value.unwrap();\n\t}\n}\n";
	let ctx =
		shared::read_file_context_from_text(Path::new("runtime_cfg_test.rs"), text.to_owned())
			.expect("context")
			.expect("has ctx");
	let (violations, _edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().all(|violation| !matches!(
		violation.rule,
		"RUST-STYLE-RUNTIME-001" | "RUST-STYLE-RUNTIME-002"
	)));
}

#[test]
fn runtime_rules_classify_standalone_test_files_without_matching_production_names() {
	let text =
		"fn fixture(value: Option<usize>) { let _ = value.unwrap(); let _ = value.expect(\"\"); }";

	for (path, test_file) in [
		("tests.rs", true),
		("tests/support/helper.rs", true),
		("src/agent_prompt_edit/tests.rs", true),
		(r"src\agent_prompt_edit\tests.rs", true),
		("src/tests/helper.rs", true),
		("src/journal_test.rs", true),
		("src/journal_tests.rs", true),
		("src/journal_tests_support.rs", false),
		("src/contests.rs", false),
		("src/tests_support.rs", false),
		("src/tests.rs/helper.rs", false),
		("src/production.rs", false),
	] {
		let ctx = shared::read_file_context_from_text(Path::new(path), text.to_owned())
			.expect("context")
			.expect("has ctx");
		let (violations, edits) = rules::collect_violations(&ctx, true);

		for rule in ["RUST-STYLE-RUNTIME-001", "RUST-STYLE-RUNTIME-002"] {
			assert_eq!(
				violations.iter().any(|violation| violation.rule == rule),
				!test_file,
				"{path}: {rule}"
			);
			assert!(!edits.iter().any(|edit| edit.rule == rule));
		}
	}
}

#[test]
fn runtime_rules_follow_external_test_module_owners() {
	let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Timestamp.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-test-owner-{}-{now}", process::id()));
	let source = root.join("src");
	let helper = source.join("support.rs");
	let text = "pub fn fixture(value: Option<usize>) { let _ = value.unwrap(); let _ = value.expect(\"\"); }";

	fs::create_dir_all(&source).expect("Create fixture.");
	fs::write(
		root.join("Cargo.toml"),
		"[package]\nname = \"test-owner-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
	)
	.expect("Write manifest.");
	fs::write(&helper, text).expect("Write helper.");
	fs::create_dir_all(source.join("nested")).expect("Create nested directory.");
	fs::write(source.join("bridge.rs"), "#[path = \"support.rs\"] mod fixture;")
		.expect("Write intermediate owner.");

	for (owner, production) in [
		("#[cfg(test)]\n#[path = \"support.rs\"] mod fixture;", false),
		("#[cfg(test)] mod support;", false),
		("#[cfg(test)] mod bridge;", false),
		("#[cfg(test)] mod nested { #[path = \"../support.rs\"] mod fixture; }", false),
		(
			"#[cfg(test)]\n#[path = \"support.rs\"] mod fixture;\n#[path = \"support.rs\"] mod production;",
			true,
		),
		("#[cfg(not(test))] mod support;", true),
		("#[cfg(any(test, feature = \"extra\"))] mod support;", true),
	] {
		fs::write(source.join("lib.rs"), owner).expect("Write owner.");

		let ctx = shared::read_file_context_from_text(&helper, text.to_owned())
			.expect("Read helper.")
			.expect("Get context.");
		let (violations, _) = rules::collect_violations(&ctx, false);

		for rule in ["RUST-STYLE-RUNTIME-001", "RUST-STYLE-RUNTIME-002"] {
			assert_eq!(violations.iter().any(|v| v.rule == rule), production, "{owner}: {rule}");
		}
	}

	fs::remove_dir_all(root).expect("Remove fixture.");
}

#[test]
fn style_fixes_preserve_serde_default_contracts() {
	// Option fields can still need defaults for sequence input, custom defaults,
	// and custom deserializers. Their type alone cannot prove redundancy.
	for attribute in [
		"#[serde(default)]",
		r#"#[serde(default, rename = "value")]"#,
		r#"#[serde(default = "default_value")]"#,
		r#"#[serde(default, deserialize_with = "read_value")]"#,
	] {
		for field_type in ["Option<String>", "std::option::Option<String>", "String"] {
			let original = format!(
				"#[derive(Deserialize)]\nstruct Payload {{\n\t{attribute}\n\tvalue: {field_type},\n}}\n"
			);
			let ctx = shared::read_file_context_from_text(
				Path::new("serde_defaults.rs"),
				original.clone(),
			)
			.expect("context")
			.expect("has ctx");
			let (violations, edits) = rules::collect_violations(&ctx, true);
			let mut rewritten = original;

			assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-SERDE-001"));

			fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

			assert!(rewritten.contains(attribute), "{rewritten}");
		}
	}
}

#[test]
fn style_fixes_preserve_serde_defaults_in_macro_input() {
	let original = r#"
define_payload! {
	struct Payload {
		#[serde(default)]
		value: Option<String>,
		#[serde(default = "defaults::fallback")]
		fallback: Option<String>,
	}
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("serde_macro_defaults.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);
	let mut rewritten = original.to_owned();

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-SERDE-001"));

	fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(rewritten.contains("#[serde(default)]"));
	assert!(rewritten.contains(r#"#[serde(default = "defaults::fallback")]"#));
}

#[test]
fn fix_passes_rewrite_cfg_pub_use_self_path_and_keep_macro_block_order() {
	let original = r#"
def_api_codes! {
	OUTSIDE_A = -3,
}

#[cfg(feature = "pubfi")]
pub use self::pubfi::{ERR_A, ERR_B};

def_api_codes! {
	OUTSIDE_B = -4,
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(Path::new("import010_mod001_macro_block.rs"), original, true)
			.expect("apply fix passes");
	let compact = rewritten.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
	let use_pos = compact.find("pubuseself::pubfi::{ERR_A,ERR_B};").expect("has rewritten pub use");
	let first_macro_pos = compact.find("OUTSIDE_A=-3").expect("has first macro call");
	let second_macro_pos = compact.find("OUTSIDE_B=-4").expect("has second macro call");

	assert!(rewritten.contains("pub use self::pubfi::{ERR_A, ERR_B};"));
	assert!(rewritten.contains("#[cfg(feature = \"pubfi\")]"));
	assert!(use_pos < first_macro_pos);
	assert!(first_macro_pos < second_macro_pos);
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
	let (check_violations, _check_edits) = rules::collect_violations(&ctx, false);
	let (fix_violations, _fix_edits) = rules::collect_violations(&ctx, true);
	let mut check_set =
		check_violations.iter().map(engine::violation_signature).collect::<Vec<_>>();
	let mut fix_set = fix_violations.iter().map(engine::violation_signature).collect::<Vec<_>>();

	check_set.sort();
	fix_set.sort();

	assert_eq!(check_set, fix_set);
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
	let (violations, _edits) = rules::collect_violations(&ctx, true);

	assert!(
		!violations.iter().any(|v| matches!(v.rule, "RUST-STYLE-NUM-001" | "RUST-STYLE-NUM-002"))
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
	let (_violations, edits) = rules::collect_violations(&ctx, true);
	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 2);
	assert!(rewritten.contains("0_usize"));
	assert!(rewritten.contains("80_000.0"));
}

#[test]
fn numeric_fix_applies_inside_macro_token_trees() {
	let original = r#"
fn sample() {
	let v = vec![0.0f32; 4];
}
"#;
	let ctx =
		shared::read_file_context_from_text(Path::new("num_macro_fix.rs"), original.to_owned())
			.expect("context")
			.expect("has ctx");
	let (_violations, edits) = rules::collect_violations(&ctx, true);
	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(rewritten.contains("vec![0.0_f32; 4]"));
}

#[test]
fn type001_flags_only_meaningless_aliases() {
	let original = r#"
type A = B;
type JsonValue = JsonValue;
type A<T> = B<T>;
type A<'a, T> = B<'a, T>;
"#;
	let ctx =
		shared::read_file_context_from_text(Path::new("types_rule_hits.rs"), original.to_owned())
			.expect("context")
			.expect("has ctx");
	let (violations, _edits) = rules::collect_violations(&ctx, true);
	let matches = violations.iter().filter(|v| v.rule == "RUST-STYLE-TYPE-001").collect::<Vec<_>>();

	assert_eq!(matches.len(), 4);
	assert!(matches.iter().all(|v| !v.fixable));
}

#[test]
fn type001_skips_specialized_or_non_path_aliases() {
	let original = r#"
		type Bytes = Vec<u8>;
		type MyResult<T> = Result<T, MyError>;
	type MyResultWithDefault<T, E = Error> = std::result::Result<T, E>;
	type Span = (usize, usize);
	"#;
	let ctx =
		shared::read_file_context_from_text(Path::new("types_rule_skips.rs"), original.to_owned())
			.expect("context")
			.expect("has ctx");
	let (violations, _edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-TYPE-001"));
}

#[test]
fn type001_skips_associated_types_in_impl_blocks() {
	let original = r#"
trait Service {
	type Response;
	type Error;
}

struct Wrapper;

impl Service for Wrapper {
	type Response = String;
	type Error = std::io::Error;
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("types_rule_impl_associated_types.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, _edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-TYPE-001"));
}

#[test]
fn type001_fix_removes_primitive_public_alias_and_rewrites_uses() {
	let original = r#"
pub type Code = i16;

	pub struct ApiEnvelope {
		pub code: Code,
	}
"#;
	let path = Path::new("types_rule_primitive_alias_fix.rs");
	let ctx = shared::read_file_context_from_text(path, original.to_owned())
		.expect("context")
		.expect("has ctx");
	let type_fixes = types::collect_type_alias_rename_fixes(&ctx);

	assert_eq!(type_fixes.len(), 1);

	let plan = &type_fixes[0];
	let renames = [plan.clone()];
	let mut edits = plan.definition_edits.clone();
	let skip_ranges = edits.iter().map(|edit| (edit.start, edit.end)).collect::<Vec<_>>();
	let usage_edits = types::build_type_alias_usage_rename_edits(&ctx, &renames, &skip_ranges);

	edits.extend(usage_edits);

	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(!rewritten.contains("type Code"));
	assert!(rewritten.contains("pub code: i16,"));
}

#[test]
fn type001_private_alias_is_autofixable() {
	let original = r#"
type Hidden = i32;

fn produce() -> Hidden {
	Hidden
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("types_rule_private_alias.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, _edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-TYPE-001" && v.fixable));

	let type_fixes = types::collect_type_alias_rename_fixes(&ctx);

	assert_eq!(type_fixes.len(), 1);

	let plan = &type_fixes[0];

	assert_eq!(plan.target, "i32");
	assert!(plan.definition_edits.iter().any(|edit| edit.rule == "RUST-STYLE-TYPE-001"));

	let renames = [plan.clone()];
	let skip_ranges =
		plan.definition_edits.iter().map(|edit| (edit.start, edit.end)).collect::<Vec<_>>();
	let usage_edits = types::build_type_alias_usage_rename_edits(&ctx, &renames, &skip_ranges);

	assert!(usage_edits.iter().any(|edit| edit.replacement == "i32"));
}

#[test]
fn generics002_fixes_typed_collect_turbofish() {
	let original = r#"
fn sample() {
	let values = vec![0_u8, 1];
	let iter = values.into_iter();
	let v: Vec<u8> = iter.collect::<Vec<u8>>();
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("generics002_collect.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);
	let matches =
		violations.iter().filter(|v| v.rule == "RUST-STYLE-GENERICS-002").collect::<Vec<_>>();

	assert_eq!(matches.len(), 1);
	assert!(matches.iter().all(|v| v.fixable));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-GENERICS-002"));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert_eq!(
		rewritten,
		r#"
fn sample() {
	let values = vec![0_u8, 1];
	let iter = values.into_iter();
	let v: Vec<u8> = iter.collect();
}
"#
	);
}

#[test]
fn generics002_fixes_typed_constructor_turbofish() {
	let original = r#"
use std::collections::HashMap;

fn sample() {
	let m: HashMap<u8, u8> = HashMap::<u8, u8>::new();
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("generics002_constructor.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);
	let matches =
		violations.iter().filter(|v| v.rule == "RUST-STYLE-GENERICS-002").collect::<Vec<_>>();

	assert_eq!(matches.len(), 1);
	assert!(matches.iter().all(|v| v.fixable));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-GENERICS-002"));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert_eq!(
		rewritten,
		r#"
use std::collections::HashMap;

fn sample() {
	let m: HashMap<u8, u8> = HashMap::new();
}
"#
	);
}

#[test]
fn generics002_skips_untyped_collect_turbofish() {
	let original = r#"
fn sample(iter: impl Iterator<Item = Vec<u8>>) {
	let v = iter.collect::<Vec<u8>>();
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("generics002_untyped_skip.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, _edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-GENERICS-002"));
}

#[test]
fn generics002_skips_inferred_collect_wildcard() {
	let original = r#"
fn sample(iter: impl Iterator<Item = u8>) {
	let v: Vec<u8> = iter.collect::<Vec<_>>();
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("generics002_wildcard_skip.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, _edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-GENERICS-002"));
}

#[test]
fn generics003_fixes_canonical_constructor_turbofish() {
	let original = r#"
fn sample() {
	let _ = <Vec<u8>>::new();
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("generics003_vec_constructor.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);
	let matches =
		violations.iter().filter(|v| v.rule == "RUST-STYLE-GENERICS-003").collect::<Vec<_>>();

	assert_eq!(matches.len(), 1);
	assert!(matches.iter().all(|v| v.fixable));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-GENERICS-003"));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert_eq!(
		rewritten,
		r#"
fn sample() {
	let _ = Vec::<u8>::new();
}
"#
	);
}

#[test]
fn generics003_fixes_canonical_collection_turbofish() {
	let original = r#"
fn sample() {
	let _ = <std::collections::HashMap<u8, u8>>::new();
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("generics003_hashmap_constructor.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);
	let matches =
		violations.iter().filter(|v| v.rule == "RUST-STYLE-GENERICS-003").collect::<Vec<_>>();

	assert_eq!(matches.len(), 1);
	assert!(matches.iter().all(|v| v.fixable));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-GENERICS-003"));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(rewritten.contains("let _ = std::collections::HashMap::<u8, u8>::new();"));
}

#[test]
fn generics003_skips_disambiguated_trait_path() {
	let original = r#"
trait Trait {
	type Assoc;
}

fn sample() {
	let _ = <T as Trait>::Assoc;
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("generics003_disambiguated_skip.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-GENERICS-003"));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-GENERICS-003"));
}

#[test]
fn generics003_skips_non_path_type_anchor() {
	let original = r#"
fn sample() {
	let _ = <(u8, u8)>::something;
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("generics003_non_path_anchor_skip.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-GENERICS-003"));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-GENERICS-003"));
}
