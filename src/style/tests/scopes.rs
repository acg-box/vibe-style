use std::{
	env, fs,
	path::Path,
	process::{self, Command},
	time::{SystemTime, UNIX_EPOCH},
};

use ra_ap_syntax::{AstNode as _, Edition, SourceFile, ast::Use};

use crate::style::{engine, fixes, imports, rules, shared};

#[test]
fn import007_reports_non_fixable_glob_without_safe_expansion() {
	let original = r#"
use crate::prelude::*;
use sqlx::*;
use std::collections::HashMap;

fn run() {
	let _ = HashMap::<u8, u8>::new();
}
"#;
	let ctx =
		shared::read_file_context_from_text(Path::new("import010_glob.rs"), original.to_owned())
			.expect("context")
			.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-007" && !v.fixable));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-007"));

	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(rewritten.contains("use crate::prelude::*;"));
	assert!(rewritten.contains("use sqlx::*;"));
	assert!(rewritten.contains("use std::collections::HashMap;"));
}

#[test]
fn import007_fixes_super_glob_when_used_symbols_can_be_resolved() {
	let original = r#"
fn helper() {}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn sample_case() {
		helper();
	}
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import010_nested_glob.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-007" && v.fixable));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-007"));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-MOD-007"));

	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(!rewritten.contains("use super::*;"));
	assert!(rewritten.contains("use super::{"));
	assert!(rewritten.contains("helper"));
}

