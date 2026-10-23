use std::{
	env, fs,
	path::Path,
	process::{self, Command},
	time::{SystemTime, UNIX_EPOCH},
};

use crate::style::{engine, fixes, rules, shared};

#[test]
fn import008_fix_rewrites_existing_keep_alive_trait_import_to_plain_when_symbol_is_used() {
	let original = r#"
use serde::{Deserialize as _};

fn decode<T: serde::Deserialize<'static>>() {}
"#;
	let mut rewritten = original.to_owned();

	for _ in 0..4 {
		let ctx = shared::read_file_context_from_text(
			Path::new("import008_trait_keep_alive_upgrade.rs"),
			rewritten.clone(),
		)
		.expect("context")
		.expect("has ctx");
		let (_violations, edits) = rules::collect_violations(&ctx, true);

		if edits.is_empty() {
			break;
		}

		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
	}

	assert!(rewritten.contains("use serde::{Deserialize};"), "{rewritten}");
	assert!(!rewritten.contains("Deserialize as _"));
	assert!(rewritten.contains("fn decode<T: Deserialize<'static>>() {}"));
}

#[test]
fn import004_merges_parent_module_import_when_shortening_existing_full_path_call() {
	let original = r#"
use crate::{app::AuthState};

fn sample() {
	let _ = AuthState;
	crate::app::build_auth_state();
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import004_existing_full_path_function_merge.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(rewritten.contains("use crate::{app::{self, AuthState}};"), "{rewritten}");
	assert!(rewritten.contains("app::build_auth_state();"), "{rewritten}");
	assert!(!rewritten.contains("crate::app::build_auth_state();"), "{rewritten}");
}

#[test]
fn import004_preserves_turbofish_when_shortening_existing_full_path_call() {
	let original = r#"
fn sample() {
	crate::app::build_auth_state::<AuthState>();
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import004_existing_full_path_function_turbofish.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(rewritten.contains("use crate::app;"), "{rewritten}");
	assert!(rewritten.contains("app::build_auth_state::<AuthState>();"), "{rewritten}");
	assert!(!rewritten.contains("crate::app::build_auth_state::<AuthState>();"), "{rewritten}");
}

#[test]
fn import006_and_import008_share_inserted_type_imports() {
	let original = "pub fn sample() -> std::io::ErrorKind {\n use std::io::ErrorKind;\n ErrorKind::NotFound\n}\n";
	let (rewritten, _, _, _) =
		engine::apply_fix_passes(Path::new("shared_type_import.rs"), original, true)
			.expect("Apply fixes.");

	assert_eq!(rewritten.matches("use std::io::ErrorKind;").count(), 1, "{rewritten}");
}

#[test]
fn import_hoisting_precedes_qualified_path_shortening() {
	let original = "fn sample(_: std::io::ErrorKind) { use std::io::{ErrorKind, SeekFrom}; let _ = ErrorKind::NotFound; let _ = SeekFrom::Start(0); }\n";
	let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Timestamp.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-group-hoist-{}-{now}", process::id()));
	let path = root.join("lib.rs");

	fs::create_dir_all(&root).expect("Create fixture directory.");

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

	fs::remove_dir_all(root).expect("Remove fixture.");

	assert!(output.status.success(), "{rewritten}\n{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn import008_fix_imports_unambiguous_type_paths_and_keeps_group_order() {
	let original = r#"
use std::collections::HashSet;
use crate::local::Marker;

fn run<'e, E>(_exec: E)
where
	E: sqlx::Executor<'e>,
{
	let _ = HashSet::<usize>::new();
	let _ = Marker;
}
"#;
	let mut rewritten = original.to_owned();

	for _ in 0..4 {
		let ctx = shared::read_file_context_from_text(
			Path::new("import008_unambiguous.rs"),
			rewritten.clone(),
		)
		.expect("context")
		.expect("has ctx");
		let (_violations, edits) = rules::collect_violations(&ctx, true);

		if edits.is_empty() {
			break;
		}

		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
	}

	assert!(rewritten.contains("use sqlx::Executor;"));
	assert!(rewritten.contains("E: Executor<'e>,"));

	let std_idx = rewritten.find("use std::collections::HashSet;").expect("std");
	let third_party_idx = rewritten.find("use sqlx::Executor;").expect("third-party");
	let self_idx = rewritten.find("use crate::local::Marker;").expect("self");

	assert!(std_idx < third_party_idx);
	assert!(third_party_idx < self_idx);
}

#[test]
fn import008_shortens_same_name_module_macro_paths() {
	let original = r#"
use color_eyre::eyre::{Result};

fn build() -> Result<()> {
	Err(color_eyre::eyre::eyre!("boom"))
}
"#;
	let mut rewritten = original.to_owned();

	for _ in 0..4 {
		let ctx = shared::read_file_context_from_text(
			Path::new("import008_macro_module_shorten.rs"),
			rewritten.clone(),
		)
		.expect("context")
		.expect("has ctx");
		let (_violations, edits) = rules::collect_violations(&ctx, true);

		if edits.is_empty() {
			break;
		}

		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
	}

	assert!(rewritten.contains("eyre::eyre!(\"boom\")"));
	assert!(!rewritten.contains("color_eyre::eyre::eyre!(\"boom\")"));

	let use_tree_start =
		rewritten.find("use color_eyre::eyre::{").expect("must keep color_eyre::eyre use tree");
	let use_tree_end =
		rewritten[use_tree_start..].find("};").expect("must keep color_eyre::eyre use tree end");
	let use_tree = &rewritten[use_tree_start..use_tree_start + use_tree_end + 2];

	assert!(use_tree.contains("self"));
	assert!(use_tree.contains("Result"));
	assert!(!rewritten.contains("use color_eyre::eyre;"));
}

#[test]
fn import008_fix_keeps_module_import_for_same_name_module_macro() {
	let original = r#"
use color_eyre::eyre::{Result};

fn build() -> Result<()> {
	Err(color_eyre::eyre::eyre!("boom"))
}
"#;
	let mut rewritten = original.to_owned();

	for _ in 0..4 {
		let ctx = shared::read_file_context_from_text(
			Path::new("import008_macro_module_fix.rs"),
			rewritten.clone(),
		)
		.expect("context")
		.expect("has ctx");
		let (_violations, edits) = rules::collect_violations(&ctx, true);

		if edits.is_empty() {
			break;
		}

		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
	}

	assert!(rewritten.contains("eyre::eyre!(\"boom\")"));
	assert!(!rewritten.contains("color_eyre::eyre::eyre!(\"boom\")"));

	let use_tree_start =
		rewritten.find("use color_eyre::eyre::{").expect("must keep color_eyre::eyre use tree");
	let use_tree_end =
		rewritten[use_tree_start..].find("};").expect("must keep color_eyre::eyre use tree end");
	let use_tree = &rewritten[use_tree_start..use_tree_start + use_tree_end + 2];

	assert!(use_tree.contains("self"));
	assert!(use_tree.contains("Result"));
	assert!(!rewritten.contains("use color_eyre::eyre;"));
}

#[test]
fn import008_merges_same_name_module_macro_into_existing_braced_use_tree() {
	let original = r#"
use color_eyre::eyre::{Result};

fn build() -> Result<()> {
	Err(color_eyre::eyre::eyre!("boom"))
}
"#;
	let mut rewritten = original.to_owned();

	for _ in 0..4 {
		let ctx = shared::read_file_context_from_text(
			Path::new("import008_macro_module_merge.rs"),
			rewritten.clone(),
		)
		.expect("context")
		.expect("has ctx");
		let (_violations, edits) = rules::collect_violations(&ctx, true);

		if edits.is_empty() {
			break;
		}

		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
	}

	assert!(rewritten.contains("eyre::eyre!(\"boom\")"));
	assert!(!rewritten.contains("color_eyre::eyre::eyre!(\"boom\")"));

	let use_tree_start =
		rewritten.find("use color_eyre::eyre::{").expect("must keep color_eyre::eyre use tree");
	let use_tree_end =
		rewritten[use_tree_start..].find("};").expect("must keep color_eyre::eyre use tree end");
	let use_tree = &rewritten[use_tree_start..use_tree_start + use_tree_end + 2];

	assert!(use_tree.contains("self"));
	assert!(use_tree.contains("Result"));
	assert!(!rewritten.contains("use color_eyre::eyre;"));
}

#[test]
fn import008_skips_ambiguous_type_symbol_paths() {
	let text = r#"
fn left<'e, E>(_exec: E)
where
	E: foo::Executor<'e>,
{
}

fn right<'e, E>(_exec: E)
where
	E: bar::Executor<'e>,
{
}
"#;
	let ctx =
		shared::read_file_context_from_text(Path::new("import008_ambiguous.rs"), text.to_owned())
			.expect("context")
			.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-008"));
	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009"));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-008"));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-009"));
}

#[test]
fn import008_keeps_lowercase_ffi_type_function_collision_fully_qualified() {
	let original = r#"
fn install(signal: i32) {
	let mut action: libc::sigaction = unsafe { std::mem::zeroed() };

	unsafe {
		libc::sigaction(signal, &raw const action, std::ptr::null_mut());
	}
}
"#;
	let path = Path::new("import008_lowercase_ffi_type_function_collision.rs");
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(path, original, true).expect("apply fix passes");

	assert!(!rewritten.contains("use libc::sigaction;"), "{rewritten}");
	assert!(rewritten.contains("action: libc::sigaction"), "{rewritten}");
	assert!(rewritten.contains("libc::sigaction(signal"), "{rewritten}");

	let ctx = shared::read_file_context_from_text(path, rewritten.clone())
		.expect("context")
		.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|violation| {
		matches!(violation.rule, "RUST-STYLE-IMPORT-008" | "RUST-STYLE-IMPORT-009")
	}));
	assert!(
		!edits
			.iter()
			.any(|edit| { matches!(edit.rule, "RUST-STYLE-IMPORT-008" | "RUST-STYLE-IMPORT-009") })
	);

	let (repeated, applied_count, _, _) =
		engine::apply_fix_passes(path, &rewritten, true).expect("repeat fix passes");

	assert_eq!(applied_count, 0);
	assert_eq!(repeated, rewritten);
}

