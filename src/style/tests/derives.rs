use std::path::Path;

use crate::style::{engine, fixes, rules, shared};

#[test]
fn import011_reorders_derive_groups_and_alphabetizes_entries() {
	let original = r#"
use serde::Serialize;
use crate::WorkspaceMacro;

#[derive(
	Serialize,
	Debug,
	WorkspaceMacro,
	Clone,
	Default,
)]
struct Row;
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import011_derive_groups.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-011" && v.fixable));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-011"));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied > 0);
	assert!(rewritten.contains("#[derive(Clone, Debug, Default, Serialize, WorkspaceMacro)]"));
}

#[test]
fn import011_reorders_unqualified_imported_derives_by_display_text() {
	let original = r#"
use sqlx::FromRow;
use serde::Serialize;

#[derive(Serialize, Default, FromRow, Clone)]
struct Row;
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import011_unqualified_derive.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-011" && v.fixable));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-011"));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied > 0);
	assert!(rewritten.contains("#[derive(Clone, Default, FromRow, Serialize)]"));
}

#[test]
fn import011_skips_derive_when_any_entry_is_unresolved() {
	let original = r#"
#[derive(Clone, Unknown, serde::Serialize)]
struct Row;
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import011_unresolved_derive.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-011"));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-011"));
}

#[test]
fn import011_converges_when_imported_derive_symbol_and_associated_call_stay_short() {
	let original = r#"
use foo::Bar;

#[derive(Bar, Clone)]
struct Row;

fn run() {
	foo::Bar::make();
}
"#;
	let (rewritten, applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(Path::new("import011_after_import009.rs"), original, true)
			.expect("apply fix passes");

	assert!(applied_count > 0, "Rewritten:\n{rewritten}");
	assert!(rewritten.contains("use foo::Bar;"), "Rewritten:\n{rewritten}");
	assert!(rewritten.contains("#[derive(Clone, Bar)]"), "Rewritten:\n{rewritten}");
	assert!(rewritten.contains("Bar::make();"), "Rewritten:\n{rewritten}");
}