#[test]
fn import007_super_glob_fix_ignores_symbol_names_only_mentioned_in_strings() {
	let original = r#"
pub struct ChunkingConfig {
	pub max_tokens: u32,
	pub overlap_tokens: u32,
}
pub struct Tokenizer;

pub fn load_tokenizer(_name: &str) -> Result<Tokenizer, String> {
	Ok(Tokenizer)
}
pub fn split_text(_text: &str, _cfg: &ChunkingConfig, _tokenizer: &Tokenizer) -> Vec<String> {
	Vec::new()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn splits_into_chunks_with_overlap() {
		let cfg = ChunkingConfig { max_tokens: 10, overlap_tokens: 2 };
		let tokenizer = load_tokenizer("demo").expect("Tokenizer loading should succeed.");
		let chunks = split_text("One. Two. Three. Four.", &cfg, &tokenizer);

		assert!(!chunks.is_empty() || chunks.is_empty());
	}
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import007_super_glob_string_symbol.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (_violations, _edits) = rules::collect_violations(&ctx, true);
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import007_super_glob_string_symbol.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(!rewritten.contains("use super::*;"));
	assert!(!rewritten.contains("use super::{"));
}

#[test]
fn import007_fix_expands_crate_prelude_glob_when_module_exports_are_known() {
	let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Read timestamp.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-prelude-expand-{}-{now}", process::id()));
	let src_dir = root.join("src");
	let sample_path = src_dir.join("sample.rs");
	let original = "use crate::prelude::*;\n\nfn run() -> Result<()> {\n\tOk(())\n}\n";

	fs::create_dir_all(&src_dir).expect("Create temp src directory.");
	fs::write(
		root.join("Cargo.toml"),
		"[package]\nname = \"vstyle-prelude-expand\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
	)
	.expect("Write Cargo manifest.");
	fs::write(
		root.join("src/main.rs"),
		"mod prelude {\n\tpub use color_eyre::{Result, eyre};\n}\n",
	)
	.expect("Write crate root.");
	fs::write(&sample_path, original).expect("Write sample source.");

	let ctx =
		shared::read_file_context(&sample_path).expect("Read context.").expect("Have context.");
	let (_violations, edits) = rules::collect_violations(&ctx, true);
	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("Apply edits.");

	assert!(rewritten.contains("use crate::prelude::{Result};"));
	assert!(!rewritten.contains("use crate::prelude::*;"));
}

#[test]
fn import007_fix_expands_crate_prelude_glob_inside_braced_use() {
	let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Read timestamp.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-prelude-braced-{}-{now}", process::id()));
	let src_dir = root.join("src");
	let sample_path = src_dir.join("sample.rs");
	let original = "use crate::{\n\tprelude::*,\n\tstyle::RunSummary,\n};\n\nfn run(summary: RunSummary) -> Result<()> {\n\tlet _ = summary;\n\tOk(())\n}\n";

	fs::create_dir_all(&src_dir).expect("Create temp src directory.");
	fs::write(
		root.join("Cargo.toml"),
		"[package]\nname = \"vstyle-prelude-braced\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
	)
	.expect("Write Cargo manifest.");
	fs::write(
		root.join("src/main.rs"),
		"mod prelude {\n\tpub use color_eyre::{Result, eyre};\n}\nmod style { pub struct RunSummary; }\n",
	)
	.expect("Write crate root.");
	fs::write(&sample_path, original).expect("Write sample source.");

	let ctx =
		shared::read_file_context(&sample_path).expect("Read context.").expect("Have context.");
	let (_violations, edits) = rules::collect_violations(&ctx, true);
	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("Apply edits.");
	let compact = rewritten.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();

	assert!(compact.contains("usecrate::{"));
	assert!(compact.contains("prelude::{Result}"));
	assert!(compact.contains("style::RunSummary"));
	assert!(!compact.contains("prelude::*"));
}

#[test]
fn import007_fix_preserves_cfg_attribute_when_expanding_pub_glob() {
	let original = r#"
pub mod api_code {
	#[cfg(feature = "pubfi")]
	mod pubfi {
		pub const ERR_A: i16 = -1;
		pub const ERR_B: i16 = -2;
	}

	#[cfg(feature = "pubfi")]
	pub use self::pubfi::*;
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import007_preserves_cfg_pub_glob.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-007" && v.fixable));

	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(rewritten.contains("#[cfg(feature = \"pubfi\")]"));
	assert!(rewritten.contains("pub use self::pubfi::{ERR_A, ERR_B};"));
}

#[test]
fn import007_fix_preserves_inline_cfg_attribute_when_expanding_pub_glob() {
	let original = r#"
pub mod api_code {
	#[cfg(feature = "pubfi")] mod pubfi {
		pub const ERR_A: i16 = -1;
		pub const ERR_B: i16 = -2;
	}

	#[cfg(feature = "pubfi")] pub use self::pubfi::*;
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import007_preserves_inline_cfg_pub_glob.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-007" && v.fixable));

	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(rewritten.contains("#[cfg(feature = \"pubfi\")] pub use self::pubfi::{ERR_A, ERR_B};"));
}

#[test]
fn import007_fix_expands_long_pub_glob_without_custom_layout() {
	let original = r#"
pub mod api_code {
	#[cfg(feature = "pubfi")]
	mod pubfi {
		pub const ERR_000: i16 = -1;
		pub const ERR_001: i16 = -2;
		pub const ERR_002: i16 = -3;
		pub const ERR_003: i16 = -4;
		pub const ERR_004: i16 = -5;
		pub const ERR_005: i16 = -6;
		pub const ERR_006: i16 = -7;
		pub const ERR_007: i16 = -8;
		pub const ERR_008: i16 = -9;
		pub const ERR_009: i16 = -10;
		pub const ERR_010: i16 = -11;
		pub const ERR_011: i16 = -12;
	}

	#[cfg(feature = "pubfi")]
	pub use self::pubfi::*;
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import007_long_pub_glob_multiline.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-007" && v.fixable));

	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(rewritten.contains("pub use self::pubfi::{ERR_000"));
	assert!(rewritten.contains("ERR_011};"));
	assert!(!rewritten.contains("pub use self::pubfi::*;"));
}

#[test]
fn import007_fix_rewrites_rayon_prelude_glob_to_traits() {
	let original = "use rayon::prelude::*;\n\nfn sample(batch: &[usize]) {\n\tlet _ = batch.par_iter().map(|value| value + 1).count();\n}\n";
	let ctx =
		shared::read_file_context_from_text(Path::new("import007_rayon.rs"), original.to_owned())
			.expect("context")
			.expect("has ctx");
	let (_violations, edits) = rules::collect_violations(&ctx, true);
	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(!rewritten.contains("use rayon::prelude::*;"));
	assert!(rewritten.contains("use rayon::iter::{"));
	assert!(rewritten.contains("IntoParallelRefIterator"));
	assert!(rewritten.contains("ParallelIterator"));
}

#[test]
fn import010_fix_rewrites_top_level_super_import_to_crate_absolute() {
	let original = "use super::shared::Edit;\n\nfn run(edit: Edit) {\n\tlet _ = edit;\n}\n";
	let ctx =
		shared::read_file_context_from_text(Path::new("src/style/imports.rs"), original.to_owned())
			.expect("context")
			.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-010" && v.fixable));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-010"));

	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(rewritten.contains("use crate::style::shared::Edit;"));
	assert!(!rewritten.contains("use super::shared::Edit;"));
}

#[test]
fn import007_preserves_parent_glob_with_inherited_symbols() {
	let text = "use std::io::*;\nstruct Marker;\n#[cfg(test)] mod tests { use super::*; fn run() { let _: Option<Error> = None; let _ = Marker; } }";
	let ctx = shared::read_file_context_from_text(Path::new("inherited_glob.rs"), text.to_owned())
		.expect("Read context.")
		.expect("Have context.");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-007"));
	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-007" && !v.fixable));
}

#[test]
fn import010_resolves_binary_entrypoint_as_crate_root() {
	let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Timestamp.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-bin-owner-{}-{now}", process::id()));
	let bin = root.join("src/bin");

	fs::create_dir_all(&bin).expect("Create binary directory.");
	fs::write(
		root.join("Cargo.toml"),
		"[package]\nname = \"bin-owner-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
	)
	.expect("Write manifest.");

	let path = bin.join("tool.rs");
	let original = "const VALUE: u32 = 1;\nfn main() {}\n#[cfg(test)] mod tests { use super::VALUE; fn value() -> u32 { VALUE } }";

	fs::write(&path, original).expect("Write binary.");

	let ctx = shared::read_file_context(&path).expect("Read context.").expect("Have context.");
	let (_, edits) = rules::collect_violations(&ctx, true);
	let edits = edits.into_iter().filter(|edit| edit.rule == "RUST-STYLE-IMPORT-010").collect();
	let mut rewritten = original.to_owned();

	fixes::apply_edits(&mut rewritten, edits).expect("Rewrite parent path.");

	assert!(rewritten.contains("use crate::VALUE;"), "{rewritten}");

	fs::remove_dir_all(root).expect("Remove fixture.");
}

#[test]
fn import010_respects_path_attribute_module_ownership() {
	let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Read timestamp.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-path-module-{}-{now}", process::id()));
	let src = root.join("src");
	let child = src.join("migration_tests.rs");
	let original = "use super::APPLICATION_ID;\nfn fixture() -> u32 { APPLICATION_ID }\n";

	fs::create_dir_all(&src).expect("Create source directory.");
	fs::write(
		root.join("Cargo.toml"),
		"[package]\nname = \"path-module-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
	)
	.expect("Write manifest.");
	fs::write(src.join("lib.rs"), "mod migrations;\n").expect("Write root.");
	fs::write(
		src.join("migrations.rs"),
		"const APPLICATION_ID: u32 = 1;\n#[cfg(test)]\n#[path = \"migration_tests.rs\"]\nmod tests;\n",
	)
	.expect("Write parent.");
	fs::write(&child, original).expect("Write child.");

	let ctx = shared::read_file_context(&child).expect("Read context.").expect("Have context.");
	let (_, edits) = rules::collect_violations(&ctx, true);
	let edits = edits.into_iter().filter(|edit| edit.rule == "RUST-STYLE-IMPORT-010").collect();
	let mut rewritten = original.to_owned();

	fixes::apply_edits(&mut rewritten, edits).expect("Apply path rewrite.");

	assert!(rewritten.contains("use crate::migrations::APPLICATION_ID;"), "{rewritten}");

	fs::write(
		src.join("lib.rs"),
		"mod migrations;\n#[path = \"migration_tests.rs\"]\nmod alternate;\n",
	)
	.expect("Write ambiguous ownership.");

	let ctx = shared::read_file_context(&child).expect("Refresh context.").expect("Have context.");
	let (_, edits) = rules::collect_violations(&ctx, true);

	assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-010"));

	fs::remove_dir_all(root).expect("Remove fixture.");
}

#[test]
fn import010_preserves_aliases_and_nested_parent_paths() {
	for (original, expected) in [
		("use super::{Kind as K};", "Kind as K"),
		("use super :: Kind as K;", "crate::parent::Kind as K"),
		("use super::{super::Root, Kind};", "crate::Root"),
	] {
		let ctx = shared::read_file_context_from_text(
			Path::new("src/parent/child.rs"),
			original.to_owned(),
		)
		.expect("Read context.")
		.expect("Have context.");
		let (_, edits) = rules::collect_violations(&ctx, true);
		let edits = edits.into_iter().filter(|edit| edit.rule == "RUST-STYLE-IMPORT-010").collect();
		let mut rewritten = original.to_owned();

		fixes::apply_edits(&mut rewritten, edits).expect("Apply import rewrite.");

		assert!(rewritten.contains(expected), "{rewritten}");
		assert!(!rewritten.contains("::super"), "{rewritten}");
	}
}

#[test]
fn import010_does_not_report_self_prefix_use() {
	let original = r#"
pub mod api_code {
	#[cfg(feature = "pubfi")]
	pub use self::pubfi::{ERR_A, ERR_B};
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import010_self_prefix.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-010"));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-010"));

	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert_eq!(rewritten.trim_start_matches('\n'), original.trim_start_matches('\n'));
}