#[test]
fn import008_merges_alias_child_import_into_existing_parent_module_use() {
	let original = r#"
use futures::channel::mpsc;

fn send(tx: mpsc::UnboundedSender<u8>) {
	let _ = tx;
}
"#;
	let mut rewritten = original.to_owned();

	for _ in 0..4 {
		let ctx = shared::read_file_context_from_text(
			Path::new("import008_merge_alias_child.rs"),
			rewritten.clone(),
		)
		.expect("context")
		.expect("has ctx");
		let (_violations, edits) = rules::collect_violations(&ctx, true);

		if edits.is_empty() {
			break;
		}

		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
	}

	assert!(rewritten.contains("use futures::channel::mpsc::{self, UnboundedSender};"));
	assert!(rewritten.contains("fn send(tx: UnboundedSender<u8>)"));
	assert!(!rewritten.contains("use mpsc::UnboundedSender;"));
}

#[test]
fn import008_merges_children_into_existing_nested_use_tree() {
	let original = r#"
use pubfi_extractor::{
	TextIndex,
	asset::{self, AssetRegistry},
	chain_object, mentions,
	product::{self, ProductRegistry},
};

fn demo(
	_i: TextIndex,
	_m: mentions::Mentions,
	a: asset::AssetMention,
	b: chain_object::ChainObjectMention,
	c: product::ProductMention,
) {
	let _ = (a, b, c);
}
"#;
	let mut rewritten = original.to_owned();

	for _ in 0..4 {
		let ctx = shared::read_file_context_from_text(
			Path::new("import008_merge_nested_use_tree.rs"),
			rewritten.clone(),
		)
		.expect("context")
		.expect("has ctx");
		let (_violations, edits) = rules::collect_violations(&ctx, true);

		if edits.is_empty() {
			break;
		}

		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
	}

	let compact = rewritten.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();

	assert!(compact.contains("asset::{"));
	assert!(compact.contains("AssetRegistry"));
	assert!(compact.contains("AssetMention"));
	assert!(compact.contains("chain_object::{self,ChainObjectMention}"), "{rewritten}");
	assert!(compact.contains("product::{"));
	assert!(compact.contains("ProductRegistry"));
	assert!(compact.contains("ProductMention"));
	assert!(compact.contains("a:AssetMention"));
	assert!(compact.contains("b:ChainObjectMention"));
	assert!(compact.contains("c:ProductMention"));
	assert!(!compact.contains("useasset::AssetMention;"));
	assert!(!compact.contains("usechain_object::ChainObjectMention;"));
	assert!(!compact.contains("useproduct::ProductMention;"));
}

