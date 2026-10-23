use std::{
	env, fs,
	path::Path,
	process::{self, Command},
	time::{SystemTime, UNIX_EPOCH},
};

use crate::style::{engine, fixes, rules, shared};

#[test]
fn import004_preserves_bindings_used_by_external_children() {
	let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Timestamp.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-child-import-{}-{now}", process::id()));
	let src = root.join("src");
	let parent = src.join("lib.rs");
	let original = "use std::fs::read;\n#[path = \"consumer.rs\"] mod child;\npub fn run() { let _ = read(\"fixture\"); }\n";

	fs::create_dir_all(&src).expect("Create source directory.");
	fs::write(
		root.join("Cargo.toml"),
		"[package]\nname = \"child-import-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
	)
	.expect("Write manifest.");
	fs::write(&parent, original).expect("Write parent.");

	for child in [
		"use super::*; pub fn run() { let _ = read(\"fixture\"); }",
		"use super::{self as parent, *}; pub fn run() { let _ = read(\"fixture\"); }",
		"use super::read as load; pub fn run() { let _ = load(\"fixture\"); }",
		"pub fn run() { let _ = super::read(\"fixture\"); }",
		"use crate::read as load; pub fn run() { let _ = load(\"fixture\"); }",
		"mod nested { pub fn run() { let _ = super::super::read(\"fixture\"); } }",
	] {
		fs::write(src.join("consumer.rs"), child).expect("Write child.");

		let ctx =
			shared::read_file_context(&parent).expect("Read context.").expect("Have context.");
		let (violations, edits) = rules::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-004" && v.fixable));

		let edits = edits.into_iter().filter(|edit| edit.rule == "RUST-STYLE-IMPORT-004").collect();
		let mut rewritten = original.to_owned();

		fixes::apply_edits(&mut rewritten, edits).expect("Qualify parent calls.");

		assert!(rewritten.contains("use std::fs::read;"), "{child}: {rewritten}");
		assert!(rewritten.contains("std::fs::read(\"fixture\")"), "{rewritten}");

		fs::write(&parent, &rewritten).expect("Write qualified parent.");

		let compiled = Command::new("rustc")
			.args(["--edition=2024", "--crate-type=lib"])
			.arg(&parent)
			.arg("-o")
			.arg(root.join("fixture.rlib"))
			.output()
			.expect("Compile parent and child.");

		assert!(
			compiled.status.success(),
			"{child}: {}",
			String::from_utf8_lossy(&compiled.stderr)
		);

		fs::write(&parent, original).expect("Restore next fixture input.");
	}

	let inline = "use std::fs::read;\npub fn run() { let _ = read(\"fixture\"); }\nmod child { use std::ptr::read; pub fn run() -> u8 { unsafe { read(&7_u8) } } pub fn parent() { let _ = super::read(\"fixture\"); } }\n";

	fs::write(&parent, inline).expect("Write inline shadow fixture.");

	let ctx =
		shared::read_file_context(&parent).expect("Read inline fixture.").expect("Have context.");
	let (_, edits) = rules::collect_violations(&ctx, true);
	let edits = edits.into_iter().filter(|edit| edit.rule == "RUST-STYLE-IMPORT-004").collect();
	let mut rewritten = inline.to_owned();

	fixes::apply_edits(&mut rewritten, edits).expect("Qualify only parent calls.");

	assert!(rewritten.contains("std::fs::read(\"fixture\")"), "{rewritten}");
	assert!(rewritten.contains("unsafe { read(&7_u8) }"), "{rewritten}");
	assert!(rewritten.contains("super::read(\"fixture\")"), "{rewritten}");

	fs::write(&parent, rewritten).expect("Write transformed inline fixture.");

	let compiled = Command::new("rustc")
		.args(["--edition=2024", "--crate-type=lib"])
		.arg(&parent)
		.arg("-o")
		.arg(root.join("inline.rlib"))
		.output()
		.expect("Compile inline shadow fixture.");

	assert!(compiled.status.success(), "{}", String::from_utf8_lossy(&compiled.stderr));

	let ctx = shared::read_file_context(&parent)
		.expect("Read qualified fixture.")
		.expect("Have context.");
	let (violations, _) = rules::collect_violations(&ctx, false);

	assert!(!violations.iter().any(|v| v.line == 1 && v.rule == "RUST-STYLE-IMPORT-004"));

	fs::write(&parent, original).expect("Restore external fixture.");
	fs::write(src.join("consumer.rs"), "pub fn run() { let _ = std::fs::read(\"fixture\"); }")
		.expect("Write independent child.");

	let ctx = shared::read_file_context(&parent).expect("Read context.").expect("Have context.");
	let (_, edits) = rules::collect_violations(&ctx, true);

	assert!(
		edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-004"),
		"Independent child must not block qualification."
	);

	fs::remove_dir_all(root).expect("Remove fixture.");
}

#[test]
fn import004_planned_modules_preserve_configuration_boundaries() {
	let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Timestamp.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-planned-cfg-{}-{now}", process::id()));
	let parent = root.join("src/lib.rs");

	fs::create_dir_all(root.join("src")).expect("Create fixture.");
	fs::write(
		root.join("Cargo.toml"),
		"[package]\nname = \"planned-cfg-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
	)
	.expect("Write manifest.");

	for imports in [
		"#[cfg(test)] use std::fs::read_to_string;\nuse std::fs::read;",
		"use std::fs::read;\n#[cfg(test)] use std::fs::read_to_string;",
		"#[cfg_attr(not(test), cfg(any()))] use std::fs::read_to_string;\nuse std::fs::read;",
	] {
		let original = format!(
			"{imports}\npub fn run() {{ let _ = read(\"fixture\"); }}\n#[cfg(test)] pub fn test_only() {{ let _ = read_to_string(\"fixture\"); }}\n"
		);

		fs::write(&parent, &original).expect("Write source.");

		let (rewritten, _, _, _) =
			engine::apply_fix_passes(&parent, &original, true).expect("Apply fixes.");

		fs::write(&parent, &rewritten).expect("Write rewritten source.");

		let (again, _, _, _) =
			engine::apply_fix_passes(&parent, &rewritten, true).expect("Repeat fixes.");

		assert_eq!(again, rewritten, "Conditional fixes must be stable.");

		for testing in [false, true] {
			let mut command = Command::new("rustc");

			command.args(["--edition=2024", "--crate-type=lib"]);

			if testing {
				command.arg("--test");
			}

			let compiled = command
				.arg(&parent)
				.arg("-o")
				.arg(root.join("compiled"))
				.output()
				.expect("Compile fixture.");

			assert!(
				compiled.status.success(),
				"testing={testing}\n{rewritten}\n{}",
				String::from_utf8_lossy(&compiled.stderr)
			);
		}
	}

	fs::remove_dir_all(root).expect("Remove fixture.");
}

#[test]
fn import004_does_not_use_test_only_module_for_production_calls() {
	let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Timestamp.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-cfg-module-{}-{now}", process::id()));
	let src = root.join("src");
	let parent = src.join("lib.rs");
	let original = "#[cfg(test)] use std::fs;\nuse std::fs::read;\npub fn run() { let _ = read(\"fixture\"); }\n#[cfg(test)] pub fn test_only() { let _ = fs::read(\"fixture\"); }\n";

	fs::create_dir_all(&src).expect("Create source directory.");
	fs::write(
		root.join("Cargo.toml"),
		"[package]\nname = \"cfg-module-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
	)
	.expect("Write manifest.");
	fs::write(&parent, original).expect("Write source.");

	let (rewritten, _, _, _) =
		engine::apply_fix_passes(&parent, original, true).expect("Apply fixes.");

	assert!(rewritten.contains("pub fn run() { let _ = std::fs::read("), "{rewritten}");

	fs::write(&parent, &rewritten).expect("Write transformed fixture.");

	for testing in [false, true] {
		let mut command = Command::new("rustc");

		command.args(["--edition=2024", "--crate-type=lib"]);

		if testing {
			command.arg("--test");
		}

		command.arg(&parent).arg("-o").arg(root.join("compiled"));

		let compiled = command.output().expect("Compile conditional fixture.");

		assert!(compiled.status.success(), "{}", String::from_utf8_lossy(&compiled.stderr));
	}

	fs::remove_dir_all(root).expect("Remove fixture.");
}