#[test]
fn fix_passes_rewrite_cfg_pub_use_glob_and_keep_macro_block_order() {
	let original = r#"
#[cfg(feature = "pubfi")]
mod pubfi {
	def_api_codes! {
		ERR_A = -1,
		ERR_B = -2,
	}
}

def_api_codes! {
	OUTSIDE_A = -3,
}

#[cfg(feature = "pubfi")]
pub use self::pubfi::*;

def_api_codes! {
	OUTSIDE_B = -4,
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(Path::new("import007_mod001_macro_block.rs"), original, true)
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
fn import010_fix_rewrites_nested_super_chain_to_crate_absolute() {
	let original = r#"
mod inner {
	use super::super::bar;

	fn run(item: bar::Baz) {
		let _ = item;
	}
}
"#;
	let ctx =
		shared::read_file_context_from_text(Path::new("src/style/foo.rs"), original.to_owned())
			.expect("context")
			.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-010" && v.fixable));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-010"));

	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(rewritten.contains("use crate::style::bar;"));
	assert!(!rewritten.contains("use super::super::bar;"));
}

#[test]
fn import010_reports_non_fixable_when_super_depth_exceeds_module_depth() {
	let text = "use super::shared::Edit;\n";
	let ctx = shared::read_file_context_from_text(Path::new("src/lib.rs"), text.to_owned())
		.expect("context")
		.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-010" && !v.fixable));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-010"));
}

#[test]
fn import006_allows_file_top_level_use() {
	let text = r#"
use std::collections::BTreeMap;

fn sample() {
	let _ = BTreeMap::<u8, u8>::new();
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import006_file_top_level.rs"),
		text.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-006"));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-006"));
}

#[test]
fn import006_allows_inline_module_top_level_use() {
	let text = r#"
mod nested {
	use std::collections::BTreeMap;

	fn sample() {
		let _ = BTreeMap::<u8, u8>::new();
	}
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import006_inline_module_top_level.rs"),
		text.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-006"));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-006"));
}

#[test]
fn import006_reports_function_local_use() {
	let text = r#"
fn sample() {
	use std::collections::BTreeMap;

	let _ = BTreeMap::<u8, u8>::new();
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import006_function_local_violation.rs"),
		text.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-006"
			&& v.message == "Use items must appear only at file top level or module top level."
			&& v.fixable
	}));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-006"));
}

