use std::{
	env, fs,
	path::Path,
	process::{self, Command},
	time::{SystemTime, UNIX_EPOCH},
};

use crate::style::{
	engine, fixes, rules,
	shared::{self, Edit},
};

#[test]
fn import005_fix_rewrites_error_rs_to_fully_qualified_paths() {
	let original = r#"
use std::fmt::{Display, Formatter};
use tonic::Status;

#[derive(Debug)]
pub enum ApiError {
	Internal,
}

impl Display for ApiError {
	fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
		write!(f, "boom")
	}
}

pub fn to_status() -> Status {
	Status::internal("boom")
}
"#;
	let ctx = shared::read_file_context_from_text(Path::new("error.rs"), original.to_owned())
		.expect("context")
		.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-005" && v.fixable));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-005"));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 3);
	assert!(!rewritten.contains("\nuse std::fmt::{Display, Formatter};"));
	assert!(!rewritten.contains("\nuse tonic::Status;"));
	assert!(rewritten.contains("impl std::fmt::Display for ApiError"));
	assert!(rewritten.contains("f: &mut std::fmt::Formatter"));
	assert!(rewritten.contains("pub fn to_status() -> tonic::Status"));
	assert!(rewritten.contains("tonic::Status::internal(\"boom\")"));
}

#[test]
fn import005_in_error_rs_is_not_fixable_when_symbol_is_ambiguous() {
	let text = r#"
use a::A;
use b::A;

pub enum Error {
	Value(A),
}
"#;
	let ctx = shared::read_file_context_from_text(Path::new("error.rs"), text.to_owned())
		.expect("context")
		.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-005" && !v.fixable));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-005"));
}

#[test]
fn import_fix_normalizes_mixed_self_child_use_tree() {
	let original = r#"
use crate::alpha::{beta, beta::Gamma};

fn sample(value: beta::Gamma) -> beta::Gamma {
	value
}
"#;
	let ctx =
		shared::read_file_context_from_text(Path::new("import_self_child.rs"), original.to_owned())
			.expect("context")
			.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-002"
			&& v.message == "Normalize imports like `use a::{b, b::c}` to `use a::{b::{self, c}}`."
			&& v.fixable
	}));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(rewritten.contains("use crate::alpha::{beta::{self, Gamma}};"));
}

#[test]
fn import_fix_does_not_rewrite_already_grouped_multiline_use_tree() {
	let original = r#"
	use std::{
		future::{self, Future, Ready},
		pin::Pin,
		rc::Rc,
		task::{Context, Poll},
	};
	"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import_already_grouped_multiline.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-002"
			&& v.message == "Normalize imports like `use a::{b, b::c}` to `use a::{b::{self, c}}`."
	}));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-002"));
}

#[test]
fn import_fix_does_not_collapse_multiline_braced_use_tree_when_semantics_are_unchanged() {
	let original = r#"
	use qdrant_client::{
		Qdrant, QdrantError,
		qdrant::{
			CreateCollectionBuilder, Distance, Modifier, SparseVectorParamsBuilder,
			SparseVectorsConfigBuilder, VectorParamsBuilder, VectorsConfigBuilder,
		},
	};
	"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import_multiline_braced_third_party.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);
	let edits_debug = format!("{edits:?}");
	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-003"));
	assert_eq!(applied, 0, "Expected no edits.\nEdits: {edits_debug}\nRewritten:\n{rewritten}");
	assert_eq!(rewritten, original);
}

#[test]
fn import_check_does_not_report_ambiguous_symbol_for_keep_alive_alias() {
	let text = r#"
use color_eyre::eyre::Context as _;
use std::task::Context;
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import_keep_alive_ambiguous_false_positive.rs"),
		text.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, _edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-004"
			&& v.message
				== "Ambiguous imported symbol `Context` is not allowed; use fully qualified paths."
	}));
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
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-004"
			&& v.message
				== "Ambiguous imported symbol `Client` is not allowed; use fully qualified paths."
			&& !v.fixable
	}));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-004"));
}

