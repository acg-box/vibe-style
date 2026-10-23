use std::{
	env, fs,
	path::Path,
	process::{self, Command},
};

use crate::style::{engine, fixes, rules, shared};

#[test]
fn import_groups_preserve_root_alias_origins() {
	let workspace = env!("CARGO_PKG_NAME").replace('-', "_");

	for (standard_alias, workspace_alias) in [("_", "_"), ("standard", "workspace_alias")] {
		let text = format!(
			"use std as {standard_alias};\n\nuse anyhow::Result;\n\nuse {workspace} as {workspace_alias};\nuse crate::support::Marker;\n"
		);
		let ctx = shared::read_file_context_from_text(Path::new("root_alias_groups.rs"), text)
			.expect("context")
			.expect("has ctx");
		let (violations, _) = rules::collect_violations(&ctx, false);
		let grouping: Vec<_> = violations
			.iter()
			.filter(|v| matches!(v.rule, "RUST-STYLE-IMPORT-001" | "RUST-STYLE-IMPORT-002"))
			.collect();

		assert!(grouping.is_empty(), "{standard_alias}: {grouping:?}");
	}
}

#[test]
fn import_group_fix_normalizes_spacing_without_reordering_groups() {
	let original = r#"
use std::collections::HashSet;


use anyhow::Result;
use crate::z::Z;
"#;
	let ctx = shared::read_file_context_from_text(Path::new("import_fix.rs"), original.to_owned())
		.expect("context")
		.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

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
fn import_group_fix_applies_with_cfg_attribute_string_literal() {
	let original = r#"#[cfg(feature = "test")]
use crate::z::Z;

use std::collections::HashSet;
"#;
	let ctx =
		shared::read_file_context_from_text(Path::new("import_group_cfg.rs"), original.to_owned())
			.expect("context")
			.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.is_empty());
	assert!(edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-002"));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert_eq!(
		rewritten,
		r#"use std::collections::HashSet;

#[cfg(feature = "test")]
use crate::z::Z;
"#
	);
}

#[test]
fn pub_use_group_fix_removes_blank_lines_for_same_root() {
	let original = r#"
pub use tokenizers::Tokenizer;

pub use tokenizers::Error;

use unicode_segmentation::UnicodeSegmentation;
"#;
	let ctx =
		shared::read_file_context_from_text(Path::new("pub_use_group_fix.rs"), original.to_owned())
			.expect("context")
			.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-002" && v.fixable));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert_eq!(
		rewritten,
		r#"
pub use tokenizers::Tokenizer;
pub use tokenizers::Error;

use unicode_segmentation::UnicodeSegmentation;
"#
	);
}

#[test]
fn pub_use_spacing_preserves_visibility_boundaries() {
	for (outer, inner) in [
		("pub", "pub(crate)"),
		("pub(crate)", "pub(super)"),
		("pub(super)", "pub(in crate::scope)"),
	] {
		let original =
			format!("{outer} use external::Public;\n\n{inner} use external::Internal;\n");
		let ctx = shared::read_file_context_from_text(
			Path::new("pub_use_visibility.rs"),
			original.clone(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = rules::collect_violations(&ctx, true);
		let mut rewritten = original.clone();

		assert!(
			!violations
				.iter()
				.any(|v| { matches!(v.rule, "RUST-STYLE-IMPORT-002" | "RUST-STYLE-MOD-002") })
		);

		fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert_eq!(rewritten, original);
	}
}

#[test]
fn pub_use_spacing_repairs_each_visibility_group_and_converges() {
	let original = "pub use external::Public;\npub(crate) use external::First;\n\npub(crate) use external::Second;\n";
	let expected = "pub use external::Public;\n\npub(crate) use external::First;\npub(crate) use external::Second;\n";
	let mut rewritten = original.to_owned();

	for _ in 0..2 {
		let ctx = shared::read_file_context_from_text(
			Path::new("pub_use_convergence.rs"),
			rewritten.clone(),
		)
		.expect("context")
		.expect("has ctx");
		let (_, edits) = rules::collect_violations(&ctx, true);

		fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert_eq!(rewritten, expected);
	}
}

#[test]
fn pub_use_group_fix_converges_local_module_reexports_to_self_group() {
	let original = r#"
mod add_event;
mod add_note;

pub use add_event::{AddEventRequest, AddEventResponse};

pub use add_note::{AddNoteRequest, AddNoteResponse};
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("pub_use_local_self_group_fix.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-002"
			&& v.fixable
			&& v.message == "Prefer converging local module re-exports into `pub use self::{...};`."
	}));
	assert!(edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-002"));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
	let compact = rewritten.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();

	assert!(applied >= 1);
	assert!(compact.contains(
			"pubuseself::{add_event::{AddEventRequest,AddEventResponse},add_note::{AddNoteRequest,AddNoteResponse}};"
		));
}

#[test]
fn pub_use_group_fix_converges_pub_super_local_reexports_with_cfg_tail() {
	let original = r#"
mod cache;
mod diversity;
mod policy;
mod text;

pub(super) use cache::{build_cached_scores, hash_query};
pub(super) use diversity::{build_rerank_ranks, select_diverse_results};
pub(super) use policy::{build_policy_snapshot, resolve_scopes};
pub(super) use text::{merge_matched_fields, tokenize_query};
#[cfg(test)] pub(super) use policy::BlendSegment;
#[cfg(test)] pub(super) use text::{lexical_overlap_ratio, scope_description_boost};
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("pub_use_local_self_group_pub_super_fix.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-002"
			&& v.fixable
			&& v.message == "Prefer converging local module re-exports into `pub use self::{...};`."
	}));
	assert!(edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-002"));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
	let compact = rewritten.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();

	assert!(applied >= 1);
	assert!(compact.contains(
			"pub(super)useself::{cache::{build_cached_scores,hash_query},diversity::{build_rerank_ranks,select_diverse_results},policy::{build_policy_snapshot,resolve_scopes},text::{merge_matched_fields,tokenize_query}};"
		));
	assert!(compact.contains(
			"#[cfg(test)]pub(super)useself::{policy::BlendSegment,text::{lexical_overlap_ratio,scope_description_boost}};"
		));
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
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-001" && !v.fixable));
	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-002" && !v.fixable));
	assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-001"));
}