#[test]
fn import008_recovers_short_child_imports_into_existing_parent_use_tree() {
	let original = r#"
use asset::AssetMention;
use chain_object::ChainObjectMention;
use product::ProductMention;

use pubfi_extractor::{
	TextIndex,
	asset::{self, AssetRegistry},
	chain_object, mentions,
	product::{self, ProductRegistry},
};

fn demo(
	_i: TextIndex,
	_m: mentions::Mentions,
	a: AssetMention,
	b: ChainObjectMention,
	c: ProductMention,
) {
	let _ = (a, b, c);
}
"#;
	let mut rewritten = original.to_owned();

	for _ in 0..4 {
		let ctx = shared::read_file_context_from_text(
			Path::new("import008_recover_short_child_use.rs"),
			rewritten.clone(),
		)
		.expect("context")
		.expect("has ctx");
		let (_violations, edits) = rules::collect_violations(&ctx, true);

		if edits.is_empty() {
			break;
		}

		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
	}

	let compact = rewritten.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();

	assert!(compact.contains("asset::{"));
	assert!(compact.contains("AssetRegistry"));
	assert!(compact.contains("AssetMention"));
	assert!(compact.contains("chain_object::{self,ChainObjectMention}"), "{rewritten}");
	assert!(compact.contains("product::{"));
	assert!(compact.contains("ProductRegistry"));
	assert!(compact.contains("ProductMention"));
	assert!(!compact.contains("useasset::AssetMention;"));
	assert!(!compact.contains("usechain_object::ChainObjectMention;"));
	assert!(!compact.contains("useproduct::ProductMention;"));
}