#[test]
fn import_check_does_not_report_brace_artifact_symbol() {
	let text = r#"
use async_nats::{
	ConnectOptions, Event, HeaderMap,
	jetstream::{
		self, Context,
		consumer::{AckPolicy, PullConsumer},
	},
};
use crate::{
	config::{ConnectionConfig, ConsumeConfig, PublishConfig},
	error::{Error, Result},
	events::{DocumentIngestedEvent, FeedMatchEvent},
	msg_id,
};
"#;
	let ctx =
		shared::read_file_context_from_text(Path::new("import_brace_artifact.rs"), text.to_owned())
			.expect("context")
			.expect("has ctx");
	let (violations, _edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-004"
			&& v.message.contains("Ambiguous imported symbol `}` is not allowed")
	}));
}

#[test]
fn import_check_does_not_report_ambiguous_self_from_braced_use_tree() {
	let text = r#"
use futures::stream::{self, TryStreamExt};
use crate::{store::{self, InsightInsertOutcome}};
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import_self_ambiguous_false_positive.rs"),
		text.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, _edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-004"
			&& v.message.contains("Ambiguous imported symbol `self` is not allowed")
	}));
}

#[test]
fn import_fix_qualifies_unqualified_function_calls() {
	let original = r#"
use crate::math::sum;

fn sample() -> usize {
	sum(1, 2)
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import004_function_fix.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-004"
			&& v.message
				== "Do not import free functions or macros into scope; prefer qualified module paths."
			&& v.fixable
	}));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-004"));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 2);
	assert!(!rewritten.contains("use crate::math::sum;"));
	assert!(rewritten.contains("use crate::math;"));
	assert!(rewritten.contains("math::sum(1, 2)"));
}

