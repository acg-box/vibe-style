mod file;
mod fixes;
mod impls;
mod imports;
mod module;
mod quality;
mod semantic;
mod shared;
mod spacing;

pub(crate) use shared::{CargoOptions, RunSummary};

use std::{fs, path::PathBuf};

use rayon::prelude::*;

use crate::prelude::*;
use shared::{Edit, FileContext, Violation};

const FILE_BATCH_SIZE: usize = 64;
const MAX_FIX_PASSES: usize = 8;

#[derive(Debug)]
struct FileFixOutcome {
	path: PathBuf,
	rewritten_text: Option<String>,
	applied_count: usize,
}

pub(crate) fn run_check(
	requested_files: &[PathBuf],
	cargo_options: &CargoOptions,
) -> Result<RunSummary> {
	let files = shared::resolve_files(requested_files, cargo_options)?;
	let mut violations: Vec<Violation> = Vec::new();

	for batch in files.chunks(FILE_BATCH_SIZE) {
		let batch_results = batch
			.par_iter()
			.map(|file| -> Result<Vec<Violation>> {
				let Some(ctx) = shared::read_file_context(file)? else {
					return Ok(Vec::new());
				};
				let (found, _edits) = collect_violations(&ctx, false);

				Ok(found)
			})
			.collect::<Vec<_>>();

		for result in batch_results {
			violations.extend(result?);
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

pub(crate) fn run_fix(
	requested_files: &[PathBuf],
	cargo_options: &CargoOptions,
) -> Result<RunSummary> {
	let files = shared::resolve_files(requested_files, cargo_options)?;
	let mut total_applied = 0_usize;

	for batch in files.chunks(FILE_BATCH_SIZE) {
		let outcomes = batch
			.par_iter()
			.map(|file| -> Result<FileFixOutcome> {
				let mut text = match fs::read_to_string(file) {
					Ok(text) => text,
					Err(_) => {
						return Ok(FileFixOutcome {
							path: file.clone(),
							rewritten_text: None,
							applied_count: 0,
						});
					},
				};
				let mut pass = 0_usize;
				let mut applied_count = 0_usize;

				while pass < MAX_FIX_PASSES {
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

					applied_count += applied;
				}

				Ok(FileFixOutcome {
					path: file.clone(),
					rewritten_text: if applied_count > 0 { Some(text) } else { None },
					applied_count,
				})
			})
			.collect::<Vec<_>>();

		for outcome in outcomes {
			let outcome = outcome?;

			total_applied += outcome.applied_count;

			if let Some(text) = outcome.rewritten_text {
				fs::write(&outcome.path, text)?;
			}
		}
	}

	total_applied += semantic::apply_semantic_fixes(&files, cargo_options)?;

	let checked = run_check(requested_files, cargo_options)?;

	Ok(RunSummary {
		file_count: checked.file_count,
		violation_count: checked.violation_count,
		unfixable_count: checked.unfixable_count,
		applied_fix_count: total_applied,
		output_lines: checked.output_lines,
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
	file::check_serde_option_default(ctx, &mut violations, &mut edits, with_fixes);
	file::check_error_rs_no_use(ctx, &mut violations, &mut edits, with_fixes);
	imports::check_import_rules(ctx, &mut violations, &mut edits, with_fixes);
	module::check_module_order(ctx, &mut violations, &mut edits, with_fixes);
	module::check_cfg_test_mod_tests_use_super(ctx, &mut violations, &mut edits, with_fixes);
	impls::check_impl_adjacency(ctx, &mut violations, &mut edits, with_fixes);
	impls::check_impl_rules(ctx, &mut violations, &mut edits, with_fixes);
	impls::check_inline_trait_bounds(ctx, &mut violations);
	quality::check_std_macro_calls(ctx, &mut violations, &mut edits, with_fixes);
	quality::check_logging_quality(ctx, &mut violations);
	quality::check_expect_unwrap(ctx, &mut violations, &mut edits, with_fixes);
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
	fn runtime_fix_rewrites_unwrap_to_expect() {
		let original = "fn demo_case(value: Option<usize>) -> usize {\n\tvalue.unwrap()\n}\n";
		let ctx = shared::read_file_context_from_text(
			Path::new("runtime_unwrap_fix.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);

		assert!(
			violations
				.iter()
				.any(|violation| violation.rule == "RUST-STYLE-RUNTIME-001" && violation.fixable)
		);

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains(r#"value.expect("Expected operation to succeed.")"#));
	}

	#[test]
	fn runtime_fix_normalizes_expect_message_sentence() {
		let original =
			"fn demo_case(value: Option<usize>) -> usize {\n\tvalue.expect(\"missing value\")\n}\n";
		let ctx = shared::read_file_context_from_text(
			Path::new("runtime_expect_fix.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);

		assert!(
			violations
				.iter()
				.any(|violation| violation.rule == "RUST-STYLE-RUNTIME-002" && violation.fixable)
		);

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains(r#"value.expect("Missing value.")"#));
	}

	#[test]
	fn runtime_rules_ignore_cfg_test_module_calls() {
		let text = "#[cfg(test)]\nmod tests {\n\t#[test]\n\tfn sample_case() {\n\t\tlet value = Some(1usize);\n\t\tlet _ = value.unwrap();\n\t}\n}\n";
		let ctx =
			shared::read_file_context_from_text(Path::new("runtime_cfg_test.rs"), text.to_owned())
				.expect("context")
				.expect("has ctx");
		let (violations, _edits) = collect_violations(&ctx, true);

		assert!(violations.iter().all(|violation| !matches!(
			violation.rule,
			"RUST-STYLE-RUNTIME-001" | "RUST-STYLE-RUNTIME-002"
		)));
	}

	#[test]
	fn serde001_fix_removes_standalone_default_attr_on_option_field() {
		let original = r#"
#[derive(Deserialize)]
struct Payload {
	#[serde(default)]
	value: Option<String>,
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("serde001_standalone.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-SERDE-001" && v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-SERDE-001"));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(!rewritten.contains("#[serde(default)]"));
		assert!(rewritten.contains("value: Option<String>,"));
	}

	#[test]
	fn serde001_fix_removes_default_from_combined_serde_attr() {
		let original = r#"
#[derive(Deserialize)]
struct Payload {
	#[serde(default, rename = "value")]
	value: Option<String>,
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("serde001_combined.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-SERDE-001" && v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-SERDE-001"));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(!rewritten.contains("default"));
		assert!(rewritten.contains(r#"#[serde(rename = "value")]"#));
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
		let (violations, edits) = collect_violations(&ctx, true);

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
		let (violations, edits) = collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-005" && !v.fixable));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-005"));
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
		assert!(
			rewritten.contains("use std::collections::HashSet;\n\n\tuse super::*;\n\n\t#[test]")
		);
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
		assert!(rewritten.contains("use std::collections::HashSet;"));
		assert!(rewritten.contains("use super::*;"));

		let std_idx = rewritten.find("use std::collections::HashSet;").expect("std import");
		let super_idx = rewritten.find("use super::*;").expect("super glob import");

		assert!(std_idx < super_idx);
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
	fn import006_ignores_std_macro_tokens_inside_string_literals() {
		let text = r##"
fn example() {
	let prompt = r#"
	- keep std::format!(...) in examples.
	"#;
	println!("{prompt}");
}
"##;
		let ctx =
			shared::read_file_context_from_text(Path::new("import006_string.rs"), text.to_owned())
				.expect("context")
				.expect("has ctx");
		let (violations, _edits) = collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-006"));
	}

	#[test]
	fn import006_fix_rewrites_real_std_macro_call() {
		let original = r#"
fn example(value: usize) -> String {
	std::format!("{value}")
}
"#;
		let ctx =
			shared::read_file_context_from_text(Path::new("import006_fix.rs"), original.to_owned())
				.expect("context")
				.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-006" && v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains("format!(\"{value}\")"));
		assert!(!rewritten.contains("std::format!"));
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
		let workspace_root = env!("CARGO_PKG_NAME").replace('-', "_");
		let original = format!("\nuse anyhow::Result;\nuse {workspace_root}::internal::Alpha;\n");
		let ctx = shared::read_file_context_from_text(
			Path::new("import_workspace_member.rs"),
			original.clone(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);

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
use prelude::*;
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
		let (violations, edits) = collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-001"));
		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-002" && v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(
			rewritten.contains("use tracing_subscriber::EnvFilter;\n\nuse auth::AuthInterceptor;")
		);
		assert!(
			rewritten.contains("use prelude::*;\nuse service::GatewayService;\nuse types::App;")
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
		let (violations, edits) = collect_violations(&ctx, true);

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
		let (violations, edits) = collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-002"
				&& v.message
					== "Normalize imports like `use a::{b, b::c}` to `use a::{b::{self, c}}`."
		}));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-002"));
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
		let ctx = shared::read_file_context_from_text(
			Path::new("import_brace_artifact.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, _edits) = collect_violations(&ctx, true);

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
		let (violations, _edits) = collect_violations(&ctx, true);

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
		let (violations, edits) = collect_violations(&ctx, true);

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
		assert!(rewritten.contains("crate::math::sum(1, 2)"));
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
		let (violations, edits) = collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-004" && v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-004"));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 2);
		assert!(!rewritten.contains("use crate::metrics::emit;"));
		assert!(rewritten.contains("crate::metrics::emit!(\"ok\")"));
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
		let (violations, edits) = collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-004" && v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-004"));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 2);
		assert!(rewritten.contains("shared::line_from_offset()"));
		assert!(rewritten.contains("use super::shared::{self, Edit, offset_from_line};"));
		assert!(
			!rewritten.contains("use super::shared::{Edit, line_from_offset, offset_from_line};")
		);
	}

	#[test]
	fn import008_fix_imports_unambiguous_type_paths_and_keeps_group_order() {
		let original = r#"
use std::collections::HashSet;

use crate::prelude::*;

fn run<'e, E>(_exec: E)
where
	E: sqlx::Executor<'e>,
{
	let _ = HashSet::<usize>::new();
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
			let (_violations, edits) = collect_violations(&ctx, true);

			if edits.is_empty() {
				break;
			}

			let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		}

		assert!(rewritten.contains("use sqlx::Executor;"));
		assert!(rewritten.contains("E: Executor<'e>,"));

		let std_idx = rewritten.find("use std::collections::HashSet;").expect("std");
		let third_party_idx = rewritten.find("use sqlx::Executor;").expect("third-party");
		let self_idx = rewritten.find("use crate::prelude::*;").expect("self");

		assert!(std_idx < third_party_idx);
		assert!(third_party_idx < self_idx);
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
		let ctx = shared::read_file_context_from_text(
			Path::new("import008_ambiguous.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-008"));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-008"));
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
		let ctx = shared::read_file_context_from_text(
			Path::new("import008_cfg_test.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);

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
			let (_violations, edits) = collect_violations(&ctx, true);

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
	fn import008_does_not_shorten_when_glob_import_exists() {
		let original = r#"
use sqlx::Result;

use super::*;
use pubfi_db::service::feed::dry_runs::FeedDryRunRow;

pub async fn get_by_id(db: &PgPool, dry_run_id: i64) -> sqlx::Result<Option<FeedDryRunRow>> {
	let _ = (db, dry_run_id);
	todo!()
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import008_glob_ambiguity_guard.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-008"));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-008"));

		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(rewritten.contains("-> sqlx::Result<"));
		assert!(!rewritten.contains("-> Result<"));
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
		let (violations, edits) = collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-008"));
		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009"));
		assert!(!edits.iter().any(|e| {
			matches!(
				e.rule,
				"RUST-STYLE-IMPORT-001"
					| "RUST-STYLE-IMPORT-002"
					| "RUST-STYLE-IMPORT-003"
					| "RUST-STYLE-IMPORT-004"
					| "RUST-STYLE-IMPORT-006"
					| "RUST-STYLE-IMPORT-007"
					| "RUST-STYLE-IMPORT-008"
					| "RUST-STYLE-IMPORT-009"
			)
		}));
	}

	#[test]
	fn import009_fix_rewrites_mixed_import_and_qualified_symbol_usage() {
		let original = r#"
use a::A;

fn sample(a: A, aa: b::A) {
	let _ = (a, aa);
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import009_mixed_usage.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009" && v.fixable));

		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(!rewritten.contains("use a::A;"));
		assert!(rewritten.contains("fn sample(a: a::A, aa: b::A)"));
	}

	#[test]
	fn import009_fix_rewrites_when_glob_import_conflicts() {
		let original = r#"
use sqlx::Result;

use super::*;
use pubfi_db::service::feed::dry_runs::FeedDryRunRow;

pub async fn get_by_id(db: &PgPool, dry_run_id: i64) -> Result<Option<FeedDryRunRow>> {
	let _ = (db, dry_run_id);
	todo!()
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import009_glob_conflict.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009" && v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-009"));

		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(!rewritten.contains("\nuse sqlx::Result;"));
		assert!(rewritten.contains("-> sqlx::Result<"));
		assert!(!rewritten.contains("-> Result<"));
	}

	#[test]
	fn mod005_fix_moves_impl_block_adjacent_to_type() {
		let original = r#"
struct Sample;

enum Other {
	Item,
}

impl Sample {
	fn new() -> Self {
		Self
	}
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod005_move_impl.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-005" && v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-MOD-005"));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 2);

		let struct_idx = rewritten.find("struct Sample;").expect("struct");
		let impl_idx = rewritten.find("impl Sample").expect("impl");
		let enum_idx = rewritten.find("enum Other").expect("enum");

		assert!(struct_idx < impl_idx);
		assert!(impl_idx < enum_idx);
	}

	#[test]
	fn mod005_fix_moves_impl_block_after_type_when_impl_precedes_type() {
		let original = r#"
impl Sample {
	fn new() -> Self {
		Self
	}
}

struct Sample;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod005_impl_before_type.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-005" && v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-MOD-005"));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 2);
		assert!(rewritten.contains("struct Sample;\nimpl Sample {"));
	}

	#[test]
	fn mod005_fix_does_not_accumulate_blank_lines_around_relocated_impls() {
		let original = r#"
use std::sync::Arc;

struct RoundGate {
	in_progress: bool,
}

struct RoundGateGuard {
	gate: Arc<RoundGate>,
}

impl RoundGate {
	fn new() -> Self {
		Self { in_progress: false }
	}
}

impl RoundGateGuard {
	fn new(gate: Arc<RoundGate>) -> Self {
		Self { gate }
	}
}

impl Drop for RoundGateGuard {
	fn drop(&mut self) {}
}

/// Bootstrap and run all crawlers.
pub async fn run() {}
"#;
		let mut rewritten = original.to_owned();

		for _ in 0..MAX_FIX_PASSES {
			let Some(ctx) = shared::read_file_context_from_text(
				Path::new("mod005_blank_lines.rs"),
				rewritten.clone(),
			)
			.expect("context") else {
				break;
			};
			let (_violations, edits) = collect_violations(&ctx, true);

			if edits.is_empty() {
				break;
			}

			let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

			if applied == 0 {
				break;
			}
		}

		assert!(
			rewritten.contains("struct RoundGate {\n\tin_progress: bool,\n}\nimpl RoundGate {")
		);
		assert!(rewritten.contains(
			"struct RoundGateGuard {\n\tgate: Arc<RoundGate>,\n}\nimpl RoundGateGuard {"
		));
		assert!(rewritten.contains("}\n\n/// Bootstrap and run all crawlers."));
		assert!(!rewritten.contains("}\n\n\n/// Bootstrap and run all crawlers."));
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

	#[test]
	fn mod001_fix_reorders_cross_category_top_level_items() {
		let original = r#"
fn execute() -> usize {
	1
}

const LIMIT: usize = 3;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod001_cross_kind.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-001" && v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(
			rewritten.find("const LIMIT").unwrap_or_default()
				< rewritten.find("fn execute").unwrap_or_default()
		);
	}

	#[test]
	fn mod002_fix_reorders_pub_items_across_interleaved_segments() {
		let original = r#"
fn internal_a() -> usize {
	1
}

const LIMIT: usize = 3;

pub fn external() -> usize {
	2
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod002_interleaved.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-002" && v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(
			rewritten.find("pub fn external").unwrap_or_default()
				< rewritten.find("fn internal_a").unwrap_or_default()
		);
	}

	#[test]
	fn mod001_fix_keeps_adjacent_top_level_const_group_compact() {
		let original = r#"
const CONTENT_CHAR_LIMIT: usize = 500;

fn helper() -> usize {
	1
}

const INSIGHTS_PER_FEED_LIMIT: usize = 3;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod001_const_compact.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-001" && v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains(
			"const CONTENT_CHAR_LIMIT: usize = 500;\nconst INSIGHTS_PER_FEED_LIMIT: usize = 3;\n\nfn helper() -> usize {"
		));
	}

	#[test]
	fn mod001_fix_keeps_blank_line_between_const_and_static_groups() {
		let original = r#"
static RE_HEADING: usize = 1;
const SANITIZE_EXPANSION_LIMIT: f32 = 1.3;
const SANITIZE_SHRINK_LIMIT: f32 = 0.6;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod001_const_static_grouping.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-001" && v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains(
			"const SANITIZE_EXPANSION_LIMIT: f32 = 1.3;\nconst SANITIZE_SHRINK_LIMIT: f32 = 0.6;\n\nstatic RE_HEADING: usize = 1;"
		));
	}

	#[test]
	fn mod002_fix_separates_pub_and_non_pub_const_groups() {
		let original = r#"
const INTERNAL_LIMIT: usize = 3;

pub const PUBLIC_LIMIT: usize = 5;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod002_const_compact.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-002" && v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(
			rewritten
				.contains("pub const PUBLIC_LIMIT: usize = 5;\n\nconst INTERNAL_LIMIT: usize = 3;")
		);
	}

	#[test]
	fn mod002_fix_inserts_blank_line_between_visibility_batches_without_reorder() {
		let original = r#"
pub const PUBLIC_LIMIT: usize = 5;
const INTERNAL_LIMIT: usize = 3;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod002_visibility_batch_spacing.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-MOD-002"
				&& v.message
					== "Insert exactly one blank line between pub and non-pub batches within the same item kind."
				&& v.fixable
		}));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(
			rewritten
				.contains("pub const PUBLIC_LIMIT: usize = 5;\n\nconst INTERNAL_LIMIT: usize = 3;")
		);
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
		let (violations, edits) = collect_violations(&ctx, true);

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
		let (violations, edits) = collect_violations(&ctx, true);

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
		let (violations, _edits) = collect_violations(&ctx, true);

		assert!(
			!violations
				.iter()
				.any(|v| v.rule == "RUST-STYLE-SPACE-003" || v.rule == "RUST-STYLE-SPACE-004")
		);
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
		let (violations, edits) = collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-004" && v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains("let value = 1;\n\n\t\treturn value;"));
	}
}