#[test]
fn import_group_treats_workspace_members_as_self_group_for_spacing() {
	let workspace_root = env!("CARGO_PKG_NAME").replace('-', "_");
	let original = format!("\nuse anyhow::Result;\nuse {workspace_root}::internal::Alpha;\n");
	let ctx = shared::read_file_context_from_text(
		Path::new("import_workspace_member.rs"),
		original.clone(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-001"));
	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-002" && v.fixable));

	let mut rewritten = original.clone();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert_eq!(
		rewritten,
		format!("\nuse anyhow::Result;\n\nuse {workspace_root}::internal::Alpha;\n")
	);
}

#[test]
fn import_group_treats_file_local_mod_roots_as_self_group() {
	let original = r#"
use clap::Parser;
use tonic::transport::Server;
use tonic_health::server;
use tracing_subscriber::EnvFilter;
use auth::AuthInterceptor;
use cli::Cli;
use config::GatewayConfig;
use context::Context;
use db::Database;
use grpc::gateway_service_server::GatewayServiceServer;
use prelude::Core;
use service::GatewayService;
use types::App;

mod auth;
mod cli;
mod config;
mod context;
mod db;
mod error;
mod mail;
mod rate_limit;
mod service;
mod types;
mod grpc {
	tonic::include_proto!("pubfi.gateway");
}
mod prelude {
	pub use color_eyre::{Result, eyre};
	pub use time::OffsetDateTime;

	pub(crate) use crate::{error::SqlxStatusExt, grpc};
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import_local_mod_roots.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-001"));
	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-002" && v.fixable));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);
	assert!(rewritten.contains("use tracing_subscriber::EnvFilter;\n\nuse auth::AuthInterceptor;"));
	assert!(
		rewritten.contains("use prelude::Core;\nuse service::GatewayService;\nuse types::App;")
	);
}

#[test]
fn import_group_fix_reorders_origin_groups() {
	let original = r#"
use anyhow::Result;
use std::collections::HashSet;
use crate::z::Z;
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import_group_reorder.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-001" && v.fixable));
	assert!(edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-002"));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied >= 1);

	let std_idx = rewritten.find("use std::collections::HashSet;").expect("std");
	let third_party_idx = rewritten.find("use anyhow::Result;").expect("third-party");
	let self_idx = rewritten.find("use crate::z::Z;").expect("self");

	assert!(std_idx < third_party_idx);
	assert!(third_party_idx < self_idx);
}

#[test]
fn import_group_reorder_still_applies_in_fallback_mode_with_import009_present() {
	let original =
		"use crate::z::Z;\n\nuse std::fmt::Result;\n\nfn parse() -> Result<()> {\n\tOk(())\n}\n";
	let ctx = shared::read_file_context_from_text(
		Path::new("import_group_fallback_import009.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (fallback_violations, fallback_edits) =
		rules::collect_violations_with_import_shortening(&ctx, true, false);

	assert!(
		!fallback_edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-009"),
		"fallback mode should not collect import-shortening edits"
	);
	assert!(
		!fallback_violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009"),
		"fallback mode should not collect import-shortening violations"
	);
	assert!(
		fallback_edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-002"),
		"fallback mode should still include import-group edits, got edits={:?}, violations={:?}",
		fallback_edits.iter().map(|edit| edit.rule).collect::<Vec<_>>(),
		fallback_violations.iter().map(|violation| violation.rule).collect::<Vec<_>>()
	);

	let (rewritten, applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(Path::new("import_group_fallback_import009.rs"), original, false)
			.expect("apply fix passes");

	assert!(applied_count > 0);
	assert!(rewritten.contains("use std::fmt::Result;\n\nuse crate::z::Z;\n"));
	assert!(rewritten.contains("fn parse() -> Result<()> {\n\tOk(())\n}\n"));
	assert!(rewritten.contains("use std::fmt::Result;"));
}

#[test]
fn import002_fix_normalizes_mixed_self_child_use_tree_with_aliases() {
	let original = r#"
	use crate::{grpc, grpc::{ReferralCode as ProtoReferralCode, gateway_service_server::GatewayService as GrpcGatewayService}};
	"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import002_mixed_self_child_with_aliases.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (_violations, edits) = rules::collect_violations(&ctx, true);
	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(rewritten.contains(
			"use crate::{grpc::{ReferralCode as ProtoReferralCode, gateway_service_server::GatewayService as GrpcGatewayService}};"
		));
	assert!(!rewritten.contains("use crate::{grpc, grpc::"));
}

#[test]
fn import002_fix_drops_unused_self_from_nested_use_group() {
	let original = r#"
use crate::{grpc::{self, ReferralCode as ProtoReferralCode, VerifyMailCodeRequest}};
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import002_drop_unused_self_in_nested_group.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (_violations, edits) = rules::collect_violations(&ctx, true);
	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(rewritten.contains(
		"use crate::{grpc::{ReferralCode as ProtoReferralCode, VerifyMailCodeRequest}};"
	));
	assert!(!rewritten.contains("grpc::{self,"));
}

#[test]
fn import_group_cleanup_preserves_module_bindings_used_by_children() {
	let root = env::temp_dir().join(format!("vstyle-child-module-binding-{}", process::id()));
	let src = root.join("src");
	let parent = src.join("parent.rs");

	fs::create_dir_all(&src).expect("Create source directory.");
	fs::write(
		root.join("Cargo.toml"),
		"[package]\nname = \"child-binding\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
	)
	.expect("Write manifest.");
	fs::write(src.join("lib.rs"), "pub mod theme { pub struct Marker; }\npub mod parent;\n")
		.expect("Write root.");

	for imports in ["use crate::{theme::{self, Marker}};", "use crate::{theme, theme::Marker};"] {
		for child in [
			"use super::theme; pub fn read() -> usize { std::mem::size_of::<theme::Marker>() }",
			"use super::*; pub fn read() -> usize { std::mem::size_of::<theme::Marker>() }",
			"use crate::parent::theme; pub fn read() -> usize { std::mem::size_of::<theme::Marker>() }",
			"use super::theme::Marker; pub fn read() -> usize { std::mem::size_of::<Marker>() }",
			"use crate::parent::{theme::Marker}; pub fn read() -> usize { std::mem::size_of::<Marker>() }",
		] {
			let original = format!(
				"{imports}\n#[path = \"consumer.rs\"] mod child;\npub fn marker() -> Marker {{ Marker }}\npub fn read() -> usize {{ child::read() }}\n"
			);

			fs::write(&parent, &original).expect("Write parent.");
			fs::write(src.join("consumer.rs"), child).expect("Write child.");

			let (rewritten, _, _, _) =
				engine::apply_fix_passes(&parent, &original, true).expect("Apply fixes.");

			fs::write(&parent, &rewritten).expect("Write rewritten parent.");

			let compiled = Command::new("rustc")
				.args(["--edition=2024", "--crate-type=lib", "--emit=metadata"])
				.arg(src.join("lib.rs"))
				.arg("-o")
				.arg(root.join("fixture.rmeta"))
				.output()
				.expect("Compile fixture.");

			assert!(
				compiled.status.success(),
				"{rewritten}\n{}",
				String::from_utf8_lossy(&compiled.stderr)
			);

			let (repeated, _, _, _) =
				engine::apply_fix_passes(&parent, &rewritten, true).expect("Repeat fixes.");

			assert_eq!(rewritten, repeated);
		}
	}

	fs::remove_dir_all(root).expect("Remove fixture.");
}

#[test]
fn import_normalization_and_group_order_converge() {
	let original = "use crate::{theme::{self, Color}};\n\nuse std::path::PathBuf;\n\npub fn take(_: Color, _: PathBuf) {}\n";
	let path = Path::new("src/consumer.rs");
	let (rewritten, count, _, _) =
		engine::apply_fix_passes(path, original, true).expect("Apply fixes.");
	let ctx = shared::read_file_context_from_text(path, rewritten.clone())
		.expect("Parse.")
		.expect("Context.");
	let (violations, _) = rules::collect_violations(&ctx, false);

	assert!(count > 0, "{rewritten}");
	assert!(!rewritten.contains("self, Color"), "{rewritten}");
	assert!(
		rewritten.find("use std::").expect("Standard import")
			< rewritten.find("use crate::").expect("Local import")
	);
	assert!(
		!violations
			.iter()
			.any(|v| matches!(v.rule, "RUST-STYLE-IMPORT-001" | "RUST-STYLE-IMPORT-002")),
		"{violations:?}"
	);

	let (repeated, _, _, _) =
		engine::apply_fix_passes(path, &rewritten, true).expect("Repeat fixes.");

	assert_eq!(rewritten, repeated);
}