#[test]
fn import_fix_uses_full_path_when_parent_module_name_is_ambiguous() {
	let original = r#"
use other::math;
use crate::math::sum;

fn sample() -> usize {
	sum(1, 2)
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import004_function_fix_ambiguous_module.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(!rewritten.contains("use crate::math;"));
	assert!(rewritten.contains("crate::math::sum(1, 2)"), "{rewritten}");
}

#[test]
fn import_fix_rewrites_std_iter_once_to_parent_module_path() {
	let original = r#"
#[cfg(test)]
mod tests {
	use std::iter::once;

	#[test]
	fn sample() {
		let _ = once(1usize);
	}
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(Path::new("import004_std_iter_once.rs"), original, true)
			.expect("apply fix passes");

	assert!(rewritten.contains("\tuse std::iter;"), "{rewritten}");
	assert!(rewritten.contains("\t\tlet _ = iter::once("), "{rewritten}");
	assert!(!rewritten.contains("use std::iter::once;"), "{rewritten}");
}

#[test]
fn cfg_test_module_import_group_and_free_function_rules_apply_inside_tests_module() {
	let original = r#"
#[cfg(test)]
mod tests {
	use crate::math::sum;
	use anyhow::Result;
	use std::collections::HashSet;

	#[test]
	fn sample() -> Result<()> {
		let values = HashSet::new();
		let _ = values;
		Ok(sum(1, 2))
	}
}
"#;
	let path = Path::new("cfg_test_module_import_scope_gap.rs");
	let ctx = shared::read_file_context_from_text(path, original.to_owned())
		.expect("context")
		.expect("has ctx");
	let (violations, _edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-001" && v.fixable));
	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-002" && v.fixable));
	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-004"
			&& v.message
				== "Do not import free functions or macros into scope; prefer qualified module paths."
			&& v.fixable
	}));

	let (rewritten, applied, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(path, original, true).expect("apply fix passes");

	assert!(applied >= 3);

	let std_use_idx =
		rewritten.find("use std::collections::HashSet;").expect("std import must remain");
	let anyhow_use_idx =
		rewritten.find("use anyhow::Result;").expect("third-party import must remain");

	assert!(std_use_idx < anyhow_use_idx);
	assert!(
		rewritten.contains(
			"use std::collections::HashSet;\n\n\tuse anyhow::Result;\n\n\tuse crate::math;"
		)
	);
	assert!(rewritten.contains("Ok(math::sum(1, 2))"));
	assert!(!rewritten.contains("\tuse crate::math::sum;"));
}

#[test]
fn shared_import_insertions_preserve_distinct_conditions() {
	let edits = vec![
		Edit {
			start: 0,
			end: 0,
			replacement: "use std::io::ErrorKind;\n#[cfg(unix)]\nuse std::fmt::Error;\n".into(),
			rule: "RUST-STYLE-IMPORT-006",
		},
		Edit {
			start: 0,
			end: 0,
			replacement: "use std::io::ErrorKind;\n#[cfg(windows)]\nuse std::fmt::Error;\n".into(),
			rule: "RUST-STYLE-IMPORT-008",
		},
	];
	let mut text = "fn sample() {}\n".to_owned();

	fixes::apply_edits(&mut text, edits).expect("Apply insertions.");

	assert_eq!(text.matches("use std::io::ErrorKind;").count(), 1, "{text}");
	assert!(text.contains("#[cfg(unix)]\nuse std::fmt::Error;"), "{text}");
	assert!(text.contains("#[cfg(windows)]\nuse std::fmt::Error;"), "{text}");
}

#[test]
fn import_rewrites_preserve_descendant_bindings_and_module_aliases() {
	let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Timestamp.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-descendants-{}-{now}", process::id()));
	let source = root.join("src");
	let outer = source.join("outer.rs");

	fs::create_dir_all(&source).expect("Create fixture.");
	fs::write(
		root.join("Cargo.toml"),
		"[package]\nname = \"descendant-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
	)
	.expect("Write manifest.");
	fs::write(source.join("lib.rs"), "mod palette; mod outer;").expect("Write root.");
	fs::write(source.join("palette.rs"), "pub struct Shade; pub fn tone() -> u8 { 1 }")
		.expect("Write provider.");

	for (original, child) in [
		(
			"macro_rules! local { () => { struct Shade; }; }\nlocal!();\nuse crate::palette::*;\n#[path = \"child.rs\"] mod child;\nfn sample() -> u8 { tone() }",
			"#[test] fn preserved() { let _ = crate::outer::Shade; }",
		),
		(
			"use crate::palette::*;\n#[path = \"child.rs\"] mod child;\nfn sample() -> u8 { tone() }",
			"use crate::outer::Shade; #[test] fn preserved() { let _ = Shade; }",
		),
		(
			"use crate::palette::*;\n#[path = \"child.rs\"] mod child;\nfn sample() -> u8 { tone() }",
			"use crate::outer::*; #[test] fn preserved() { let _ = Shade; }",
		),
		(
			"use std::cmp::max;\n#[path = \"child.rs\"] mod child;\nfn sample() -> i32 { max(1, 2) }",
			"use crate::outer as api; #[test] fn preserved() { assert_eq!(api::max(3, 4), 4); }",
		),
		(
			"use std::cmp::max;\n#[path = \"child.rs\"] mod child;\nfn sample() -> i32 { max(1, 2) }",
			"use crate::{outer}; #[test] fn preserved() { assert_eq!(outer::max(3, 4), 4); }",
		),
	] {
		fs::write(&outer, original).expect("Write owner.");
		fs::write(source.join("child.rs"), child).expect("Write child.");

		let (rewritten, _, _, _) =
			engine::apply_fix_passes(&outer, original, true).expect("Fix owner.");

		fs::write(&outer, &rewritten).expect("Write fixed owner.");

		let binary = root.join("fixture");
		let compiled = Command::new("rustc")
			.args(["--edition=2024", "--test"])
			.arg(source.join("lib.rs"))
			.arg("-o")
			.arg(&binary)
			.output()
			.expect("Compile fixture.");

		assert!(
			compiled.status.success(),
			"{child}\n{rewritten}\n{}",
			String::from_utf8_lossy(&compiled.stderr)
		);

		let executed = Command::new(&binary).output().expect("Run fixture.");

		assert!(executed.status.success(), "{}", String::from_utf8_lossy(&executed.stderr));
		assert!(String::from_utf8_lossy(&executed.stdout).contains("1 passed;"));
	}

	let provider = "#[cfg(test)] use std::io::Read as _;\npub struct Token;\n#[path = \"child.rs\"] mod child;";
	let child = "use crate::outer::*; #[test] fn preserved() { let _ = Token; assert_eq!(std::io::empty().read(&mut []).unwrap(), 0); }";
	let child_path = source.join("child.rs");

	fs::write(&outer, provider).expect("Write anonymous trait provider.");
	fs::write(&child_path, child).expect("Write anonymous trait consumer.");

	let (rewritten, _, _, _) =
		engine::apply_fix_passes(&child_path, child, true).expect("Fix anonymous trait consumer.");

	assert!(!rewritten.contains("::*"), "{rewritten}");

	fs::write(&child_path, &rewritten).expect("Write fixed trait consumer.");

	let compiled = Command::new("rustc")
		.args(["--edition=2024", "--test"])
		.arg(source.join("lib.rs"))
		.arg("-o")
		.arg(root.join("fixture"))
		.output()
		.expect("Compile trait fixture.");

	assert!(
		compiled.status.success(),
		"{rewritten}\n{}",
		String::from_utf8_lossy(&compiled.stderr)
	);

	fs::remove_dir_all(root).expect("Remove fixture.");
}

#[test]
fn import_removal_qualifies_dependent_nested_use_roots() {
	let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Timestamp.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-nested-use-{}-{now}", process::id()));
	let path = root.join("lib.rs");

	fs::create_dir_all(&root).expect("Create fixture directory.");

	for original in [
		"use std::io::ErrorKind as Kind;\nfn sample() { use Kind as Source; let _ = Source::NotFound; }\n",
		"use std::io::ErrorKind;\nfn sample(value: ErrorKind) -> bool { use ErrorKind as Source; matches!(value, Source::NotFound) }\n#[cfg(unix)] fn qualified(_: std::io::ErrorKind) {}\n",
		"use std::io::ErrorKind as Kind;\nfn sample() { use {Kind as Source}; let _ = Source::NotFound; }\n",
		"use std::io::ErrorKind as Kind;\nfn sample() { use Kind::{NotFound}; let _ = NotFound; }\n",
		"use std::io::ErrorKind as Kind;\nfn outer(_: Kind) {}\nfn sample() { use std::option::Option as Kind; use Kind as Source; let _ = Source::Some(1); }\n",
	] {
		let (rewritten, _, _, _) =
			engine::apply_fix_passes(&path, original, true).expect("Apply fixes.");

		fs::write(&path, &rewritten).expect("Write rewritten fixture.");

		let output = Command::new("rustc")
			.args(["--edition=2024", "--crate-type=lib", "--emit=metadata"])
			.arg(&path)
			.arg("-o")
			.arg(root.join("fixture.rmeta"))
			.output()
			.expect("Compile rewritten fixture.");

		assert!(
			output.status.success(),
			"{rewritten}\n{}",
			String::from_utf8_lossy(&output.stderr)
		);
	}

	fs::remove_dir_all(root).expect("Remove fixture.");
}

#[test]
fn import_fixes_qualify_nested_macros_without_changing_literal_text() {
	let original = r#"
use crate::metrics::emit;
fn sample() {
    emit!(emit!("emit!()"), crate::metrics::emit!("qualified"));
    wrapper!(selector: emit!(), qualified: crate::metrics::emit!());
}
"#;
	let (rewritten, _, _, _) =
		engine::apply_fix_passes(Path::new("nested_macro.rs"), original, true)
			.expect("Apply fixes.");

	assert!(rewritten.contains("metrics::emit!(metrics::emit!(\"emit!()\")"), "{rewritten}");
	assert!(!rewritten.contains("metrics::metrics::"), "{rewritten}");
	assert!(rewritten.contains("selector: metrics::emit!()"), "{rewritten}");
}

#[test]
fn import_fixes_preserve_bindings_imported_by_child_modules() {
	for (file, child_import) in [
		("src/lib.rs", "super::emit"),
		("src/lib.rs", "super::*"),
		("src/lib.rs", "crate::emit"),
		("src/parent.rs", "crate::parent::emit"),
	] {
		let original = format!(
			"use crate::metrics::emit;\nfn sample() {{ emit!(); }}\n#[cfg(test)]\nmod tests {{\n    use {child_import};\n    fn child() {{ emit!(); }}\n}}\n"
		);
		let (rewritten, _, _, _) =
			engine::apply_fix_passes(Path::new(file), &original, true).expect("Apply fixes.");

		assert!(rewritten.contains("use crate::metrics::emit;"), "{rewritten}");
	}
}

#[test]
fn import_rules_ignore_paths_inside_macro_string_literals() {
	let original = r##"fn sample() {
    assert_eq!("std::io::empty()", r#"std::io::empty()"#);
    assert_eq!("foo::Bar { x: 1 }", "foo::Bar { x: 1 }");
}"##;
	let ctx =
		shared::read_file_context_from_text(Path::new("macro_strings.rs"), original.to_owned())
			.expect("Read context.")
			.expect("Have context.");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(
		!violations.iter().any(|v| v.rule.starts_with("RUST-STYLE-IMPORT-")),
		"{violations:#?}"
	);
	assert!(!edits.iter().any(|edit| edit.rule.starts_with("RUST-STYLE-IMPORT-")), "{edits:#?}");
}

#[test]
fn import_edits_preserve_bindings_when_rules_overlap() {
	let original =
		"use std::io::Read;\n\nfn sample() {\n    let _ = std::io::empty().read(&mut []);\n}\n";
	let path = Path::new("import_overlap.rs");
	let ctx = shared::read_file_context_from_text(path, original.to_owned())
		.expect("Read context.")
		.expect("Have context.");
	let (_, edits) = rules::collect_violations(&ctx, true);
	let mut rewritten = original.to_owned();

	fixes::apply_edits(&mut rewritten, edits).expect("Apply conflicting import rules.");

	assert!(
		rewritten.contains("std::io::empty()") || rewritten.contains("use std::io::{self,"),
		"{rewritten}"
	);

	let (finished, _, _, _) =
		engine::apply_fix_passes(path, original, true).expect("Finish rewrites.");

	assert!(!finished.contains("std::io::empty()"), "{finished}");
	assert!(
		finished.contains("use std::io::{self,") || finished.contains("use std::io;"),
		"{finished}"
	);
}

#[test]
fn import_fix_qualifies_unqualified_macro_calls() {
	let original = r#"
use crate::metrics::emit;

fn sample() {
	emit!("ok");
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import004_macro_fix.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-004" && v.fixable));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-004"));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 2);
	assert!(!rewritten.contains("use crate::metrics::emit;"));
	assert!(rewritten.contains("use crate::metrics;"));
	assert!(rewritten.contains("metrics::emit!(\"ok\")"));
}