#[test]
fn import006_reports_nested_block_local_use() {
	let text = r#"
fn sample() {
	if true {
		use std::collections::BTreeMap;

		let _ = BTreeMap::<u8, u8>::new();
	}
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import006_nested_block_violation.rs"),
		text.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-006"
			&& v.message == "Use items must appear only at file top level or module top level."
			&& v.fixable
	}));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-006"));
}

#[test]
fn import006_fix_hoists_simple_function_local_use_to_file_scope() {
	let original = r#"
fn sample() {
	use std::collections::BTreeMap;

	let _ = BTreeMap::<u8, u8>::new();
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(Path::new("import006_file_scope_fix.rs"), original, true)
			.expect("apply fix passes");

	assert!(rewritten.contains("use std::collections::BTreeMap;"));
	assert!(rewritten.contains("use std::collections::BTreeMap;\n\nfn sample()"));
	assert!(!rewritten.contains("\n\tuse std::collections::BTreeMap;\n"));
}

#[test]
fn import006_fix_hoists_simple_function_local_use_to_enclosing_module_scope() {
	let original = r#"
mod nested {
	fn sample() {
		use std::collections::BTreeMap;

		let _ = BTreeMap::<u8, u8>::new();
	}
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(Path::new("import006_module_scope_fix.rs"), original, true)
			.expect("apply fix passes");

	assert!(rewritten.contains("mod nested {\n\tuse std::collections::BTreeMap;"), "{rewritten}");
	assert!(rewritten.contains("\n\tfn sample()"));
	assert!(!rewritten.contains("\n\t\tuse std::collections::BTreeMap;\n"));
}

#[test]
fn import006_fix_deletes_local_use_when_equivalent_module_scope_import_already_exists() {
	let original = r#"
mod nested {
	use std::collections::BTreeMap;

	fn sample() {
		use std::collections::BTreeMap;

		let _ = BTreeMap::<u8, u8>::new();
	}
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import006_delete_duplicate_local_use.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert_eq!(rewritten.matches("use std::collections::BTreeMap;").count(), 1);
	assert!(!rewritten.contains("\n\t\tuse std::collections::BTreeMap;\n"));
}

#[test]
fn import006_cfg_gated_function_local_use_is_reported_non_fixable() {
	let text = r#"
#[cfg(feature = "cli")]
fn sample() {
	use std::collections::BTreeMap;

	let _ = BTreeMap::<u8, u8>::new();
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import006_cfg_gated_local_use.rs"),
		text.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-006"
			&& v.message == "Use items must appear only at file top level or module top level."
			&& !v.fixable
	}));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-006"));
}

#[test]
fn crate_globs_preserve_child_trait_bindings() {
	let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Timestamp.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-child-trait-{}-{now}", process::id()));
	let source = root.join("src");
	let consumer = source.join("consumer.rs");

	fs::create_dir_all(&source).expect("Create fixture.");
	fs::write(
		root.join("Cargo.toml"),
		"[package]\nname = \"child-trait-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
	)
	.expect("Write manifest.");
	fs::write(source.join("lib.rs"), "mod consumer;\n").expect("Write root.");

	let original = r#"use std::future::Future;

pub async fn ready() -> u8 {
    9
}

#[cfg(test)]
mod tests {
    use crate::consumer::*;

    #[test]
    fn preserved() {
        let mut value = std::pin::pin!(ready());
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert_eq!(value.as_mut().poll(&mut context), std::task::Poll::Ready(9));
    }
}
"#;

	fs::write(&consumer, original).expect("Write consumer.");

	let (rewritten, _, _, _) =
		engine::apply_fix_passes(&consumer, original, true).expect("Fix imports.");

	assert!(!rewritten.contains("::*"), "{rewritten}");

	fs::write(&consumer, &rewritten).expect("Write fixed consumer.");

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
		"{rewritten}\n{}",
		String::from_utf8_lossy(&compiled.stderr)
	);

	let executed = Command::new(&binary).output().expect("Run fixture.");

	assert!(
		executed.status.success(),
		"{}\n{}",
		String::from_utf8_lossy(&executed.stdout),
		String::from_utf8_lossy(&executed.stderr)
	);
	assert!(String::from_utf8_lossy(&executed.stdout).contains("1 passed;"));

	fs::remove_dir_all(root).expect("Remove fixture.");
}

#[test]
fn crate_globs_preserve_inline_macro_references() {
	let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Timestamp.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-current-glob-{}-{now}", process::id()));
	let source = root.join("src");
	let consumer = source.join("consumer.rs");

	fs::create_dir_all(&source).expect("Create fixture.");
	fs::write(
		root.join("Cargo.toml"),
		"[package]\nname = \"current-glob-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
	)
	.expect("Write manifest.");
	fs::write(source.join("lib.rs"), "mod provider; mod consumer;\n").expect("Write root.");
	fs::write(source.join("provider.rs"), "pub struct Token;\npub fn compute() -> u8 { 9 }\n")
		.expect("Write provider.");

	let original = r#"use crate::provider::{Token, compute};

pub fn run(_: Token) -> u8 {
    compute()
}

#[cfg(test)]
mod tests {
    use crate::consumer::*;

    #[test]
    fn preserved() {
        assert_eq!(compute(), 9);
        assert_eq!(run(Token), 9);
    }
}
"#;

	fs::write(&consumer, original).expect("Write consumer.");

	let (rewritten, _, _, _) =
		engine::apply_fix_passes(&consumer, original, true).expect("Fix imports.");

	assert!(!rewritten.contains("::*"), "{rewritten}");

	fs::write(&consumer, &rewritten).expect("Write fixed consumer.");

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
		"{rewritten}\n{}",
		String::from_utf8_lossy(&compiled.stderr)
	);

	let executed = Command::new(&binary).output().expect("Run fixture.");

	assert!(
		executed.status.success(),
		"{}\n{}",
		String::from_utf8_lossy(&executed.stdout),
		String::from_utf8_lossy(&executed.stderr)
	);
	assert!(String::from_utf8_lossy(&executed.stdout).contains("1 passed;"));

	fs::remove_dir_all(root).expect("Remove fixture.");
}