#[test]
fn import004_shortens_existing_full_path_free_function_call() {
	let original = r#"
fn sample() {
	crate::app::build_auth_state();
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import004_existing_full_path_function.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(rewritten.contains("use crate::app;"), "{rewritten}");
	assert!(rewritten.contains("app::build_auth_state();"), "{rewritten}");
	assert!(!rewritten.contains("crate::app::build_auth_state();"), "{rewritten}");
}

#[test]
fn import004_does_not_hoist_import_out_of_cfg_feature_expression() {
	let original = r#"
async fn entrypoint() {
	#[cfg(feature = "process-test-support")]
	if std::env::var("PROCESS_TEST").is_ok() {
		return crate::process_test_support::serve_disabled_artifact_runtime().await;
	}
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(Path::new("import004_cfg_feature_expression.rs"), original, true)
			.expect("apply fix passes");

	assert_eq!(rewritten, original);
}

#[test]
fn import004_keeps_existing_full_path_when_parent_module_name_is_ambiguous() {
	let original = r#"
use other::app;

fn sample() {
	crate::app::build_auth_state();
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import004_existing_full_path_function_ambiguous.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(!rewritten.contains("use crate::app;"), "{rewritten}");
	assert!(rewritten.contains("crate::app::build_auth_state();"), "{rewritten}");
}

#[test]
fn import004_shortens_existing_full_path_call_inside_cfg_test_module() {
	let original = r#"
#[cfg(test)]
mod tests {
	use crate::app::AuthState;

	#[test]
	fn sample() {
		let _ = AuthState;
		crate::app::build_auth_state();
	}
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import004_existing_full_path_function_cfg_test.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(rewritten.contains("\tuse crate::app::{self, AuthState};"), "{rewritten}");
	assert!(rewritten.contains("\t\tapp::build_auth_state();"), "{rewritten}");
	assert!(!rewritten.contains("crate::app::build_auth_state();"), "{rewritten}");
}

#[test]
fn import004_shortens_existing_full_path_call_inside_macro_tokens() {
	let original = r#"
fn sample() {
	assert!(crate::app::build_auth_state().is_ok());
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import004_existing_full_path_function_macro_tokens.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(rewritten.contains("use crate::app;"), "{rewritten}");
	assert!(rewritten.contains("assert!(app::build_auth_state().is_ok());"), "{rewritten}");
	assert!(!rewritten.contains("crate::app::build_auth_state()"), "{rewritten}");
}

#[test]
fn import004_keeps_existing_full_path_when_root_qualified_time_path_would_conflict() {
	let original = r#"
fn sample() {
	let _ = time::OffsetDateTime::UNIX_EPOCH;
	tokio::time::sleep();
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import004_existing_full_path_function_root_time_conflict.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(!rewritten.contains("use tokio::time;"), "{rewritten}");
	assert!(rewritten.contains("tokio::time::sleep();"), "{rewritten}");
}

#[test]
fn import004_keeps_existing_full_path_when_imported_root_time_use_would_conflict() {
	let original = r#"
use time::OffsetDateTime;

fn sample() {
	let _ = OffsetDateTime::UNIX_EPOCH;
	tokio::time::sleep();
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import004_existing_full_path_function_imported_time_conflict.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(rewritten.contains("use time::OffsetDateTime;"), "{rewritten}");
	assert!(!rewritten.contains("use tokio::time;"), "{rewritten}");
	assert!(rewritten.contains("tokio::time::sleep();"), "{rewritten}");
}

#[test]
fn import004_keeps_existing_full_path_when_root_qualified_product_path_would_conflict() {
	let original = r#"
fn sample() {
	let _ = product::Catalog::default();
	crate::product::build();
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import004_existing_full_path_function_root_product_conflict.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(!rewritten.contains("use crate::product;"), "{rewritten}");
	assert!(rewritten.contains("product::build();"), "{rewritten}");
}

#[test]
fn import004_keeps_existing_full_path_when_imported_root_product_use_would_conflict() {
	let original = r#"
use product::Catalog;

fn sample() {
	let _ = Catalog::default();
	crate::product::build();
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import004_existing_full_path_function_imported_product_conflict.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(rewritten.contains("use product::Catalog;"), "{rewritten}");
	assert!(!rewritten.contains("use crate::product;"), "{rewritten}");
	assert!(rewritten.contains("product::build();"), "{rewritten}");
}

#[test]
fn import004_does_not_rewrite_method_calls_through_parent_module_imports() {
	let original = r#"
use std::iter;

fn sample() {
	let ordered = [1usize, 2usize];
	let _ = ordered.iter().copied().collect::<Vec<_>>();
	let _ = iter::once(3usize);
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import004_method_call_parent_module_import.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(rewritten.contains("ordered.iter().copied()"), "{rewritten}");
	assert!(!rewritten.contains("ordered.std::iter()"), "{rewritten}");
	assert!(rewritten.contains("iter::once("), "{rewritten}");
}

#[test]
fn import004_does_not_rewrite_method_calls_inside_macro_tokens() {
	let original = r#"
use std::iter;

fn sample() {
	let ordered = [1usize, 2usize];
	let _ = format!(
		"{:?}",
		ordered.iter().map(|entry| entry.saturating_add(1)).collect::<Vec<_>>()
	);
	let _ = iter::once(3usize);
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import004_method_call_inside_macro_tokens.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(rewritten.contains("ordered.iter().map("), "{rewritten}");
	assert!(!rewritten.contains("ordered.std::iter()"), "{rewritten}");
	assert!(rewritten.contains("iter::once("), "{rewritten}");
}

#[test]
fn import004_keeps_full_path_when_free_function_import_root_time_use_would_conflict() {
	let original = r#"
use time::OffsetDateTime;
use tokio::time::sleep;

fn sample() {
	let _ = OffsetDateTime::UNIX_EPOCH;
	sleep();
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import004_free_function_import_imported_time_conflict.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(rewritten.contains("use time::OffsetDateTime;"), "{rewritten}");
	assert!(!rewritten.contains("use tokio::time;"), "{rewritten}");
	assert!(!rewritten.contains("use tokio::time::sleep;"), "{rewritten}");
	assert!(rewritten.contains("tokio::time::sleep();"), "{rewritten}");
	assert!(!rewritten.contains("\n\ttime::sleep();"), "{rewritten}");
}

#[test]
fn macro_qualification_preserves_reexported_standard_modules() {
	let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Timestamp.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-std-reexport-{}-{now}", process::id()));
	let source = root.join("src");
	let consumer = source.join("consumer.rs");
	let original = r#"use crate::facade::panic;

#[test]
fn keeps_builtin_macro() {
    let result = panic::catch_unwind(|| panic!("fixture"));
    let message = result.unwrap_err();
    let text = message.downcast_ref::<&str>().copied()
        .or_else(|| message.downcast_ref::<String>().map(String::as_str));
    assert_eq!(text, Some("fixture"));
}
"#;

	fs::create_dir_all(&source).expect("Create fixture.");
	fs::write(
		root.join("Cargo.toml"),
		"[package]\nname = \"std-reexport-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
	)
	.expect("Write manifest.");
	fs::write(source.join("lib.rs"), r#"mod provider; mod facade; mod consumer;
#[macro_export] macro_rules! custom_panic { ($message:expr) => { std::panic!("custom {}", $message) }; }"#)
			.expect("Write root.");
	fs::write(source.join("provider.rs"), "pub use std::panic::{self}; pub struct Token;")
		.expect("Write provider.");

	for (facade, custom, self_only) in [
		("pub use std::panic::{self};", false, false),
		("pub use crate::provider::panic;", false, false),
		("pub use std::panic::{self}; pub use crate::custom_panic as panic;", true, false),
		("pub use std::panic::{self}; pub use crate::custom_panic as panic;", false, true),
	] {
		fs::write(source.join("facade.rs"), facade).expect("Write facade.");

		let original = if custom {
			original.replace("Some(\"fixture\")", "Some(\"custom fixture\")")
		} else {
			original.to_owned()
		};
		let original = if self_only {
			original
				.replace(
					"use crate::facade::panic;",
					"use crate::{provider::*, facade::panic::{self}};",
				)
				.replace("let result =", "let _ = Token;\n    let result =")
		} else {
			original
		};

		fs::write(&consumer, &original).expect("Write consumer.");

		let (rewritten, _, _, _) =
			engine::apply_fix_passes(&consumer, &original, true).expect("Fix consumer.");

		if self_only {
			assert!(!rewritten.contains("::*"), "{rewritten}");
		}
		if custom {
			assert!(rewritten.contains("facade::panic!"), "{rewritten}");
		}

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
			"{rewritten}\n{}\n{}",
			String::from_utf8_lossy(&executed.stdout),
			String::from_utf8_lossy(&executed.stderr)
		);
		assert!(String::from_utf8_lossy(&executed.stdout).contains("1 passed;"));
	}

	fs::remove_dir_all(root).expect("Remove fixture.");
}

#[test]
fn import009_preserves_test_module_macro_bindings() {
	let original = "use std::io::Error;\nfn outer(_: Error) {}\nfn other(_: std::fmt::Error) {}\n#[cfg(test)]\nmod tests {\n\tuse std::fmt::Error;\n\t#[test]\n\tfn inner() {\n\t\tassert_eq!(format!(\"{:?}\", Error::default()), \"Error\");\n\t}\n}\n";
	let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Timestamp.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-macro-scope-{}-{now}", process::id()));
	let path = root.join("lib.rs");
	let binary = root.join("fixture");

	fs::create_dir_all(&root).expect("Create fixture directory.");

	let (rewritten, _, _, _) =
		engine::apply_fix_passes(&path, original, true).expect("Apply fixes.");

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

	fs::remove_dir_all(root).expect("Remove fixture.");

	assert!(executed.status.success(), "{}", String::from_utf8_lossy(&executed.stderr));
}

#[test]
fn import004_preserves_explicit_call_type_and_const_arguments() {
	let original = "use std::mem::{size_of, align_of};\nuse std::array::from_fn;\nfn main() { let size = size_of::<[u16; 3]>(); let alignment = align_of::<u16>(); let array = from_fn::<u8, 3, _>(|i| i as u8); assert_eq!(size, 6); assert_eq!(alignment, 2); assert_eq!(array, [0, 1, 2]); assert_eq!(size_of::<u32>(), 4); }\n";
	let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Timestamp.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-call-generics-{}-{now}", process::id()));
	let path = root.join("main.rs");
	let binary = root.join("fixture");

	fs::create_dir_all(&root).expect("Create fixture directory.");

	let (rewritten, _, _, _) =
		engine::apply_fix_passes(&path, original, true).expect("Apply fixes.");

	fs::write(&path, &rewritten).expect("Write rewritten fixture.");

	let compiled = Command::new("rustc")
		.arg("--edition=2024")
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

	fs::remove_dir_all(root).expect("Remove fixture.");

	assert!(executed.status.success(), "{}", String::from_utf8_lossy(&executed.stderr));
}

#[test]
fn import009_preserves_imports_used_by_bare_patterns() {
	let original = "use libc::EPERM;\nuse libc::ESRCH;\nfn plain(code: Option<i32>) -> bool { match code { Some(ESRCH) => true, Some(EPERM) => false, _ => false } }\n#[cfg(unix)]\nfn guarded(code: Option<i32>) -> bool { match code { Some(libc::ESRCH) => true, Some(libc::EPERM) => false, _ => false } }\n";
	let ctx =
		shared::read_file_context_from_text(Path::new("constant_patterns.rs"), original.into())
			.expect("Read context.")
			.expect("Have context.");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009" && !v.fixable));
	assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-009"));
}

#[test]
fn import004_qualifies_function_calls_after_macro_labels() {
	let original = "use crate::geometry::point;\nfn sample() { let _ = point(1); wrapper!(origin: point(2)); }";
	let (rewritten, _, _, _) =
		engine::apply_fix_passes(Path::new("macro_labels.rs"), original, true)
			.expect("Apply fixes.");

	assert!(rewritten.contains("origin: geometry::point(2)"), "{rewritten}");
	assert!(!rewritten.contains("geometry::geometry::"), "{rewritten}");
}

#[test]
fn import004_reuses_module_bindings_planned_for_other_imports() {
	let original = r#"pub fn observe() {}
pub mod conversations {
    pub mod tests {
        pub fn connected() {}
        pub fn dispatched() {}
        pub fn archived() {}
        pub fn replied() {}
    }
}
#[cfg(test)]
mod tests {
    use super::observe;
    use crate::conversations::tests::{connected, dispatched};
    #[test]
    fn connects_session() {
        observe();
        connected();
        dispatched();
    }
    #[test]
    fn archives_session() {
        use crate::conversations::tests::{archived, replied};
        archived();
        replied();
    }
}
"#;
	let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Timestamp.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-shared-plan-{}-{now}", process::id()));
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

	assert!(rewritten.contains("tests::connected()"), "{rewritten}");

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
fn import004_preserves_relative_parent_depth() {
	let original =
		"fn sample() { super::super::render(); wrapper!(super::super::super::render()); }";
	let (rewritten, _, _, _) =
		engine::apply_fix_passes(Path::new("relative_parent.rs"), original, true)
			.expect("Apply fixes.");

	assert!(rewritten.contains("super::super::render()"), "{rewritten}");
	assert!(rewritten.contains("super::super::super::render()"), "{rewritten}");
	assert!(!rewritten.contains("use super"), "{rewritten}");
}

#[test]
fn import004_checks_relative_module_binding_collisions() {
	let original =
		"use crate::conversations::tests;\nfn sample() { super::super::tests::profiles(); }";
	let (rewritten, _, _, _) =
		engine::apply_fix_passes(Path::new("relative_collision.rs"), original, true)
			.expect("Apply fixes.");

	assert!(rewritten.contains("super::super::tests::profiles()"), "{rewritten}");
	assert!(!rewritten.contains("use super"), "{rewritten}");
}

#[cfg(unix)]
#[test]
fn import004_coordinates_direct_imports_and_shortened_paths() {
	let original = r#"#[cfg(test)]
mod tests {
    use std::os::unix::fs::{MetadataExt, symlink};
    #[test]
    fn sample() {
        let _ = std::fs::read("file");
        let _ = std::fs::metadata("file").map(|metadata| metadata.mode());
        let _ = symlink("a", "b");
    }
}
"#;
	let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Timestamp.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-planned-imports-{}-{now}", process::id()));

	fs::create_dir_all(&root).expect("Create fixture directory.");

	let path = root.join("fixture.rs");
	let (rewritten, _, _, _) =
		engine::apply_fix_passes(&path, original, true).expect("Apply fixes.");

	assert!(rewritten.contains("fs::symlink(\"a\", \"b\")"), "{rewritten}");

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
fn import004_fix_prefers_parent_module_keep_alive_style_for_eyre() {
	let original = r#"
use color_eyre::eyre::{Context as _, Result, eyre};

fn sample() -> Result<()> {
	Err(eyre!("boom"))
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import004_eyre_parent_module_fix.rs"),
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
	assert!(
		rewritten.contains("use color_eyre::eyre::{self, Context as _, Result};"),
		"{rewritten}"
	);
	assert!(rewritten.contains("Err(eyre::eyre!(\"boom\"))"));
	assert!(!rewritten.contains("use color_eyre::eyre::{Context as _, Result, eyre};"));
}

#[test]
fn import009_autofixes_when_different_qualified_symbol_path_exists() {
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
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009" && v.fixable));
	assert!(edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-009"));

	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(!rewritten.contains("use a::A;"));
	assert!(rewritten.contains("fn sample(a: a::A, aa: b::A)"));
}

#[test]
fn import009_autofixes_when_qualified_and_imported_paths_differ() {
	let original = r#"
use qdrant_client::qdrant::Value;

struct Payload {
	raw: serde_json::Value,
}

fn build_value() -> Value {
	Value::from(1_i64)
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import009_different_qualified_path.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009" && v.fixable));
	assert!(edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-009"));

	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(!rewritten.contains("use qdrant_client::qdrant::Value;"));
	assert!(rewritten.contains("fn build_value() -> qdrant_client::qdrant::Value"));
	assert!(rewritten.contains("qdrant_client::qdrant::Value::from(1_i64)"));
}

#[test]
fn import009_rewrites_unqualified_derive_symbol() {
	let original = r#"
use foo::Bar;
#[serde(skip_serializing_if = "core::ops::Not::not")]
#[derive(Bar)]
struct Row;

#[derive(foo :: Bar)]
struct Qualified;

fn run() {
	foo::Bar::make();
}
"#;
	let mut rewritten = original.to_owned();
	let mut applied_count = 0_usize;

	for _ in 0..4 {
		let ctx = shared::read_file_context_from_text(
			Path::new("import009_unqualified_derive_symbol.rs"),
			rewritten.clone(),
		)
		.expect("context")
		.expect("has ctx");
		let (_violations, edits) = rules::collect_violations(&ctx, true);

		if edits.is_empty() {
			break;
		}

		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		applied_count += applied;
	}

	assert!(applied_count > 0);
	assert!(rewritten.contains("#[derive(foo::Bar)]"));
	assert!(!rewritten.contains("#[derive(Bar)]"));
	assert!(!rewritten.contains("use foo::Bar;"));
	assert!(!rewritten.contains("foo::foo::Bar"));
	assert!(rewritten.lines().any(|line| line.contains("derive") && line.contains("foo :: Bar")));
	assert!(rewritten.contains("foo::Bar::make()"));

	let skip_serializing_if_lines: Vec<&str> = rewritten
		.lines()
		.filter(|line| line.contains("skip_serializing_if"))
		.map(str::trim)
		.collect();

	assert_eq!(skip_serializing_if_lines.len(), 1);
	assert_eq!(
		skip_serializing_if_lines[0],
		r#"#[serde(skip_serializing_if = "core::ops::Not::not")]"#
	);
}

#[test]
fn import009_fix_rewrites_ambiguous_pubfi_ai_usage_with_grouped_import_kept() {
	let original = r#"
use pubfi_ai::Usage;
use pubfi_ai::{Cost, Inference};

fn normalize(
	input: Usage,
	rig_usage: rig::completion::Usage,
	qualified: pubfi_ai::Usage,
) -> Usage {
	let _ = (std::mem::size_of::<Cost>(), std::mem::size_of::<Inference>());
	let _ = rig_usage;
	let _ = qualified;
	let local: Usage = input;
	local
}
"#;
	let (rewritten, applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import009_pubfi_ai_usage_grouped_import.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(applied_count > 0, "Rewritten:\n{rewritten}");
	assert!(!rewritten.contains("use pubfi_ai::Usage;"), "Rewritten:\n{rewritten}");
	assert!(rewritten.contains("use pubfi_ai::{Cost, Inference};"), "Rewritten:\n{rewritten}");
	assert!(rewritten.contains("input: pubfi_ai::Usage"), "Rewritten:\n{rewritten}");
	assert!(rewritten.contains(") -> pubfi_ai::Usage"), "Rewritten:\n{rewritten}");
	assert!(rewritten.contains("let local: pubfi_ai::Usage = input;"), "Rewritten:\n{rewritten}");
	assert!(rewritten.contains("rig_usage: rig::completion::Usage"), "Rewritten:\n{rewritten}");
	assert!(rewritten.contains("qualified: pubfi_ai::Usage"), "Rewritten:\n{rewritten}");
}

#[test]
fn import009_fix_rewrites_pubfi_ai_usage_snippet_and_is_idempotent() {
	let original = r#"
use pubfi_ai::Usage;
use pubfi_ai::{Cost, Inference};

#[test]
fn usage_from_rig_skips_empty() {
	let usage = rig::completion::Usage { input_tokens: 0, output_tokens: 0, total_tokens: 0 };

	assert!(Usage::from_rig(usage).is_none());
}

#[test]
fn usage_from_rig_maps_tokens() {
	let usage = rig::completion::Usage { input_tokens: 12, output_tokens: 34, total_tokens: 46 };
	let mapped = pubfi_ai::Usage::from_rig(usage).expect("Expected usage to map.");

	assert_eq!(mapped, Usage { input_tokens: 12, output_tokens: 34, total_tokens: 46 });
}

#[test]
fn usage_merge_accumulates() {
	let mut usage = Some(pubfi_ai::Usage { input_tokens: 10, output_tokens: 5, total_tokens: 15 });

	pubfi_ai::Usage::merge(&mut usage, Some(pubfi_ai::Usage { input_tokens: 2, output_tokens: 3, total_tokens: 5 }));

	assert_eq!(usage, Some(Usage { input_tokens: 12, output_tokens: 8, total_tokens: 20 }));
}

#[test]
fn inference_record_usage_accumulates() {
	let mut usage = None;
	let mut inference = Inference::new(
		String::from("ok"),
		Some(pubfi_ai::Usage { input_tokens: 7, output_tokens: 9, total_tokens: 16 }),
	);

	inference.cost = Some(Cost::usd(0.00012));

	let mut cost = None;
	let output = inference.record_usage(&mut usage, &mut cost);

	assert_eq!(output, "ok");
	assert_eq!(usage, Some(Usage { input_tokens: 7, output_tokens: 9, total_tokens: 16 }));
	assert_eq!(cost, Some(Cost::usd(0.00012)));
}
"#;
	let (rewritten, applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import009_pubfi_ai_usage_snippet_idempotence.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(applied_count > 0, "Rewritten:\n{rewritten}");
	assert!(!rewritten.contains("use pubfi_ai::Usage;"), "Rewritten:\n{rewritten}");
	assert!(rewritten.contains("assert!(pubfi_ai::Usage::from_rig(usage).is_none());"));
	assert!(rewritten.contains(
		"assert_eq!(mapped, pubfi_ai::Usage { input_tokens: 12, output_tokens: 34, total_tokens: 46 });"
	));
	assert!(rewritten.contains(
			"assert_eq!(usage, Some(pubfi_ai::Usage { input_tokens: 12, output_tokens: 8, total_tokens: 20 }));"
		));
	assert!(rewritten.contains(
			"assert_eq!(usage, Some(pubfi_ai::Usage { input_tokens: 7, output_tokens: 9, total_tokens: 16 }));"
		));

	let (
		repeated,
		repeated_applied_count,
		_repeated_had_import_shortening_edits,
		_repeated_had_let_mut_reorder_edits,
	) = engine::apply_fix_passes(
		Path::new("import009_pubfi_ai_usage_snippet_idempotence.rs"),
		&rewritten,
		true,
	)
	.expect("repeat apply fix passes");

	assert_eq!(repeated_applied_count, 0, "Repeated:\n{repeated}");
	assert_eq!(repeated, rewritten);
}

#[test]
fn import009_autofixes_result_when_std_result_is_also_used() {
	let original = r#"
use color_eyre::Result;

fn forward_std_result(value: std::result::Result<(), ()>) -> std::result::Result<(), ()> {
	value
}

async fn acquire_queue() -> Result<()> {
	let _ = std::result::Result::<(), ()>::Ok(());
	Ok(())
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import009_result_std_result.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009" && v.fixable));
	assert!(edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-009"));

	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(!rewritten.contains("use color_eyre::Result;"));
	assert!(rewritten.contains("async fn acquire_queue() -> color_eyre::Result<()>"));
	assert!(rewritten.contains("fn forward_std_result(value: std::result::Result<(), ()>)"));
	assert!(rewritten.contains("std::result::Result::<(), ()>::Ok(())"));
}

#[test]
fn import009_autofixes_crawler_like_std_tokio_reqwest_symbol_conflicts() {
	let original = r#"
use std::io::Error;
use std::result::Result;
use std::time::Instant;

fn dispatch() -> Result<(), Error> {
	let _tokio_now = tokio::time::Instant::now();
	let _reqwest_err = reqwest::Error::from(std::io::Error::other("boom"));
	let _reqwest_ok = reqwest::Result::<(), reqwest::Error>::Ok(());
	let _instant = Instant::now();
	let _error = Error::other("local");
	let _result = Result::<(), Error>::Ok(());
	Ok(())
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import009_crawler_std_tokio_reqwest_conflict.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009" && v.fixable));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-009"));

	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import009_crawler_std_tokio_reqwest_conflict.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(!rewritten.contains("use std::io::Error;"));
	assert!(!rewritten.contains("use std::result::Result;"));
	assert!(!rewritten.contains("use std::time::Instant;"));
	assert!(rewritten.contains("fn dispatch() -> std::result::Result<(), std::io::Error>"));
	assert!(rewritten.contains("let _instant = std::time::Instant::now();"));
	assert!(rewritten.contains("let _error = std::io::Error::other(\"local\");"));
	assert!(rewritten.contains("let _result = std::result::Result::<(), std::io::Error>::Ok(());"));
	assert!(rewritten.contains("tokio::time::Instant::now()"));
	assert!(rewritten.contains("reqwest::Error::from(std::io::Error::other(\"boom\"))"));
	assert!(rewritten.contains("reqwest::Result::<(), reqwest::Error>::Ok(())"));
}

#[test]
fn import009_fix_applies_for_pubfi_crawler_dispatcher_braced_segments() {
	let original = r#"
use std::{
	collections::{HashSet, VecDeque, hash_map::Entry},
	error::Error,
	fmt::{Display, Formatter},
	time::Instant,
};
use color_eyre::{Report, eyre};
use color_eyre::Result;

#[derive(Debug)]
struct DispatchError;

impl Display for DispatchError {
	fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
		write!(f, "{f}")
	}
}

impl Error for DispatchError {}

fn execute(deadline: Instant) -> Result<(), Report> {
	let _ = HashSet::<String>::new();
	let _ = VecDeque::<String>::new();
	let _ = std::mem::size_of::<Entry<String, String>>();
	let _ = eyre!("boom");
	let _tokio_deadline = tokio::time::Instant::from_std(deadline);
	let _now = Instant::now();
	let _reqwest_error = reqwest::Error::from(std::io::Error::other("boom"));
	let _std_result = std::result::Result::<(), reqwest::Error>::Ok(());
	Ok(())
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import009_pubfi_crawler_dispatcher.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	for symbol in ["Error", "Instant", "Result"] {
		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-009"
				&& v.fixable
				&& v.message.contains(&format!("`{symbol}`"))
		}));
	}

	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-009"));

	let (rewritten, applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import009_pubfi_crawler_dispatcher.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(applied_count > 0);
	assert!(!rewritten.contains("error::Error,"));
	assert!(!rewritten.contains("time::Instant,"));
	assert!(rewritten.contains("impl std::error::Error for DispatchError {}"));
	assert!(rewritten.contains("let _now = std::time::Instant::now();"));
}

#[test]
fn import009_fix_applies_for_pubfi_gateway_service_referral_symbols() {
	let original = r#"
use color_eyre::Result;
use tonic::{Request, Response, Status};

use crate::{
	grpc::{
		GetReferralCodeRequest, GetReferralRelationByInviteeRequest, UpsertReferralCodeRequest,
		UpsertReferralRelationRequest,
	},
};
use crate::grpc::ReferralCode;
use crate::grpc::ReferralRelation;

async fn get_referral_code(
	request: Request<GetReferralCodeRequest>,
) -> Result<Response<ReferralCode>, Status> {
	let UpsertReferralCodeRequest { id, referral_code, referrer } = UpsertReferralCodeRequest {
		id: String::new(),
		referral_code: String::new(),
		referrer: String::new(),
	};
	let _record = crate::types::ReferralCode { id, referral_code, referrer, created_at: now() };

	let GetReferralCodeRequest { id: _ } = request.into_inner();

	Ok(Response::new(ReferralCode {
		id: String::new(),
		referral_code: String::new(),
		referrer: String::new(),
		created_at: None,
	}))
}

async fn get_referral_relation(
	request: Request<GetReferralRelationByInviteeRequest>,
) -> Result<Response<ReferralRelation>, Status> {
	let UpsertReferralRelationRequest { id, referral_code, invitee } = UpsertReferralRelationRequest {
		id: String::new(),
		referral_code: String::new(),
		invitee: String::new(),
	};
	let _relation = crate::types::ReferralRelation { id, referral_code, invitee, created_at: now() };

	let GetReferralRelationByInviteeRequest { invitee: _ } = request.into_inner();

	Ok(Response::new(ReferralRelation {
		id: String::new(),
		referral_code: String::new(),
		invitee: String::new(),
		created_at: None,
	}))
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import009_pubfi_gateway_service.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	for symbol in ["ReferralCode", "ReferralRelation"] {
		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-009"
				&& v.fixable
				&& v.message.contains(&format!("`{symbol}`"))
		}));
	}

	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-009"));

	let (rewritten, applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(Path::new("import009_pubfi_gateway_service.rs"), original, true)
			.expect("apply fix passes");

	assert!(applied_count > 0);
	assert!(rewritten.contains("Response::new(crate::grpc::ReferralCode {"));
	assert!(rewritten.contains("Response::new(crate::grpc::ReferralRelation {"));
}

#[test]
fn import009_fix_applies_for_pubfi_gateway_service_large_use_block_shape() {
	let original = r#"
use color_eyre::Result;
use tonic::{Request, Response, Status};

use crate::{auth, context::Context, db::{mail_code, referral, user_data}, error::SqlxStatusExt as _, grpc::{DeleteUserDataRequest, GetReferralCodeRequest, GetReferralRelationByInviteeRequest, GetReferralRelationsByCodeRequest, GetReferralRelationsByCodeResponse, GetUserDataRequest, SendMailCodeRequest, UpdateUserDataRequest, UpdateUserDataResponse, UpsertReferralCodeRequest, UpsertReferralRelationRequest, UserData, VerifyMailCodeRequest}, mail, rate_limit::{RateLimitAction, RateLimitKeyType, RateLimitWindow}, types::{UserDataUpdate}};
use crate::grpc::ReferralCode;
use crate::grpc::ReferralRelation;

async fn get_referral_code(
	request: Request<GetReferralCodeRequest>,
) -> Result<Response<ReferralCode>, Status> {
	let UpsertReferralCodeRequest { id, referral_code, referrer } = UpsertReferralCodeRequest {
		id: String::new(),
		referral_code: String::new(),
		referrer: String::new(),
	};
	let _record = crate::types::ReferralCode { id, referral_code, referrer, created_at: now() };

	let GetReferralCodeRequest { id: _ } = request.into_inner();

	Ok(Response::new(ReferralCode {
		id: String::new(),
		referral_code: String::new(),
		referrer: String::new(),
		created_at: None,
	}))
}

async fn get_referral_relation(
	request: Request<GetReferralRelationByInviteeRequest>,
) -> Result<Response<ReferralRelation>, Status> {
	let UpsertReferralRelationRequest { id, referral_code, invitee } = UpsertReferralRelationRequest {
		id: String::new(),
		referral_code: String::new(),
		invitee: String::new(),
	};
	let _relation = crate::types::ReferralRelation { id, referral_code, invitee, created_at: now() };

	let GetReferralRelationByInviteeRequest { invitee: _ } = request.into_inner();

	Ok(Response::new(ReferralRelation {
		id: String::new(),
		referral_code: String::new(),
		invitee: String::new(),
		created_at: None,
	}))
}
"#;
	let (rewritten, applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import009_pubfi_gateway_service_large_use_shape.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(applied_count > 0, "Rewritten:\n{rewritten}");
	assert!(!rewritten.contains("use crate::grpc::ReferralCode;"), "Rewritten:\n{rewritten}");
	assert!(!rewritten.contains("use crate::grpc::ReferralRelation;"), "Rewritten:\n{rewritten}");
	assert!(rewritten.contains("Result<Response<crate::grpc::ReferralCode>, Status>"));
	assert!(rewritten.contains("Result<Response<crate::grpc::ReferralRelation>, Status>"));
	assert!(rewritten.contains("Response::new(crate::grpc::ReferralCode {"));
	assert!(rewritten.contains("Response::new(crate::grpc::ReferralRelation {"));
}

#[test]
fn import009_fix_applies_when_result_line_also_breaks_import_group_order() {
	let original = r#"
use std::time::Duration;
use reqwest::Response;

use crate::http::dispatcher::RequestSpec;
use color_eyre::Result;

fn execute(_spec: RequestSpec) -> Result<Response> {
	let _std_result = std::result::Result::<(), reqwest::Error>::Ok(());
	let _duration = Duration::from_millis(1);
	todo!()
}
"#;
	let (rewritten, applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import009_result_with_import001_conflict.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(applied_count > 0, "Rewritten:\n{rewritten}");
	assert!(!rewritten.contains("use color_eyre::Result;"), "Rewritten:\n{rewritten}");
	assert!(rewritten.contains("fn execute(_spec: RequestSpec) -> color_eyre::Result<Response>"));
}

#[test]
fn import009_fix_applies_for_pubfi_crawler_result_import_after_workspace_group() {
	let original = r#"
use std::{collections::{HashSet, VecDeque, hash_map::Entry}, fmt::{Display, Formatter}, sync::atomic::{AtomicUsize, Ordering}, task::{Context, Poll}};
use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc, time::Duration};

use derive_setters::Setters;
use reqwest::{Method, Request, Response, Url, header::HeaderName};
use tokio::sync::{
	AcquireError, OwnedSemaphorePermit,
	mpsc::{self, Receiver},
	oneshot,
};
use tower::{
	Service,
	util::{BoxCloneService, ServiceExt as _},
};
use tracing::Instrument;
use color_eyre::{Report, eyre};
use reqwest::Client;
use sqlx::types::Uuid;
use tokio::sync::{Mutex, Semaphore};

use crate::http::{
	control::{AdaptiveState, CircuitBreaker, Feedback, HostControl, QpsLimiter},
	params::{AdaptiveParams, DispatcherParams},
};
use pubfi_obs::{RuntimeEvent, RuntimeLog};
use pubfi_util::{Backoff, RetryError};

use color_eyre::Result;

pub type GatewayService = BoxCloneService<RequestSpec, Response, Report>;

#[derive(Debug)]
enum DispatchError {
	QueueFull,
}
impl Display for DispatchError {
	fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
		write!(f, "{f}")
	}
}
impl std::error::Error for DispatchError {}

fn poll_ready(_: &mut Context) -> Poll<std::result::Result<(), Report>> {
	let _ = std::result::Result::<(), Report>::Ok(());
	Poll::Ready(Ok(()))
}

async fn acquire_queue() -> Result<OwnedSemaphorePermit> {
	let _ = (
		AdaptiveState::default(),
		CircuitBreaker::new(0),
		Feedback::default(),
		HostControl::default(),
		QpsLimiter::default(),
		AdaptiveParams::default(),
		DispatcherParams::default(),
		Backoff::default(),
		RetryError::default(),
	);
	let _ = (
		HashSet::<String>::new(),
		VecDeque::<String>::new(),
		std::mem::size_of::<Entry<String, String>>(),
		AtomicUsize::new(0),
		Ordering::Relaxed,
		Arc::new(AtomicUsize::new(1)),
		Duration::from_millis(1),
		Method::GET,
		Request::new(Method::GET, Url::parse("https://example.com").expect("url")),
		Response::new(reqwest::Body::default()),
		HeaderName::from_static("x-test"),
		AcquireError::NoPermits,
		Receiver::<()>::default(),
		oneshot::channel::<()>().0,
		Client::new(),
		Uuid::nil(),
		Mutex::new(()),
		Semaphore::new(1),
		RuntimeEvent::Shutdown,
		RuntimeLog::default(),
		eyre!("boom"),
	);
	todo!()
}
"#;
	let (rewritten, applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import009_pubfi_crawler_result_after_workspace_group.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(applied_count > 0, "Rewritten:\n{rewritten}");
	assert!(!rewritten.contains("use color_eyre::Result;"), "Rewritten:\n{rewritten}");
	assert!(
		rewritten.contains("async fn acquire_queue() -> color_eyre::Result<OwnedSemaphorePermit>")
	);
}

#[test]
fn import009_removes_redundant_import_when_only_qualified_same_path_usage_remains() {
	let original = r#"
use crate::{
	structured_fields::{StructuredFields, upsert_structured_fields_tx},
};

fn run_ops() {
	let _ = StructuredFields::default();
	crate::structured_fields::upsert_structured_fields_tx();
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import009_remove_redundant_same_path_import.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(!rewritten.contains("upsert_structured_fields_tx,"));
	assert!(!rewritten.contains("use crate::structured_fields::upsert_structured_fields_tx"));
	assert!(
		rewritten.contains("use crate::{structured_fields::{self, StructuredFields}};"),
		"{rewritten}"
	);
	assert!(rewritten.contains("structured_fields::upsert_structured_fields_tx();"));
	assert!(!rewritten.contains("crate::structured_fields::upsert_structured_fields_tx();"));
}

#[test]
fn import004_multi_pass_removes_multiple_free_function_imports_without_leftover_unused_imports() {
	let original = r#"
use crate::{
	structured_fields::{StructuredFields, upsert_structured_fields_tx, validate_structured_fields},
};

fn run_ops() {
	let _ = StructuredFields::default();
	upsert_structured_fields_tx();
	validate_structured_fields();
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import004_remove_multiple_free_functions.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(
		rewritten.contains("use crate::{structured_fields::{self, StructuredFields}};"),
		"{rewritten}"
	);
	assert!(rewritten.contains("structured_fields::upsert_structured_fields_tx();"));
	assert!(rewritten.contains("structured_fields::validate_structured_fields();"));
	assert!(!rewritten.contains("use crate::structured_fields::upsert_structured_fields_tx"));
	assert!(!rewritten.contains("use crate::structured_fields::validate_structured_fields"));
	assert!(!rewritten.contains("upsert_structured_fields_tx,"));
	assert!(!rewritten.contains("validate_structured_fields,"));
}

#[test]
fn import004_rewrites_remaining_free_function_import_when_parent_module_is_already_imported() {
	let original = r#"
use crate::{
	structured_fields::{self, StructuredFields, validate_structured_fields},
};

fn run_ops() {
	let _ = StructuredFields::default();
	structured_fields::upsert_structured_fields_tx();
	validate_structured_fields();
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import004_remaining_free_function_with_existing_parent_module.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert!(
		rewritten.contains("use crate::{structured_fields::{self, StructuredFields}};"),
		"{rewritten}"
	);
	assert!(rewritten.contains("structured_fields::upsert_structured_fields_tx();"));
	assert!(rewritten.contains("structured_fields::validate_structured_fields();"));
	assert!(!rewritten.contains("crate::structured_fields::validate_structured_fields();"));
	assert!(!rewritten.contains("validate_structured_fields,"));
}

#[test]
fn import009_fix_rewrites_non_importable_generic_root_use() {
	let original = r#"
use serde::Serializer;
use S::Error;
use S::Ok;

pub fn serialize<S>(serializer: S) -> Result<Ok, Error>
where
	S: Serializer,
{
	let _ = serializer;
	todo!()
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import009_non_importable_generic_root.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	eprintln!(
		"use_items={:?}",
		ctx.top_items
			.iter()
			.filter(|item| item.kind == shared::TopKind::Use)
			.map(|item| (item.raw.clone(), item.use_path.clone()))
			.collect::<Vec<_>>()
	);
	eprintln!(
		"violations={:?}",
		violations.iter().map(|v| (v.rule, v.message.clone(), v.fixable)).collect::<Vec<_>>()
	);
	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009" && v.fixable));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-009"));

	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(!rewritten.contains("use S::Error;"));
	assert!(!rewritten.contains("use S::Ok;"));
	assert!(rewritten.contains("-> Result<S::Ok, S::Error>"));
}

#[test]
fn import009_fix_rewrites_non_importable_self_root_use() {
	let original = r#"
use Self::Error;

trait Task {
	type Error;
	fn run(&self) -> Result<(), Error>;
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import009_non_importable_self_root.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009" && v.fixable));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-009"));

	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(!rewritten.contains("use Self::Error;"));
	assert!(rewritten.contains("-> Result<(), Self::Error>"));
}

#[test]
fn import009_fix_rewrites_std_fmt_result_import_to_qualified_non_generic_uses() {
	let original = r#"
use std::fmt::Result;

fn format_output() -> Result {
	Ok(())
}

fn parse_value() -> Result<u8, &'static str> {
	Ok(1)
}
"#;
	let mut rewritten = original.to_owned();

	for _ in 0..4 {
		let ctx = shared::read_file_context_from_text(
			Path::new("import009_std_fmt_result.rs"),
			rewritten.clone(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = rules::collect_violations(&ctx, true);

		if edits.is_empty() {
			assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009"));

			break;
		}

		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
	}

	assert!(!rewritten.contains("use std::fmt::Result;"));
	assert!(rewritten.contains("fn format_output() -> std::fmt::Result"));
	assert!(rewritten.contains("fn parse_value() -> Result<u8, &'static str>"));
}

#[test]
fn import009_fix_removes_std_fmt_result_import_when_generic_result_is_used() {
	let original = r#"
use std::fmt::Result;

fn parse_value() -> Result<u8, &'static str> {
	Ok(1)
}
"#;
	let mut rewritten = original.to_owned();

	for _ in 0..4 {
		let ctx = shared::read_file_context_from_text(
			Path::new("import009_std_fmt_result_generic_only.rs"),
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

	assert!(!rewritten.contains("use std::fmt::Result;"));
	assert!(rewritten.contains("fn parse_value() -> Result<u8, &'static str>"));
}

#[test]
fn import009_autofix_applies_for_unqualified_type_with_qualified_same_path_value_constructor() {
	let original = r#"
use serde_json::Value;

fn build_payload() -> Value {
	serde_json::Value::String("ok".to_string())
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import009_unqualified_type_qualified_value_constructor.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);
	let import009_edits =
		edits.into_iter().filter(|edit| edit.rule == "RUST-STYLE-IMPORT-009").collect::<Vec<_>>();
	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, import009_edits).expect("apply edits");

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009" && v.fixable));
	assert!(applied > 0);
	assert!(!rewritten.contains("use serde_json::Value;"));
	assert!(rewritten.contains("fn build_payload() -> serde_json::Value"));
	assert!(rewritten.contains("serde_json::Value::String(\"ok\".to_string())"));
}

#[test]
fn import009_autofix_applies_for_unqualified_error_type_with_qualified_same_path_variant_usage() {
	let original = r#"
use pubfi_search::Error;

struct SearchFailure {
	source: Error,
}

fn normalize_error(input: Error) -> Error {
	let _ = input;
	pubfi_search::Error::NotFound
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import009_unqualified_error_type_qualified_variant.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);
	let import009_edits =
		edits.into_iter().filter(|edit| edit.rule == "RUST-STYLE-IMPORT-009").collect::<Vec<_>>();
	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, import009_edits).expect("apply edits");

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009" && v.fixable));
	assert!(applied > 0);
	assert!(!rewritten.contains("use pubfi_search::Error;"));
	assert!(rewritten.contains("source: pubfi_search::Error"));
	assert!(
		rewritten.contains("fn normalize_error(input: pubfi_search::Error) -> pubfi_search::Error")
	);
	assert!(rewritten.contains("pubfi_search::Error::NotFound"));
}

#[test]
fn import009_does_not_autofix_when_symbol_has_standalone_value_uses() {
	let original = r#"
use pubfi_search::query;

fn validate_percolator_query(query: &serde_json::Value) {
	let _ = sqlx::query("SELECT 1");
	let _ = query.get("bool");
	let _dsl = query::build_percolate_dsl_from_constraints;
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import009_standalone_value_uses.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009"));
	assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-009"));
}

#[test]
fn import009_ignores_lowercase_module_symbol_with_qualified_module_sibling() {
	let original = r#"
use crate::service::feed::feeds;

fn publish() {
	let _ = feeds::get_owned_feed;
	let _ = store::feed::feeds::update_by_id;
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import009_lowercase_module_symbol.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009"));
	assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-009"));
}

#[test]
fn import009_fix_rewrites_qualified_associated_fn_receiver_symbol() {
	let original = r#"
use crate::cli::PercolateFilterMode;

fn normalize(filter_mode: PercolateFilterMode) {
	let _ = pubfi_query::PercolateFilterMode::from(filter_mode);
}
"#;
	let expected = r#"
use crate::cli;

fn normalize(filter_mode: crate::cli::PercolateFilterMode) {
	let _ = pubfi_query::PercolateFilterMode::from(filter_mode);
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(
			Path::new("import009_associated_fn_receiver_symbol.rs"),
			original,
			true,
		)
		.expect("apply fix passes");

	assert_eq!(rewritten.trim_start_matches('\n'), expected.trim_start_matches('\n'));
}

#[test]
fn import009_fix_rewrites_dyn_trait_type_when_associated_receiver_path_conflicts() {
	let original = r#"
use age::Recipient;

fn encrypt(recipients: &[String]) {
	let recipients = recipients.iter().map(|s| age::x25519::Recipient::from_str(s).unwrap());
	let _ = recipients.map(|r| r as &dyn Recipient).count();
}
"#;
	let expected = r#"
fn encrypt(recipients: &[String]) {
	let recipients = recipients.iter().map(|s| age::x25519::Recipient::from_str(s).unwrap());
	let _ = recipients.map(|r| r as &dyn age::Recipient).count();
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(Path::new("import009_age_recipient_conflict.rs"), original, true)
			.expect("apply fix passes");

	assert_eq!(rewritten.trim_start_matches('\n'), expected.trim_start_matches('\n'));
}

#[test]
fn import009_cycle_guard_blocks_unqualified_value_with_qualified_type_same_path() {
	let original = r#"
use serde_json::Value;

fn build_payload() -> serde_json::Value {
	Value::String("ok".to_string())
}
"#;
	let expected = r#"
use serde_json::Value;

fn build_payload() -> Value {
	Value::String("ok".to_string())
}
"#;
	let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		engine::apply_fix_passes(Path::new("import009_import008_value_cycle.rs"), original, true)
			.expect("apply fix passes");

	assert_eq!(rewritten.trim_start_matches('\n'), expected.trim_start_matches('\n'));

	let (
		repeated,
		repeated_applied_count,
		_repeated_had_import_shortening_edits,
		_repeated_had_let_mut_reorder_edits,
	) = engine::apply_fix_passes(Path::new("import009_import008_value_cycle.rs"), &rewritten, true)
		.expect("repeat apply fix passes");

	assert_eq!(repeated_applied_count, 0);
	assert_eq!(repeated.trim_start_matches('\n'), rewritten.trim_start_matches('\n'));
}

#[test]
fn import009_fix_rewrites_struct_literal_value_path_when_symbol_is_ambiguous() {
	let original = r#"
use crate::types::ReferralCode;

fn upsert(input: crate::grpc::ReferralCode) -> crate::types::ReferralCode {
	let _ = input;
	let record = ReferralCode { id: "x".to_string() };
	record
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import009_struct_literal_value_path.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009" && v.fixable));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-009"));

	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(!rewritten.contains("use crate::types::ReferralCode;"));
	assert!(
		rewritten.contains("let record = crate::types::ReferralCode { id: \"x\".to_string() };")
	);
}

#[test]
fn import009_fix_rewrites_struct_literal_value_path_from_braced_use_with_alias_sibling() {
	let original = r#"
use crate::{error::SqlxStatusExt as _, grpc::{GetReferralCodeRequest}, types::{ReferralCode}};

fn upsert(input: crate::grpc::ReferralCode) -> crate::types::ReferralCode {
	let _ = input;
	let _ = GetReferralCodeRequest { id: String::new() };
	let record = ReferralCode { id: "x".to_string() };
	record
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import009_struct_literal_braced_alias_sibling.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009" && v.fixable));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-009"));

	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(rewritten.contains("SqlxStatusExt as _"));
	assert!(!rewritten.contains("types::{ReferralCode}"));
	assert!(
		rewritten.contains("let record = crate::types::ReferralCode { id: \"x\".to_string() };")
	);
}

#[test]
fn import009_fix_rewrites_gateway_style_struct_literal_from_large_braced_use_tree() {
	let original = r#"
use crate::{auth, context::Context, db::{mail_code, referral, user_data}, error::SqlxStatusExt as _, grpc::{DeleteUserDataRequest, GetReferralCodeRequest, GetReferralRelationByInviteeRequest, GetReferralRelationsByCodeRequest, GetReferralRelationsByCodeResponse, GetUserDataRequest, SendMailCodeRequest, UpdateUserDataRequest, UpdateUserDataResponse, UpsertReferralCodeRequest, UpsertReferralRelationRequest, UserData, VerifyMailCodeRequest}, mail, rate_limit::{RateLimitAction, RateLimitKeyType, RateLimitWindow}, types::{ReferralCode, ReferralRelation, UserDataUpdate}};

fn upsert_referral_code(request: crate::grpc::ReferralCode) -> crate::grpc::ReferralCode {
	let _ = (
		auth::client_app_from_request,
		mail_code::delete_by_id,
		referral::insert_code,
		user_data::get_or_insert,
		mail::generate_code,
		RateLimitAction::Send,
		RateLimitKeyType::Mail,
		RateLimitWindow { window_seconds: 1, limit: 1 },
		DeleteUserDataRequest {},
		GetReferralCodeRequest {},
		GetReferralRelationByInviteeRequest {},
		GetReferralRelationsByCodeRequest {},
		GetReferralRelationsByCodeResponse { relations: Vec::new() },
		GetUserDataRequest {},
		SendMailCodeRequest {},
		UpdateUserDataRequest {},
		UpdateUserDataResponse {},
		UpsertReferralCodeRequest {},
		UpsertReferralRelationRequest {},
		UserData {},
		VerifyMailCodeRequest {},
	);
	let _ = (
		ReferralRelation { id: String::new(), referral_code: String::new(), invitee: String::new(), created_at: now() },
		UserDataUpdate { user_id: String::new(), user_data: None },
	);
	let record = ReferralCode { id: String::new(), referral_code: String::new(), referrer: String::new(), created_at: now() };
	let _ = Context::new();
	let _ = request;
	crate::grpc::ReferralCode { id: record.id, referral_code: record.referral_code, referrer: record.referrer, created_at: None }
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import009_gateway_style_large_use.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009" && v.fixable));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-009"));

	let mut rewritten = original.to_owned();
	let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(!rewritten.contains("types::{ReferralCode,"));
	assert!(rewritten.contains("let record = crate::types::ReferralCode {"));
}

#[test]
fn import009_fix_applies_for_gateway_symbols_with_separate_imports_and_conflicting_paths() {
	let original = r#"
use crate::types::ReferralCode;
use crate::types::ReferralRelation;

fn upsert_referral_code(
	request: crate::grpc::ReferralCode,
	request_relation: crate::grpc::ReferralRelation,
) -> crate::grpc::ReferralCode {
	let _ = request_relation;
	let _relation = ReferralRelation {
		id: String::new(),
		referral_code: String::new(),
		invitee: String::new(),
		created_at: now(),
	};
	let record = ReferralCode {
		id: String::new(),
		referral_code: String::new(),
		referrer: String::new(),
		created_at: now(),
	};
	let _type_conflict_code: crate::grpc::ReferralCode = request;
	let _type_conflict_relation: crate::grpc::ReferralRelation = request_relation;
	crate::grpc::ReferralCode {
		id: record.id,
		referral_code: record.referral_code,
		referrer: record.referrer,
		created_at: None,
	}
}
"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import009_gateway_separate_imports_conflicting_paths.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-009" && v.fixable && v.message.contains("`ReferralCode`")
	}));
	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-009" && v.fixable && v.message.contains("`ReferralRelation`")
	}));
	assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-009"));

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

	assert!(applied > 0);
	assert!(!rewritten.contains("use crate::types::ReferralCode;"));
	assert!(!rewritten.contains("use crate::types::ReferralRelation;"));
	assert!(rewritten.contains("let _relation = crate::types::ReferralRelation {"));
	assert!(rewritten.contains("let record = crate::types::ReferralCode {"));
}

#[test]
fn import009_fix_applies_when_gateway_imports_are_eof_and_same_line() {
	let original = r#"fn upsert_referral_code(
	request: crate::grpc::ReferralCode,
	request_relation: crate::grpc::ReferralRelation,
) -> crate::grpc::ReferralCode {
	let _relation = ReferralRelation {
		id: String::new(),
		referral_code: String::new(),
		invitee: String::new(),
		created_at: now(),
	};
	let record = ReferralCode {
		id: String::new(),
		referral_code: String::new(),
		referrer: String::new(),
		created_at: now(),
	};
	let _type_conflict_code: crate::grpc::ReferralCode = request;
	let _type_conflict_relation: crate::grpc::ReferralRelation = request_relation;
	crate::grpc::ReferralCode {
		id: record.id,
		referral_code: record.referral_code,
		referrer: record.referrer,
		created_at: None,
	}
}
use crate::types::ReferralCode; use crate::types::ReferralRelation;"#;
	let ctx = shared::read_file_context_from_text(
		Path::new("import009_gateway_separate_imports_eof_same_line.rs"),
		original.to_owned(),
	)
	.expect("context")
	.expect("has ctx");
	let (violations, edits) = rules::collect_violations(&ctx, true);

	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-009" && v.fixable && v.message.contains("`ReferralCode`")
	}));
	assert!(violations.iter().any(|v| {
		v.rule == "RUST-STYLE-IMPORT-009" && v.fixable && v.message.contains("`ReferralRelation`")
	}));

	let import009_edits =
		edits.into_iter().filter(|edit| edit.rule == "RUST-STYLE-IMPORT-009").collect::<Vec<_>>();

	assert!(!import009_edits.is_empty());

	let mut rewritten = original.to_owned();
	let applied = fixes::apply_edits(&mut rewritten, import009_edits).expect("apply edits");

	assert!(applied > 0);
	assert!(!rewritten.contains("use crate::types::ReferralCode;"));
	assert!(!rewritten.contains("use crate::types::ReferralRelation;"));
	assert!(rewritten.contains("let _relation = crate::types::ReferralRelation {"));
	assert!(rewritten.contains("let record = crate::types::ReferralCode {"));
}

#[test]
fn root_crate_qualification_does_not_import_the_crate_again() {
	let original = "use fixture::First;\nuse fixture::Second;\npub fn first() -> First { fixture::First {} }\npub fn second() -> Second { fixture::Second {} }\n";
	let root = env::temp_dir().join(format!("vstyle-root-import-{}", process::id()));
	let path = root.join("consumer.rs");
	let provider = root.join("provider.rs");
	let library = root.join("libfixture.rlib");
	let ctx = shared::read_file_context_from_text(&path, original.to_owned())
		.expect("Parse input.")
		.expect("Input context.");
	let (_, edits) = rules::collect_violations(&ctx, true);
	let edits = edits.into_iter().filter(|edit| edit.rule == "RUST-STYLE-IMPORT-009").collect();
	let mut rewritten = original.to_owned();

	fixes::apply_edits(&mut rewritten, edits).expect("Qualify imports.");

	assert!(!rewritten.contains("use fixture"), "{rewritten}");
	assert!(rewritten.contains("-> fixture::First"), "{rewritten}");
	assert!(rewritten.contains("-> fixture::Second"), "{rewritten}");

	fs::create_dir_all(&root).expect("Create fixture directory.");
	fs::write(&provider, "pub struct First {}\npub struct Second {}\n").expect("Write provider.");
	fs::write(&path, &rewritten).expect("Write consumer.");

	let compiled = Command::new("rustc")
		.args(["--edition=2024", "--crate-name=fixture", "--crate-type=rlib"])
		.arg(&provider)
		.arg("-o")
		.arg(&library)
		.output()
		.expect("Compile provider.");

	assert!(compiled.status.success(), "{}", String::from_utf8_lossy(&compiled.stderr));

	let compiled = Command::new("rustc")
		.args(["--edition=2024", "--crate-type=lib", "--emit=metadata", "--extern"])
		.arg(format!("fixture={}", library.display()))
		.arg(&path)
		.arg("-o")
		.arg(root.join("consumer.rmeta"))
		.output()
		.expect("Compile consumer.");

	fs::remove_dir_all(root).expect("Remove fixture.");

	assert!(
		compiled.status.success(),
		"{rewritten}\n{}",
		String::from_utf8_lossy(&compiled.stderr)
	);
}

#[test]
fn import004_handles_unicode_module_prefixes_in_braced_imports() {
	let source = "use 包::{make};\n\nfn sample() {\n make();\n}\n";
	let ctx =
		shared::read_file_context_from_text(Path::new("unicode_import.rs"), source.to_owned())
			.expect("Read fixture.")
			.expect("Get fixture context.");
	let (_, edits) = rules::collect_violations(&ctx, true);
	let edits = edits.into_iter().filter(|edit| edit.rule == "RUST-STYLE-IMPORT-004").collect();
	let mut rewritten = source.to_owned();

	assert!(fixes::apply_edits(&mut rewritten, edits).expect("Apply import edits.") > 0);
	assert!(rewritten.contains("包::make();"));
	assert!(!rewritten.contains("use 包::{make};"));
}