#[test]
fn import008_skips_cfg_test_module_paths() {
	let text = r#"
#[cfg(test)]
mod tests {
	fn sample<'e, E>(_exec: E)
	where
		E: sqlx::Executor<'e>,
	{
	}
}
"#;
	let ctx =
		shared::read_file_context_from_text(Path::new("import008_cfg_test.rs"), text.to_owned())
			.expect("context")
			.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-008"));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-008"));
}

#[test]
fn import008_prefers_imported_symbol_over_redundant_qualified_type_path() {
	let original = r#"
use shared::{Edit, Violation};

fn demo(v: Vec<shared::Violation>) -> Option<shared::Violation> {
	let _ = Edit { start: 0, end: 0, replacement: String::new(), rule: "R" };

	v.into_iter().next()
}
"#;
	let mut rewritten = original.to_owned();

	for _ in 0..4 {
		let ctx = shared::read_file_context_from_text(
			Path::new("import008_prefers_imported_symbol.rs"),
			rewritten.clone(),
		)
		.expect("context")
		.expect("has ctx");
		let (_violations, edits) = rules::collect_violations(&ctx, true);

		if edits.is_empty() {
			break;
		}

		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
	}

	assert!(rewritten.contains("Vec<Violation>"));
	assert!(rewritten.contains("Option<Violation>"));
	assert!(!rewritten.contains("Vec<shared::Violation>"));
	assert!(!rewritten.contains("Option<shared::Violation>"));
}

#[test]
fn import008_shortens_type_like_value_constructor_paths() {
	let original = r#"
fn sort(values: &mut [usize]) {
	values.sort_by_key(|value| std::cmp::Reverse(*value));
}
"#;
	let mut rewritten = original.to_owned();

	for _ in 0..4 {
		let ctx = shared::read_file_context_from_text(
			Path::new("import008_type_like_value_constructor.rs"),
			rewritten.clone(),
		)
		.expect("context")
		.expect("has ctx");
		let (_violations, edits) = rules::collect_violations(&ctx, true);

		if edits.is_empty() {
			break;
		}

		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
	}

	assert!(rewritten.contains("use std::cmp::Reverse;"), "{rewritten}");
	assert!(rewritten.contains("Reverse(*value)"), "{rewritten}");
	assert!(!rewritten.contains("std::cmp::Reverse(*value)"), "{rewritten}");
}