#[test]
fn mixed_crate_globs_preserve_explicit_sibling_bindings() {
	let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Timestamp.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-mixed-glob-{}-{now}", process::id()));
	let source = root.join("src");
	let consumer = source.join("consumer.rs");

	fs::create_dir_all(&source).expect("Create fixture.");
	fs::write(
		root.join("Cargo.toml"),
		"[package]\nname = \"mixed-glob-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
	)
	.expect("Write manifest.");
	fs::write(source.join("lib.rs"), "mod provider; mod other; mod consumer;")
		.expect("Write root.");
	fs::write(
		source.join("provider.rs"),
		"pub struct Token; pub struct Choice; impl Choice { pub const VALUE: u8 = 1; }",
	)
	.expect("Write provider.");
	fs::write(
		source.join("other.rs"),
		"pub struct Choice; impl Choice { pub const VALUE: u8 = 9; } #[cfg(test)] pub struct Extra;",
	)
	.expect("Write explicit sibling.");

	for (import, choice, conditional) in [
		("use crate::{provider::*, other::{Choice, Extra}};", "Choice", false),
		("use {crate::provider::*, crate::other::{self, Choice, Extra}};", "Choice", false),
		(
			"#[cfg(test)] use {crate::provider::*, crate::other::{Choice as Pick, Extra}, std::io::Read as _};",
			"Pick",
			true,
		),
	] {
		let read = if conditional {
			"assert_eq!(std::io::empty().read(&mut []).unwrap(), 0);"
		} else {
			""
		};
		let original = format!(
			"{import}\n\n#[test]\nfn preserved() {{ let _ = Token; let _ = Extra; assert_eq!({choice}::VALUE, 9); {read} }}\n"
		);

		fs::write(&consumer, &original).expect("Write consumer.");

		let (rewritten, _, _, _) =
			engine::apply_fix_passes(&consumer, &original, true).expect("Fix mixed glob.");

		assert!(!rewritten.contains("::*"), "{rewritten}");

		fs::write(&consumer, &rewritten).expect("Write fixed consumer.");

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
			"{rewritten}\n{}",
			String::from_utf8_lossy(&compiled.stderr)
		);

		if conditional {
			let library = Command::new("rustc")
				.args(["--edition=2024", "--crate-type=lib"])
				.arg(source.join("lib.rs"))
				.arg("-o")
				.arg(root.join("fixture.rlib"))
				.output()
				.expect("Compile without test configuration.");

			assert!(
				library.status.success(),
				"{rewritten}\n{}",
				String::from_utf8_lossy(&library.stderr)
			);
		}

		let executed = Command::new(&binary).output().expect("Run fixture.");

		assert!(
			executed.status.success(),
			"{rewritten}\n{}\n{}",
			String::from_utf8_lossy(&executed.stdout),
			String::from_utf8_lossy(&executed.stderr)
		);
		assert!(String::from_utf8_lossy(&executed.stdout).contains("1 passed;"));
	}
	for original in [
		"pub use crate::{provider::*, other::Choice};",
		"use crate::{provider::*, /* Keep the import rationale. */ other::Choice};",
		"use crate::{provider::*, other::*};",
	] {
		let ctx = shared::read_file_context_from_text(&consumer, original.to_owned())
			.expect("Read guarded fixture.")
			.expect("Get guarded context.");
		let (_, edits) = rules::collect_violations(&ctx, true);

		assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-007"));
	}

	fs::remove_dir_all(root).expect("Remove fixture.");
}

#[test]
fn inline_glob_expansion_uses_its_own_descendant_scope() {
	let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Timestamp.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-inline-glob-{}-{now}", process::id()));
	let source = root.join("src");
	let outer = source.join("outer.rs");

	fs::create_dir_all(&source).expect("Create fixture.");
	fs::write(
		root.join("Cargo.toml"),
		"[package]\nname = \"inline-glob-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
	)
	.expect("Write manifest.");
	fs::write(source.join("lib.rs"), "mod outer;").expect("Write root.");

	let self_provider = r#"use crate::outer;

fn tone() -> u8 { 1 }

#[cfg(test)]
mod tests {
    use crate::outer::*;

    #[test]
    fn preserved() {
        assert_eq!(tone(), 1);
    }
}
"#;

	fs::write(&outer, self_provider).expect("Write self re-export provider.");

	let (rewritten, _, _, _) = engine::apply_fix_passes(&outer, self_provider, true)
		.expect("Fix self re-export consumer.");

	assert!(!rewritten.contains("::*"), "{rewritten}");

	fs::write(&outer, &rewritten).expect("Write fixed self re-export consumer.");

	let compiled = Command::new("rustc")
		.args(["--edition=2024", "--test"])
		.arg(source.join("lib.rs"))
		.arg("-o")
		.arg(root.join("fixture"))
		.output()
		.expect("Compile self re-export fixture.");

	assert!(
		compiled.status.success(),
		"{rewritten}\n{}",
		String::from_utf8_lossy(&compiled.stderr)
	);

	let executed =
		Command::new(root.join("fixture")).output().expect("Run self re-export fixture.");

	assert!(executed.status.success(), "{}", String::from_utf8_lossy(&executed.stderr));
	assert!(String::from_utf8_lossy(&executed.stdout).contains("1 passed;"));

	fs::remove_dir_all(root).expect("Remove fixture.");
}

