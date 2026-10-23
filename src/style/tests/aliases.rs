use std::{
	env, fs,
	path::Path,
	process,
	time::{SystemTime, UNIX_EPOCH},
};

use crate::style::{engine, fixes, rules, shared};

#[test]
fn import003_preserves_aliases_used_by_external_children() {
	let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Timestamp.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-child-alias-{}-{now}", process::id()));
	let src = root.join("src");
	let parent = src.join("lib.rs");

	fs::create_dir_all(&src).expect("Create source directory.");
	fs::write(
		root.join("Cargo.toml"),
		"[package]\nname = \"child-alias-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
	)
	.expect("Write manifest.");
	fs::write(
		&parent,
		"use std::io::Error as State;\nmod child;\npub fn run() -> Option<State> { None }\n",
	)
	.expect("Write parent.");

	for child in [
		"use super::{self, *}; pub fn run() -> Option<State> { None }",
		"pub fn run() -> Option<super::State> { None }",
	] {
		fs::write(src.join("child.rs"), child).expect("Write child.");

		let ctx =
			shared::read_file_context(&parent).expect("Read context.").expect("Have context.");
		let (violations, edits) = rules::collect_violations(&ctx, true);

		assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-003"), "{child}");
		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-003" && !v.fixable));
	}

	fs::write(src.join("child.rs"), "pub fn run() -> Option<std::io::Error> { None }")
		.expect("Write independent child.");

	let ctx = shared::read_file_context(&parent).expect("Read context.").expect("Have context.");
	let (_, edits) = rules::collect_violations(&ctx, true);

	assert!(edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-003"));

	fs::remove_dir_all(root).expect("Remove fixture.");
}

#[test]
fn import003_fix_rewrites_trait_keep_alive_simple_use_to_as_underscore() {
	let original = r#"
use std::io::Read;

fn sample(mut data: &[u8]) -> usize {
	let mut out = [0_u8; 1];
	let _ = data.read(&mut out);
	out[0] as usize
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import003_trait_keep_alive_simple.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-003"
			&& v.message.contains("Trait keep-alive import `Read` should use `as _`")
			&& v.fixable
	}));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-003"));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(rewritten.contains("use std::io::Read as _;"));
}

#[test]
fn import003_fix_rewrites_trait_keep_alive_braced_use_to_as_underscore() {
	let original = r#"
use std::io::{Read, Write};

fn sample(mut data: &[u8], mut sink: Vec<u8>) -> usize {
	let mut out = [0_u8; 1];
	let _ = data.read(&mut out);
	let _ = sink.write(&out);
	out[0] as usize
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import003_trait_keep_alive_braced.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-003"
			&& v.message.contains("Trait keep-alive import `Read` should use `as _`")
			&& v.fixable
	}));
	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-003"
			&& v.message.contains("Trait keep-alive import `Write` should use `as _`")
			&& v.fixable
	}));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-003"));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(rewritten.contains("use std::io::{Read as _, Write as _};"));
}

#[test]
fn import003_does_not_require_as_underscore_when_trait_name_is_referenced() {
	let text = r#"
use std::io::Read;

fn read_one<R: Read>(mut reader: R) -> usize {
	let mut out = [0_u8; 1];
	let _ = reader.read(&mut out);
	out[0] as usize
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import003_trait_name_referenced.rs"),
		text.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-003"
			&& v.message.contains("Trait keep-alive import `Read` should use `as _`")
	}));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-003"));
}

#[test]
fn import003_does_not_rewrite_trait_imports_in_parent_module_with_child_decls() {
	let text = r#"
mod child;

use serde::Deserialize;
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import003_parent_module_child_decl.rs"),
		text.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-003"));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-003"));
}