#[test]
fn import_fix_rewrites_braced_use_tree_member() {
	let original = r#"
use super::shared::{Edit, line_from_offset, offset_from_line};

fn sample() {
	let _ = line_from_offset();
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import004_braced_fix.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-004" && v.fixable));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-004"));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 2);
	assert!(rewritten.contains("shared::line_from_offset()"));
	assert!(rewritten.contains("use super::shared::{self, Edit, offset_from_line};"));
	assert!(!rewritten.contains("use super::shared::{Edit, line_from_offset, offset_from_line};"));
}

#[test]
fn import_rules_skip_error_rs_and_do_not_add_imports() {
	let text = r#"
pub enum Error {
	Io(sqlx::Error),
}
"#;
	let ctx = shared::read_file_context_from_text(Path::new("error.rs"), text.to_owned())
		.expect("context")
		.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-008"));
	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009"));
	assert!(!edits.iter().any(|e| {
		matches!(
			e.rule,
			"RUST-STYLE-IMPORT-001"
				| "RUST-STYLE-IMPORT-002"
				| "RUST-STYLE-IMPORT-003"
				| "RUST-STYLE-IMPORT-004"
				| "RUST-STYLE-IMPORT-008"
				| "RUST-STYLE-IMPORT-009"
		)
	}));
}