#[test]
fn crate_glob_expansion_preserves_external_traits_and_conditions() {
	let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Timestamp.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-crate-glob-{}-{now}", process::id()));
	let source = root.join("src");
	let consumer = source.join("consumer.rs");
	let original = "use crate::tree::*;\n#[cfg(feature = \"extra\")] fn conditional() { let _ = Conditional; }\n#[test] fn methods() { assert_eq!(helper().shifted(), 4); assert_eq!(shadowed(), 7); assert_eq!(theme::VALUE, 8); }\nfn shadowed() -> u8 { 7 }\n";

	fs::create_dir_all(&source).expect("Create fixture.");
	fs::write(
		root.join("Cargo.toml"),
		"[package]\nname = \"crate-glob-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
	)
	.expect("Write manifest.");
	fs::write(
		source.join("lib.rs"),
		"mod theme { pub const VALUE: u8 = 8; } #[path = \"owner.rs\"] mod tree;",
	)
	.expect("Write root.");
	fs::write(
		source.join("owner.rs"),
		r#"mod motion {
    pub trait SmoothControl { fn shifted(self) -> Self; }
    impl SmoothControl for u8 { fn shifted(self) -> Self { self + 1 } }
}
use crate::tree::motion::SmoothControl;
use crate::theme::{self, VALUE};
#[cfg(feature = "extra")] struct Conditional;
fn helper() -> u8 { 3 }
fn shadowed() -> u8 { 0 }
#[path = "consumer.rs"] mod child;
"#,
	)
	.expect("Write owner.");
	fs::write(&consumer, original).expect("Write consumer.");

	let (rewritten, _, _, _) =
		engine::apply_fix_passes(&consumer, original, true).expect("Fix consumer.");

	assert!(!rewritten.contains("::*"), "{rewritten}");

	fs::write(&consumer, &rewritten).expect("Write fixed consumer.");

	for extra in [false, true] {
		let binary = root.join("fixture");
		let mut command = Command::new("rustc");

		command
			.args(["--edition=2024", "--test"])
			.arg(source.join("lib.rs"))
			.arg("-o")
			.arg(&binary);

		if extra {
			command.args(["--cfg", "feature=\"extra\""]);
		}

		let compiled = command.output().expect("Compile fixture.");

		assert!(
			compiled.status.success(),
			"{rewritten}\n{}",
			String::from_utf8_lossy(&compiled.stderr)
		);

		let executed = Command::new(&binary).output().expect("Run fixture.");

		assert!(executed.status.success(), "{}", String::from_utf8_lossy(&executed.stderr));
		assert!(String::from_utf8_lossy(&executed.stdout).contains("1 passed;"));
	}
	for owner in [
		"#[path = \"owner.rs\"] mod tree; #[cfg(test)] #[path = \"owner.rs\"] mod tree;",
		"mod tree { pub use std::prelude::v1::*; }",
	] {
		fs::write(source.join("lib.rs"), owner).expect("Write unresolved owner.");

		let ctx = shared::read_file_context_from_text(&consumer, original.to_owned())
			.expect("Read consumer.")
			.expect("Get consumer context.");
		let (_, edits) = rules::collect_violations(&ctx, true);

		assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-007"));
	}

	fs::remove_dir_all(root).expect("Remove fixture.");
}