#[test]
fn import008_shortens_type_like_value_constants_without_primitive_constants() {
	let original = r#"
mod shared {
	pub const STYLE_RULE_IDS: [&str; 0] = [];
}

fn max_value() -> usize {
	usize::MAX
}

fn print_coverage() {
	for rule in shared::STYLE_RULE_IDS {
		println!("{rule}");
	}
}
"#;
	let mut rewritten = original.to_owned();

	for _ in 0..4 {
		let ctx = shared::read_file_context_from_text(
			Path::new("import008_type_like_value_constants.rs"),
			rewritten.clone(),
		)
		.expect("context")
		.expect("has ctx");
		let (_violations, edits) = rules::collect_violations(&ctx, true);

		if edits.is_empty() {
			break;
		}

		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
	}

	assert!(rewritten.contains("use shared::STYLE_RULE_IDS;"), "{rewritten}");
	assert!(rewritten.contains("for rule in STYLE_RULE_IDS"), "{rewritten}");
	assert!(rewritten.contains("usize::MAX"), "{rewritten}");
	assert!(!rewritten.contains("use usize::MAX"), "{rewritten}");
	assert!(!rewritten.contains("for rule in shared::STYLE_RULE_IDS"), "{rewritten}");
}

#[test]
fn import008_skips_non_importable_self_root_paths() {
	let text = r#"
trait Job {
	type Output;
	fn run(&self) -> Self::Output;
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import008_skip_self_root.rs"),
		text.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-008"));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-008"));
}

#[test]
fn import008_skips_non_importable_generic_root_paths() {
	let text = r#"
use serde::Serializer;

pub fn serialize<S>(serializer: S) -> Result<S::Ok, S::Error>
where
	S: Serializer,
{
	let _ = serializer;
	todo!()
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import008_skip_generic_root.rs"),
		text.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-008"));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-008"));
}

#[test]
fn import008_skips_std_result_alias_shortening() {
	let text = r#"
pub fn run() -> std::result::Result<(), String> {
	Ok(())
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import008_skip_std_result_alias.rs"),
		text.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-008"));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-008"));
}

#[test]
fn import008_keeps_io_result_qualified_when_unqualified_result_is_used() {
	let text = r#"
fn business_result() -> Result<(), i32> {
	Ok(())
}

fn read_state() -> std::io::Result<String> {
	Ok(String::new())
}

fn apply_test_override() -> Result<(), i32> {
	Ok(())
}
"#;
	let path = Path::new("import008_io_result_prelude_conflict.rs");
	let ctx = shared::read_file_context_from_text(path, text.to_owned())
		.expect("context")
		.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|violation| violation.rule == "RUST-STYLE-IMPORT-008"));
	assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-008"));

	let (rewritten, _, _, _) =
		engine::apply_fix_passes(path, text, true).expect("apply fix passes");

	assert!(!rewritten.contains("use std::io::Result;"), "Rewritten:\n{rewritten}");
	assert!(
		rewritten.contains("fn read_state() -> std::io::Result<String>"),
		"Rewritten:\n{rewritten}"
	);
	assert!(
		rewritten.contains("fn apply_test_override() -> Result<(), i32>"),
		"Rewritten:\n{rewritten}"
	);
}

#[test]
fn import008_shortens_qualified_derive_path_and_inserts_use() {
	let original = r#"
#[derive(Clone, Debug, sqlx::FromRow)]
struct Row;
"#;
	let mut rewritten = original.to_owned();

	for _ in 0..4 {
		let ctx = shared::read_file_context_from_text(
			Path::new("import008_derive_row.rs"),
			rewritten.clone(),
		)
		.expect("context")
		.expect("has ctx");
		let (_violations, edits) = rules::collect_violations(&ctx, true);

		if edits.is_empty() {
			break;
		}

		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
	}

	assert!(rewritten.lines().any(|line| line.trim() == "use sqlx::FromRow;"));
	assert!(rewritten.contains("#[derive(Clone, Debug, FromRow)]"));
	assert!(!rewritten.contains("#[derive(Clone, Debug, sqlx::FromRow)]"));
}

#[test]
fn import008_skips_ambiguous_derive_symbol_paths() {
	let text = r#"
#[derive(sqlx::FromRow)]
struct SqlRow;

#[derive(other::FromRow)]
struct OtherRow;
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import008_derive_ambiguous.rs"),
		text.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);
	let import008_violations =
		violations.iter().filter(|v| v.rule == "RUST-STYLE-IMPORT-008").count();
	let import008_edits = edits.iter().filter(|e| e.rule == "RUST-STYLE-IMPORT-008").count();

	assert_eq!(import008_violations, 0);
	assert_eq!(import008_edits, 0);
}