#[test]
fn import_rules_report_cfg_test_module_import_violations() {
	let text = r#"#[cfg(test)]
mod tests {
	use crate::structured_fields::{StructuredFields, upsert_structured_fields_tx};
	use std::collections::BTreeMap;

	#[test]
	fn run_ops() {
		let _ = StructuredFields::default();
		let _ = BTreeMap::<u8, u8>::new();
		upsert_structured_fields_tx();
	}
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import_cfg_test_module_violations.rs"),
		text.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-004"
			&& v.line == 3
			&& v.fixable
			&& v.message
				== "Do not import free functions or macros into scope; prefer qualified module paths."
	}));
	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-001"
			&& v.line == 4
			&& v.fixable
			&& v.message == "Import groups must be ordered: std, third-party, self/workspace."
	}));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-004"));
	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-008"));
	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009"));
}

#[test]
fn import_rules_fix_cfg_test_module_imports() {
	let original = r#"#[cfg(test)]
mod tests {
	use crate::structured_fields::{StructuredFields, upsert_structured_fields_tx};
	use std::collections::BTreeMap;

	#[test]
	fn run_ops() {
		let _ = StructuredFields::default();
		let _ = BTreeMap::<u8, u8>::new();
		upsert_structured_fields_tx();
	}
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(Path::new("import_cfg_test_module_fix.rs"), original, true)
			.expect("apply fix passes");
	let std_use_idx =
		rewritten.find("use std::collections::BTreeMap;").expect("std import must remain");
	let crate_use_idx =
		rewritten.find("use crate::structured_fields").expect("crate import must remain");

	assert!(std_use_idx < crate_use_idx);
	assert!(
		rewritten.contains("use std::collections::BTreeMap;\n\n\tuse crate::structured_fields")
	);
	assert!(
		rewritten.contains("\n\tuse crate::structured_fields::{self, StructuredFields};\n"),
		"{rewritten}"
	);
	assert!(rewritten.contains("\n\t\tstructured_fields::upsert_structured_fields_tx();\n"));
	assert!(!rewritten.contains(
		"use crate::structured_fields::{StructuredFields, upsert_structured_fields_tx};"
	));
	assert!(!rewritten.contains("use crate::structured_fields::upsert_structured_fields_tx;"));

	let rewritten_ctx = shared::read_file_context_from_text(
		Path::new("import_cfg_test_module_fix.rs"),
		rewritten.clone(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&rewritten_ctx, true);

	assert!(!violations.iter().any(|v| {
		matches!(
			v.rule,
			"RUST-STYLE-IMPORT-001" | "RUST-STYLE-IMPORT-002" | "RUST-STYLE-IMPORT-004"
		)
	}));
	assert!(!edits.iter().any(|e| {
		matches!(
			e.rule,
			"RUST-STYLE-IMPORT-001" | "RUST-STYLE-IMPORT-002" | "RUST-STYLE-IMPORT-004"
		)
	}));
}

#[test]
fn import_rules_fix_cfg_test_module_function_calls_inside_macro_tokens() {
	let original = r#"#[cfg(test)]
mod tests {
	use crate::routes::{effective_token_id, inject_request_id_into_json_body};
	use uuid::Uuid;

	#[test]
	fn check_routes() {
		let request_id =
			Uuid::parse_str("123e4567-e89b-12d3-a456-426614174000").expect("valid uuid");
		let body = serde_json::json!({"note_id":"abc","status":"ok"}).to_string();
		let response_body = inject_request_id_into_json_body(body.as_bytes(), &request_id)
			.expect("Expected request_id field to be injected.");
		assert!(inject_request_id_into_json_body(body.as_bytes(), &request_id).is_none());
		assert_eq!(effective_token_id("off", &axum::http::HeaderMap::new()), None);
		let _ = response_body;
	}
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import_cfg_test_module_macro_token_function_fix.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert_eq!(
		rewritten
			.matches("routes::inject_request_id_into_json_body(body.as_bytes(), &request_id)")
			.count(),
		2,
		"Rewritten:\n{rewritten}"
	);
	assert!(rewritten.contains("use crate::routes::{self};"), "{rewritten}");
	assert!(
		rewritten.contains("routes::effective_token_id(\"off\", &axum::http::HeaderMap::new())")
	);
	assert!(
		!rewritten
			.contains("use crate::routes::{effective_token_id, inject_request_id_into_json_body};")
	);

	let rewritten_ctx = shared::read_file_context_from_text(
		Path::new("import_cfg_test_module_macro_token_function_fix.rs"),
		rewritten.clone(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&rewritten_ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-004"));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-004"));
}