#[test]
fn parent_glob_expansion_preserves_conditional_bindings() {
	let original = r#"#[cfg(feature = "enabled")]
struct Conditional;
#[cfg(feature = "enabled")]
use std::io::ErrorKind;
struct Always;
#[cfg_attr(not(feature = "enabled"), cfg(any()))]
struct ViaAttr;
#[cfg_attr(feature = "enabled", cfg_attr(not(feature = "extra"), cfg(any())))]
struct Nested;
#[cfg(feature = "enabled")]
#[cfg(feature = "extra")]
struct Both;
#[cfg(feature = "enabled")]
struct Either;
#[cfg(not(feature = "enabled"))]
struct Either;
#[cfg(test)]
mod tests {
    #[cfg(test)]
    use super::*;
    #[test]
    fn unconditional() { let _ = Always; let _ = Either; }
    #[cfg(any(not(feature = "enabled"), feature = "extra"))]
    #[test]
    fn nested() { let _ = Nested; }
    #[cfg(all(feature = "enabled", feature = "extra"))]
    #[test]
    fn both() { let _ = Both; }
    #[cfg(feature = "enabled")]
    #[test]
    fn conditional() {
        let _ = Conditional;
        let _ = ViaAttr;
        let _ = ErrorKind::NotFound;
    }
}
"#;
	let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Timestamp.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-glob-cfg-{}-{now}", process::id()));
	let path = root.join("lib.rs");
	let binary = root.join("fixture");

	fs::create_dir_all(&root).expect("Create fixture directory.");

	let (rewritten, _, _, _) =
		engine::apply_fix_passes(&path, original, true).expect("Apply fixes.");

	assert!(!rewritten.contains("::*"), "{rewritten}");

	fs::write(&path, &rewritten).expect("Write rewritten fixture.");

	for (enabled, extra) in [(false, false), (false, true), (true, false), (true, true)] {
		let mut compiler = Command::new("rustc");

		compiler.args(["--edition=2024", "--test"]).arg(&path).arg("-o").arg(&binary);

		if enabled {
			compiler.args(["--cfg", "feature=\"enabled\""]);
		}
		if extra {
			compiler.args(["--cfg", "feature=\"extra\""]);
		}

		let compiled = compiler.output().expect("Compile rewritten fixture.");

		assert!(
			compiled.status.success(),
			"enabled={enabled}, extra={extra}\n{rewritten}\n{}",
			String::from_utf8_lossy(&compiled.stderr)
		);

		let executed = Command::new(&binary).output().expect("Run rewritten fixture.");

		assert!(executed.status.success(), "{}", String::from_utf8_lossy(&executed.stderr));

		let expected = if enabled && extra { 4 } else { 2 };

		assert!(String::from_utf8_lossy(&executed.stdout).contains(&format!("{expected} passed;")));
	}

	fs::remove_dir_all(root).expect("Remove fixture.");
}

#[test]
fn parent_glob_expansion_preserves_method_traits() {
	let original = r#"mod toolkit {
    pub mod prelude {
        pub trait Styled { fn size_full(self) -> Self; }
        impl Styled for u8 { fn size_full(self) -> Self { self + 1 } }
        pub trait ParentElement { fn child(self) -> Self; }
        impl ParentElement for u8 { fn child(self) -> Self { self + 2 } }
    }
}
use toolkit::prelude::{Styled, ParentElement as ChildTrait};
trait Local { fn local(self) -> Self; }
impl Local for u8 { fn local(self) -> Self { self + 3 } }
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn methods() { assert_eq!(1_u8.size_full().child().local(), 7); }
}
"#;
	let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Timestamp.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-glob-traits-{}-{now}", process::id()));
	let path = root.join("lib.rs");
	let binary = root.join("fixture");

	fs::create_dir_all(&root).expect("Create fixture directory.");

	let (rewritten, _, _, _) =
		engine::apply_fix_passes(&path, original, true).expect("Apply fixes.");

	assert!(!rewritten.contains("::*"), "{rewritten}");

	fs::write(&path, &rewritten).expect("Write rewritten fixture.");

	let compiled = Command::new("rustc")
		.args(["--edition=2024", "--test"])
		.arg(&path)
		.arg("-o")
		.arg(&binary)
		.output()
		.expect("Compile rewritten fixture.");

	assert!(
		compiled.status.success(),
		"{rewritten}\n{}",
		String::from_utf8_lossy(&compiled.stderr)
	);

	let executed = Command::new(&binary).output().expect("Run rewritten fixture.");

	assert!(executed.status.success(), "{}", String::from_utf8_lossy(&executed.stderr));
	assert!(String::from_utf8_lossy(&executed.stdout).contains("1 passed;"));

	fs::remove_dir_all(root).expect("Remove fixture.");
}

#[test]
fn parent_glob_inventory_ignores_qualified_import_leaves() {
	let original = "struct Error;\n#[cfg(test)]\nmod tests {\n\tuse super::*;\n\t#[test]\n\tfn sample() {\n\t\tuse std::fmt::{Error as Source};\n\t\tlet _ = Source;\n\t}\n}\n";
	let parsed = SourceFile::parse(original, Edition::CURRENT);
	let glob = parsed
		.tree()
		.syntax()
		.descendants()
		.filter_map(Use::cast)
		.find(|item| item.syntax().text().to_string().contains("super::*"))
		.expect("Find parent glob.");
	let symbols =
		imports::exported_symbols_from_super_scope(&glob).expect("Read parent inventory.");

	assert!(symbols.is_empty(), "{symbols:?}");
}

#[test]
fn parent_glob_expansion_retains_nested_use_dependencies() {
	let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Timestamp.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-glob-dependency-{}-{now}", process::id()));
	let path = root.join("lib.rs");

	fs::create_dir_all(&root).expect("Create fixture directory.");

	for local_import in [
		"use ErrorKind as Source;",
		"use {ErrorKind as Source};",
		"use ErrorKind::{NotFound as Selected};",
	] {
		let value = if local_import.contains("Selected") { "Selected" } else { "Source::NotFound" };
		let original = format!(
			"use std::io::ErrorKind;\n#[cfg(test)]\nmod tests {{\n\tuse super::*;\n\t#[test]\n\tfn sample() {{\n\t\t{local_import}\n\t\tlet _ = {value};\n\t}}\n}}\n"
		);
		let (rewritten, _, _, _) =
			engine::apply_fix_passes(&path, &original, true).expect("Apply fixes.");

		fs::write(&path, &rewritten).expect("Write rewritten fixture.");

		let output = Command::new("rustc")
			.args(["--edition=2024", "--test", "--emit=metadata"])
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
fn import006_preserves_conditional_expression_imports() {
	for attr in ["#[cfg(any())]", "#[cfg_attr(all(), cfg(any()))]", "#[cfg (any())]"] {
		let text = format!("fn sample() {{ {attr} {{ use missing::Feature; let _ = Feature; }} }}");
		let ctx = shared::read_file_context_from_text(Path::new("conditional_block.rs"), text)
			.expect("Read context.")
			.expect("Have context.");
		let (violations, edits) = rules::collect_violations(&ctx, true);

		assert!(
			violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-006" && !v.fixable),
			"{attr}"
		);
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-006"), "{attr}");
	}
}

#[test]
fn import006_alias_local_use_is_reported_non_fixable() {
	let text = r#"
fn sample() {
	use std::collections::BTreeMap as Map;

	let _ = Map::<u8, u8>::new();
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import006_alias_local_use.rs"),
		text.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-006"
			&& v.message == "Use items must appear only at file top level or module top level."
			&& !v.fixable
	}));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-006"));
}