#[test]
fn import008_derive_does_not_touch_skip_serializing_if_string_literal() {
	let original = r#"
#[serde(skip_serializing_if = "core::ops::Not::not")]
#[derive(serde::Serialize)]
struct Record {
	#[allow(dead_code)]
	value: bool,
}
"#;
	let mut rewritten = original.to_owned();

	for _ in 0..4 {
		let ctx = shared::read_file_context_from_text(
			Path::new("import008_derive_skip_serializing.rs"),
			rewritten.clone(),
		)
		.expect("context")
		.expect("has ctx");
		let (_violations, edits) = rules::collect_violations(&ctx, true);

		if edits.is_empty() {
			break;
		}

		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
	}

	let skip_serializing_if_lines: Vec<&str> = rewritten
		.lines()
		.filter(|line| line.contains("skip_serializing_if"))
		.map(str::trim)
		.collect();

	assert_eq!(
		skip_serializing_if_lines.len(),
		1,
		"expected exactly one skip_serializing_if attribute"
	);
	assert_eq!(
		skip_serializing_if_lines[0],
		r#"#[serde(skip_serializing_if = "core::ops::Not::not")]"#
	);
	assert!(!rewritten.contains("use core::ops::Not;"));
	assert!(rewritten.contains("#[derive(Serialize)]"));
}

#[test]
fn import008_shortens_qualified_value_receiver_path_inside_macro_and_adds_import() {
	let original = r#"
use crate::{
	agent::tracker_tool_bridge::{DynamicToolCallResponse, DynamicToolHandler, DynamicToolSpec},
};

fn build_response() -> DynamicToolCallResponse {
	let _handler = None::<DynamicToolHandler>;
	let _spec = None::<DynamicToolSpec>;

	DynamicToolCallResponse {
		content_items: vec![
			crate::agent::tracker_tool_bridge::DynamicToolContentItem::InputText {
				text: String::from("boom"),
			},
		],
		success: false,
	}
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import008_value_receiver_nested_use.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(
		rewritten
			.lines()
			.any(|line| line.trim()
				== "use crate::agent::tracker_tool_bridge::DynamicToolContentItem;"),
		"{rewritten}"
	);
	assert!(rewritten.contains("DynamicToolContentItem::InputText{"), "{rewritten}");
	assert!(
		!rewritten.contains("crate::agent::tracker_tool_bridge::DynamicToolContentItem::InputText")
	);
}

#[test]
fn import008_ignores_large_macro_noise_without_record_receiver_paths() {
	let entries = (0..128)
		.map(|idx| {
			format!("Noise {{ label: helper::module::name({idx}), count: helper::count({idx}) }}")
		})
		.collect::<Vec<_>>()
		.join(", ");
	let original = format!(
		r#"
fn build_noise() {{
	let _ = vec![{entries}];
}}
"#
	);
	let ctx = shared::read_file_context_from_text(Path::new("import008_macro_noise.rs"), original)
		.expect("context")
		.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-008"));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-008"));
}