#[test]
fn import003_preserves_explicit_keep_alive_with_external_child_modules() {
	for imports in [
		"use std::io::Read as _;",
		"use std::io::{Read as _, Write as _};",
		"use std::io::{Read as _, Write};",
		"#[cfg(unix)] use std::os::unix::fs::MetadataExt as _;",
	] {
		let original = format!("mod child;\n\n{imports}\n");
		let ctx = shared::read_file_context_from_text(
			Path::new("import003_explicit_keep_alive_parent.rs"),
			original,
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = rules::collect_violations(&ctx, true);

		assert!(
			!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-003"),
			"must retain explicit keep-alive and named child imports: {imports}"
		);
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-003"));
	}
}

#[test]
fn import003_restores_directly_referenced_trait_name_with_external_child_modules() {
	let original = "mod child;\n\nuse std::io::Read as _;\n\nfn sample<T: Read>() {}\n";
	let ctx = shared::read_file_context_from_text(
		Path::new("import003_referenced_trait_parent.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-003" && v.message.contains("referenced directly")
	}));

	let mut rewritten = original.to_owned();

	fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(rewritten.contains("use std::io::Read;"));
	assert!(!rewritten.contains("Read as _"));
}

#[test]
fn import003_fix_dedupes_plain_and_keep_alive_trait_imports_when_referenced() {
	let original = r#"
use serde::{Deserialize, Deserialize as _, Serialize, Serialize as _};

#[derive(Deserialize, Serialize)]
struct Payload {
	value: String,
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import003_trait_dedupe_when_referenced.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (_violations, edits) = rules::collect_violations(&ctx, true);
	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(rewritten.contains("use serde::{Deserialize"));
	assert!(rewritten.contains("Serialize"));
	assert!(!rewritten.contains("Deserialize as _"));
	assert!(!rewritten.contains("Serialize as _"));
}

#[test]
fn import003_fix_dedupes_plain_and_keep_alive_trait_imports_when_unreferenced() {
	let original = r#"
use std::io::{Read, Read as _};

fn noop() {}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import003_trait_dedupe_when_unreferenced.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (_violations, edits) = rules::collect_violations(&ctx, true);
	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(rewritten.contains("use std::io::{Read as _};"));
	assert!(!rewritten.contains("use std::io::{Read, Read as _};"));
}

#[test]
fn import003_fix_normalizes_unreferenced_trait_alias_and_avoids_ambiguous_symbol() {
	let original = r#"
use color_eyre::eyre::{Context as EyreContext, Result, eyre};
use std::task::Context;

fn run() -> Result<()> {
	let _ = eyre!("boom").wrap_err("failed");
	let _ = core::option::Option::<&Context<'_>>::None;
	Ok(())
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import003_trait_alias_context_ambiguous.rs"),
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
	assert!(!violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-003"
			&& v.message == "Import aliases are not allowed except `as _` keep-alive imports."
			&& !v.fixable
	}));
	assert!(
		edits.iter().any(|e| matches!(e.rule, "RUST-STYLE-IMPORT-003" | "RUST-STYLE-IMPORT-004"))
	);

	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(
		rewritten.contains("use color_eyre::eyre::{self, Context as EyreContext, Result};"),
		"{rewritten}"
	);
	assert!(rewritten.contains("let _ = eyre::eyre!(\"boom\").wrap_err(\"failed\");"));

	let rewritten_ctx = shared::read_file_context_from_text(
		Path::new("import003_trait_alias_context_ambiguous.rs"),
		rewritten,
	)
	.expect("context")
	.expect("has ctx");
	let (rewritten_violations, _rewritten_edits) = rules::collect_violations(&rewritten_ctx, true);

	assert!(!rewritten_violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-004"
			&& v.message
				== "Ambiguous imported symbol `Context` is not allowed; use fully qualified paths."
	}));

	let (
		stabilized,
		_second_pass_applied,
		_had_import_shortening_edits,
		_had_let_mut_reorder_edits,
	) = engine::apply_fix_passes(
		Path::new("import003_trait_alias_context_ambiguous.rs"),
		&rewritten_ctx.text,
		true,
	)
	.expect("apply fix passes");

	assert!(
		stabilized.contains("use color_eyre::eyre::{self, Context as _, Result};"),
		"{stabilized}"
	);
	assert!(!stabilized.contains("Context as EyreContext"));
}