#[test]
fn import006_glob_local_use_is_reported_non_fixable() {
	let text = r#"
fn sample() {
	use crate::math::*;

	let _ = 1usize;
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import006_glob_local_use.rs"),
		text.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-006"
			&& v.message == "Use items must appear only at file top level or module top level."
			&& !v.fixable
	}));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-006"));
}

#[test]
fn import006_leading_comment_local_use_is_reported_non_fixable() {
	let text = r#"
fn sample() {
	// Keep this local until cfg behavior is sorted out.
	use std::collections::BTreeMap;

	let _ = BTreeMap::<u8, u8>::new();
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import006_leading_comment_local_use.rs"),
		text.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-006"
			&& v.message == "Use items must appear only at file top level or module top level."
			&& !v.fixable
	}));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-006"));
}

#[test]
fn import007_coordinates_expansion_with_test_module_paths() {
	let original = "pub mod helper { pub fn run() {} }\n#[cfg(test)]\nmod tests {\n use super::*;\n #[test] fn calls_helper() { super::helper::run(); }\n}\n";
	let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Timestamp.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-glob-plan-{}-{now}", process::id()));
	let path = root.join("src/lib.rs");

	fs::create_dir_all(root.join("src")).expect("Create fixture directory.");
	fs::write(
		root.join("Cargo.toml"),
		"[package]\nname='glob-plan'\nversion='0.0.0'\nedition='2024'\n",
	)
	.expect("Write manifest.");
	fs::write(&path, original).expect("Write fixture.");

	let (rewritten, _, _, _) =
		engine::apply_fix_passes(&path, original, true).expect("Apply fixes.");

	assert!(!rewritten.contains("use super::*"), "{rewritten}");

	fs::write(&path, &rewritten).expect("Write rewritten fixture.");

	let output = Command::new("rustc")
		.arg("--edition=2024")
		.arg("--test")
		.arg("--emit=metadata")
		.arg(&path)
		.arg("-o")
		.arg(root.join("fixture.rmeta"))
		.output()
		.expect("Compile rewritten fixture.");

	fs::remove_dir_all(root).expect("Remove fixture.");

	assert!(output.status.success(), "{rewritten}\n{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn import006_does_not_hoist_colliding_pending_bindings() {
	let original = "fn first() {\n use std::fmt::Result;\n let _: Result = Ok(());\n}\nfn second() {\n use std::io::Result;\n let _: Result<()> = Ok(());\n}\n";
	let ctx =
		shared::read_file_context_from_text(Path::new("pending_imports.rs"), original.to_owned())
			.expect("Read context.")
			.expect("Have context.");
	let (_, edits) = rules::collect_violations(&ctx, true);
	let edits = edits.into_iter().filter(|edit| edit.rule == "RUST-STYLE-IMPORT-006").collect();
	let mut rewritten = original.to_owned();

	fixes::apply_edits(&mut rewritten, edits).expect("Apply hoists.");

	assert!(rewritten.contains(" use std::io::Result;"), "{rewritten}");
	assert!(rewritten.starts_with("use std::fmt::Result;"), "{rewritten}");
}

#[test]
fn import006_pub_use_conflict_is_reported_non_fixable() {
	let text = r#"
pub use crate::first::Thing;

fn sample() {
	use crate::second::Thing;

	let _ = 1usize;
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import006_pub_use_conflict.rs"),
		text.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-006"
			&& v.message == "Use items must appear only at file top level or module top level."
			&& !v.fixable
	}));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-006"));
}

#[test]
fn import006_keeps_cfg_test_module_top_level_use_valid() {
	let text = r#"
#[cfg(test)]
mod tests {
	use std::collections::BTreeMap;

	#[test]
	fn sample() {
		let _ = BTreeMap::<u8, u8>::new();
	}
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import006_cfg_test_module_scope.rs"),
		text.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-006"));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-006"));
}

#[test]
fn import_fixes_keep_glob_symbols_used_only_inside_macros() {
	let original = r#"
enum StoreError { Conflict }
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sample() {
        assert!(matches!(None::<StoreError>, Some(StoreError::Conflict)));
    }
}
"#;
	let (rewritten, _, _, _) =
		engine::apply_fix_passes(Path::new("src/lib.rs"), original, true).expect("Apply fixes.");

	assert!(rewritten.contains("use crate::{StoreError};"), "{rewritten}");
}