#[test]
fn import011_converges_after_import008_shortens_derive_paths() {
	let original = r#"
#[derive(sqlx::FromRow, Debug, Clone)]
struct Row;
"#;
	let (rewritten, applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(Path::new("import011_after_import008.rs"), original, true)
			.expect("apply fix passes");

	assert!(applied_count > 0, "Rewritten:\n{rewritten}");
	assert!(rewritten.lines().any(|line| line.trim() == "use sqlx::FromRow;"));
	assert!(rewritten.contains("#[derive(Clone, Debug, FromRow)]"));
}

#[test]
fn import009_fix_is_not_undone_by_import008_for_result_symbols() {
	let original = r#"
use color_eyre::Result;

fn display() -> std::fmt::Result {
	Ok(())
}

fn run() -> Result<()> {
	Ok(())
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(Path::new("import009_import008_result_cycle.rs"), original, true)
			.expect("apply fix passes");

	assert!(!rewritten.contains("use color_eyre::Result;"));
	assert!(rewritten.contains("fn display() -> std::fmt::Result"));
	assert!(rewritten.contains("fn run() -> color_eyre::Result<()>"));

	let (
		repeated,
		_repeated_applied_count,
		_repeated_had_import_shortening_edits,
		_repeated_had_let_mut_reorder_edits,
	) = engine::apply_fix_passes(
		Path::new("import009_import008_result_cycle.rs"),
		&rewritten,
		true,
	)
	.expect("repeat apply fix passes");

	assert_eq!(repeated.trim_start_matches('\n'), rewritten.trim_start_matches('\n'));
}

#[test]
fn import008_shortens_grouped_pubfi_search_error_receiver_when_import_exists() {
	let original = r#"
use pubfi_search::{
	client::SearchClient,
	query::SearchQuery,
};
use pubfi_search::Error;

struct SearchFailure {
	source: Error,
}

fn normalize_error(input: Error, query: SearchQuery, client: SearchClient) -> Error {
	let _ = input;
	let _ = query;
	let _ = client;
	pubfi_search::Error::NotFound
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import009_import008_pubfi_search_error_cycle.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(rewritten.contains("use pubfi_search::Error;"), "{rewritten}");
	assert!(rewritten.contains("source: Error"), "Rewritten:\n{rewritten}");
	assert!(
		rewritten.contains(
			"fn normalize_error(input: Error, query: SearchQuery, client: SearchClient) -> Error"
		),
		"Rewritten:\n{rewritten}"
	);
	assert!(rewritten.contains("Error::NotFound"), "Rewritten:\n{rewritten}");

	let (
		repeated,
		repeated_applied_count,
		_repeated_had_import_shortening_edits,
		_repeated_had_let_mut_reorder_edits,
	) = engine::apply_fix_passes(
		Path::new("import009_import008_pubfi_search_error_cycle.rs"),
		&rewritten,
		true,
	)
	.expect("repeat apply fix passes");

	assert_eq!(repeated_applied_count, 0);
	assert_eq!(repeated.trim_start_matches('\n'), rewritten.trim_start_matches('\n'));
}

#[test]
fn import008_shortens_serde_value_receivers_when_import_exists() {
	let original = r#"
use std::collections::HashMap;

use serde_json::Value;

fn build_payload() -> Value {
	serde_json::Value::String("ok".to_string())
}

fn capture(mut map: HashMap<String, Value>) {
	map.insert("kind".to_string(), serde_json::Value::String("x".to_string()));
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import009_import008_serde_value_cycle.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(rewritten.contains("use serde_json::Value;"), "Rewritten:\n{rewritten}");
	assert!(rewritten.contains("fn build_payload() -> Value"), "Rewritten:\n{rewritten}");
	assert!(
		rewritten.contains("fn capture(mut map: HashMap<String, Value>)"),
		"Rewritten:\n{rewritten}"
	);
	assert!(rewritten.contains("Value::String(\"x\".to_string())"), "Rewritten:\n{rewritten}");

	let (
		repeated,
		repeated_applied_count,
		_repeated_had_import_shortening_edits,
		_repeated_had_let_mut_reorder_edits,
	) = engine::apply_fix_passes(
		Path::new("import009_import008_serde_value_cycle.rs"),
		&rewritten,
		true,
	)
	.expect("repeat apply fix passes");

	assert_eq!(repeated_applied_count, 0);
	assert_eq!(repeated.trim_start_matches('\n'), rewritten.trim_start_matches('\n'));
}

#[test]
fn shortening_preserves_receiver_generics_and_nested_type_edits() {
	let source = r"pub fn value() {
    let value = std::mem::MaybeUninit::<std::ffi::c_int>::uninit();
    let values = std::iter::Iterator::collect::<std::vec::Vec<std::ffi::c_int>>(0..3);
    let _ = (value, values);
}";
	let root = env::temp_dir().join(format!("vstyle-receiver-generics-{}", process::id()));
	let path = root.join("lib.rs");

	fs::create_dir_all(&root).expect("Create fixture directory.");

	let (rewritten, applied, _, _) =
		engine::apply_fix_passes(&path, source, true).expect("Apply fixes.");

	assert!(applied > 0);
	assert!(rewritten.contains("MaybeUninit::<c_int>::uninit()"), "{rewritten}");
	assert!(rewritten.contains("Iterator::collect::<Vec<c_int>>"), "{rewritten}");

	let (repeated, _, _, _) =
		engine::apply_fix_passes(&path, &rewritten, true).expect("Repeat fixes.");

	assert_eq!(rewritten, repeated);

	fs::write(&path, &rewritten).expect("Write rewritten fixture.");

	let output = Command::new("rustc")
		.args(["--edition=2024", "--crate-type=lib", "--emit=metadata"])
		.arg(&path)
		.arg("-o")
		.arg(root.join("fixture.rmeta"))
		.output()
		.expect("Compile rewritten fixture.");

	fs::remove_dir_all(root).expect("Remove fixture.");

	assert!(output.status.success(), "{rewritten}\n{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn import_shortening_merges_last_use_without_overlapping_new_imports() {
	let root = env::temp_dir().join(format!("vstyle-last-use-{}", process::id()));
	let path = root.join("last_use.rs");

	fs::create_dir_all(&root).expect("Create fixture directory.");

	for separator in ["\n", " "] {
		let original = format!(
			"use std::collections;{separator}pub struct Fixture {{ pub map: std::collections::BTreeMap<String, String>, pub path: std::path::PathBuf }}\n"
		);
		let (rewritten, applied, _, _) =
			engine::apply_fix_passes(&path, &original, true).expect("Apply fixes.");

		assert!(applied > 0, "{rewritten}");
		assert!(rewritten.contains("pub map: BTreeMap<String, String>"), "{rewritten}");
		assert!(rewritten.contains("pub path: PathBuf"), "{rewritten}");

		let ctx = shared::read_file_context_from_text(&path, rewritten.clone())
			.expect("Parse output.")
			.expect("Output context.");
		let (violations, _) = rules::collect_violations(&ctx, false);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-008"), "{violations:?}");

		fs::write(&path, &rewritten).expect("Write rewritten fixture.");

		let compiled = Command::new("rustc")
			.args(["--edition=2024", "--crate-type=lib", "--emit=metadata"])
			.arg(&path)
			.arg("-o")
			.arg(root.join("fixture.rmeta"))
			.output()
			.expect("Compile rewritten fixture.");

		assert!(
			compiled.status.success(),
			"{rewritten}\n{}",
			String::from_utf8_lossy(&compiled.stderr)
		);
	}

	fs::remove_dir_all(root).expect("Remove fixture.");
}

#[test]
fn import_shortening_keeps_shared_guarded_type_paths_consistent() {
	let root = env::temp_dir().join(format!("vstyle-guarded-type-{}", process::id()));
	let path = root.join("guarded.rs");
	let original = "pub fn ordinary(value: std::path::PathBuf) -> usize { value.as_os_str().len() }\n#[cfg(test)]\npub fn guarded(value: std::path::PathBuf) -> usize { value.as_os_str().len() }\n#[cfg(any())]\npub fn unavailable(_: unavailable_dependency::Missing) {}\n";
	let (rewritten, _, _, _) =
		engine::apply_fix_passes(&path, original, true).expect("Apply fixes.");
	let ctx = shared::read_file_context_from_text(&path, rewritten.clone())
		.expect("Parse output.")
		.expect("Output context.");
	let (violations, _) = rules::collect_violations(&ctx, false);

	assert!(rewritten.contains("ordinary(value: PathBuf)"), "{rewritten}");
	assert!(rewritten.contains("guarded(value: PathBuf)"), "{rewritten}");
	assert!(rewritten.contains("unavailable_dependency::Missing"), "{rewritten}");
	assert!(!rewritten.contains("use unavailable_dependency"), "{rewritten}");
	assert!(
		!violations
			.iter()
			.any(|v| matches!(v.rule, "RUST-STYLE-IMPORT-008" | "RUST-STYLE-IMPORT-009")),
		"{rewritten}\n{violations:?}"
	);

	let (repeated, _, _, _) =
		engine::apply_fix_passes(&path, &rewritten, true).expect("Repeat fixes.");

	assert_eq!(rewritten, repeated);

	fs::create_dir_all(&root).expect("Create fixture directory.");
	fs::write(&path, &rewritten).expect("Write fixture.");

	for configuration in [None, Some("test")] {
		let mut command = Command::new("rustc");

		command.args(["--edition=2024", "--crate-type=lib", "--emit=metadata"]);

		if let Some(configuration) = configuration {
			command.args(["--cfg", configuration]);
		}

		let compiled = command
			.arg(&path)
			.arg("-o")
			.arg(root.join("fixture.rmeta"))
			.output()
			.expect("Compile fixture.");

		assert!(
			compiled.status.success(),
			"{rewritten}\n{}",
			String::from_utf8_lossy(&compiled.stderr)
		);
	}

	fs::remove_dir_all(root).expect("Remove fixture.");
}