#[test]
fn import003_alias_is_non_fixable_when_alias_identifier_is_referenced() {
	let text = r#"
use color_eyre::eyre::Context as EyreContext;

fn needs_context<T: EyreContext>(value: T) -> T {
	value
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import003_trait_alias_referenced.rs"),
		text.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-003"
			&& v.message
				== "Import alias `EyreContext` is not allowed; use a fully qualified path at use sites."
			&& v.fixable
	}));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-003"));

	let mut rewritten = text.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(!rewritten.contains("as EyreContext"));
	assert!(rewritten.contains("fn needs_context<T: color_eyre::eyre::Context>(value: T) -> T {"));
}

#[test]
fn import003_fix_rewrites_non_trait_alias_to_qualified_paths() {
	let original = r#"
use crate::error::{Error, Result};
use pubfi_ai::{AgentEmbed, Error as AiError};

fn should_retry_embed(error: &AiError) -> bool {
	matches!(error, AiError::Reqwest(_))
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import003_non_trait_alias_qualified_paths.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-003" && v.message.contains("`AiError`") && v.fixable
	}));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-003"));

	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(rewritten.contains("use pubfi_ai::{AgentEmbed};"));
	assert!(rewritten.contains("error: &pubfi_ai::Error"));
	assert!(rewritten.contains("pubfi_ai::Error::Reqwest"), "{rewritten}");
	assert!(!rewritten.contains("Error as AiError"));
}

#[test]
fn import003_fix_rewrites_nested_braced_aliases_to_qualified_paths() {
	let original = r#"
use crate::{
	grpc::{
		ReferralCode as ProtoReferralCode,
		UserData,
		gateway_service_server::GatewayService as GrpcGatewayService,
	},
};

fn map_types(
	_code: ProtoReferralCode,
	_service: GrpcGatewayService,
	_user_data: UserData,
) -> (crate::grpc::ReferralCode, crate::grpc::gateway_service_server::GatewayService) {
	todo!()
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import003_nested_braced_aliases.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(
		violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-003"
				&& v.message.contains("`ProtoReferralCode`")
				&& v.fixable
		}),
		"{violations:#?}"
	);
	assert!(
		violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-003"
				&& v.message.contains("`GrpcGatewayService`")
				&& v.fixable
		}),
		"{violations:#?}"
	);
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-003"));

	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(rewritten.contains("use crate::{grpc::{UserData}};"), "{rewritten}");
	assert!(rewritten.contains("_code: crate::grpc::ReferralCode"));
	assert!(rewritten.contains("_service: crate::grpc::gateway_service_server::GatewayService"));
	assert!(!rewritten.contains("ReferralCode as ProtoReferralCode"));
	assert!(!rewritten.contains("GatewayService as GrpcGatewayService"));
}

#[test]
fn import012_reports_unused_crate_keep_alive_import() {
	let text = r#"
use regex as _;

fn main() {}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import012_unused_crate_keep_alive.rs"),
		text.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-012"
				&& v.message
					== "Do not add crate keep-alive imports `use dep as _;` unless another path in the package uses that crate."
				&& !v.fixable
		}));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-012"));
}

#[test]
fn import012_allows_crate_keep_alive_import_with_other_crate_path_usage() {
	let text = r#"
use regex as _;

fn compile() {
	let _ = regex::Regex::new("a+");
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import012_used_crate_keep_alive.rs"),
		text.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-012"));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-012"));
}

#[test]
fn import012_allows_crate_keep_alive_import_with_cross_file_path_usage() {
	let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Read timestamp.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-import012-cross-file-{}-{now}", process::id()));
	let src_dir = root.join("src");
	let main_path = src_dir.join("main.rs");
	let helper_path = src_dir.join("helper.rs");

	fs::create_dir_all(&src_dir).expect("Create temp src directory.");
	fs::write(
			root.join("Cargo.toml"),
			"[package]\nname = \"vstyle-import012-cross-file\"\nversion = \"0.0.0\"\nedition = \"2021\"\n[dependencies]\nregex = \"1\"\n",
		)
		.expect("Write Cargo manifest.");
	fs::write(
		&main_path,
		"use regex as _;\n\nmod helper;\n\nfn main() {\n\thelper::compile();\n}\n",
	)
	.expect("Write main source.");
	fs::write(&helper_path, "pub(crate) fn compile() {\n\tlet _ = regex::Regex::new(\"a+\");\n}\n")
		.expect("Write helper source.");

	let ctx = shared::read_file_context(&main_path).expect("Read context.").expect("Have context.");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-012"));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-012"));

	let _ = fs::remove_dir_all(&root);
}

#[test]
fn import012_does_not_report_trait_keep_alive_imports() {
	let text = r#"
use std::fmt::Write as _;

fn main() {
	let mut out = String::new();
	let _ = write!(out, "ok");
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import012_trait_keep_alive.rs"),
		text.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-012"));
	assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-012"));
}

#[test]
fn import003_preserves_aliases_used_by_bare_patterns() {
	let original = "use libc::ESRCH as S;\nfn sample(code: Option<i32>) -> bool { match code { Some(S) => true, _ => false } }\n";
	let ctx = shared::read_file_context_from_text(Path::new("constant_alias.rs"), original.into())
		.expect("Read context.")
		.expect("Have context.");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-003" && !v.fixable));
	assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-003"));
}

#[test]
fn import003_preserves_function_local_alias_shadowing() {
	let original = r#"use std::io::ErrorKind as Phase;
pub fn outer() -> Phase { Phase::NotFound }
pub fn inner() -> Option<u8> {
    let value = Phase::Some(1);
    use std::option::Option as Phase;
    assert!(matches!(value, Phase::Some(_)));
    value
}
"#;
	let (rewritten, _, _, _) =
		engine::apply_fix_passes(Path::new("shadowed_alias.rs"), original, true)
			.expect("Apply fixes.");

	assert!(!rewritten.contains("ErrorKind::Some"), "{rewritten}");
	assert!(rewritten.contains("Phase::Some(1)"), "{rewritten}");
	assert!(rewritten.contains("value, Phase::Some(_)"), "{rewritten}");
	assert!(!rewritten.contains("Phase::NotFound"), "{rewritten}");
}

#[test]
fn import003_qualifies_tuple_constructor_patterns() {
	let original = r#"use std::result::Result as Receipt;
fn sample(value: Receipt<u8, u8>) -> u8 {
    if let Receipt::Ok(inner) = value { return inner; }
    let Receipt::Err(inner) = value else { return 0; };
    match value { Receipt::Ok(other) | Receipt::Err(other) => inner + other }
}
"#;
	let (rewritten, _, _, _) =
		engine::apply_fix_passes(Path::new("tuple_patterns.rs"), original, true)
			.expect("Apply fixes.");

	assert!(!rewritten.contains("Receipt"), "{rewritten}");
	assert!(rewritten.contains("::Ok(inner)"), "{rewritten}");
	assert!(rewritten.contains("::Err(inner)"), "{rewritten}");
}

#[test]
fn import003_rewrites_macro_alias_paths_without_touching_literals() {
	let original = r#"use crate::state::Status as S;
fn sample(value: crate::state::Status) {
    assert!(matches!(value, S::Ready), "S::Ready must remain literal");
}
"#;
	let (rewritten, _, _, _) =
		engine::apply_fix_passes(Path::new("macro_alias.rs"), original, true)
			.expect("Apply fixes.");

	assert!(!rewritten.contains("value, S::Ready"), "{rewritten}");
	assert!(rewritten.contains("\"S::Ready must remain literal\""), "{rewritten}");
}
