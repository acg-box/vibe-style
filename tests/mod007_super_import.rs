#![allow(missing_docs, unused_crate_dependencies)]

#[path = "support/fixture.rs"] mod fixture;

use std::{fs, process::Command};

use fixture::Fixture;

fn create_temp_crate_root() -> Fixture {
	let root = Fixture::new("mod007-super-import");

	fs::create_dir_all(root.join("src")).expect("Create src.");
	fs::write(
		root.join("Cargo.toml"),
		r#"
[package]
name = "vstyle-mod007-fixture"
version = "0.1.0"
edition = "2021"
"#,
	)
	.expect("Write cargo manifest.");
	fs::write(root.join(".gitignore"), "/target\n").expect("Write gitignore.");
	fs::write(
		root.join("src/lib.rs"),
		r#"
mod used;
mod unused;
"#,
	)
	.expect("Write lib.");

	root
}

#[test]
fn removes_unused_test_module_super_glob() {
	let temp_dir = create_temp_crate_root();
	let used_source = r#"
pub fn helper() -> usize {
	1
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn calls_helper() {
		assert_eq!(helper(), 1);
	}
}
"#;
	let unused_source = r#"
fn helper_value() -> usize {
	2
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn checks_value() {
		assert_eq!(1 + 1, 2);
	}
}
"#;

	fs::write(temp_dir.join("src/used.rs"), used_source).expect("write used source");
	fs::write(temp_dir.join("src/unused.rs"), unused_source).expect("write unused source");

	let status =
		Command::new("git").current_dir(&temp_dir).args(["init"]).output().expect("git init");

	assert!(status.status.success());

	let status = Command::new("git")
		.current_dir(&temp_dir)
		.args(["add", "Cargo.toml", "src/lib.rs", "src/used.rs", "src/unused.rs"])
		.output()
		.expect("git add");

	assert!(status.status.success());

	let output = Command::new(env!("CARGO_BIN_EXE_vstyle"))
		.current_dir(&temp_dir)
		.args(["tune", "--language", "rust"])
		.output()
		.expect("run vstyle");

	assert!(output.status.success());

	let used_after = fs::read_to_string(temp_dir.join("src/used.rs")).expect("read used source");
	let unused_after =
		fs::read_to_string(temp_dir.join("src/unused.rs")).expect("read unused source");

	assert!(
		!unused_after.contains("use super::*;"),
		"unused super glob should be removed in src/unused.rs"
	);
	assert!(
		!used_after.contains("use super::*;"),
		"used super glob should be expanded (IMPORT-007) rather than left as a glob in src/used.rs"
	);

	let has_used_module_import = used_after.lines().any(|line| {
		let trimmed = line.trim_start();

		trimmed == "use crate::used;"
	});
	let has_qualified_helper_call = used_after.contains("used::helper()");

	assert!(
		has_used_module_import && has_qualified_helper_call,
		"expected helper to be reached through an explicit module import after tune"
	);
}

#[test]
fn removing_unused_globs_preserves_inline_and_multiline_test_items() {
	for source in [
		"#[cfg(test)] mod tests { use super::*; #[test] fn keeps_test() { assert_eq!(2 + 2, 4); } }\n",
		"#[cfg(test)] mod tests {\n use super::*; #[test] fn keeps_test() { assert_eq!(2 + 2, 4); }\n}\n",
		"#[cfg(test)] mod tests {\n use\n super::*;\n #[test] fn keeps_test() { assert_eq!(2 + 2, 4); }\n}\n",
	] {
		let root = create_temp_crate_root();

		fs::write(root.join("src/lib.rs"), source).expect("Write test module.");

		let initialized = Command::new("git")
			.current_dir(&root)
			.args(["init", "--quiet"])
			.output()
			.expect("Initialize fixture.");

		assert!(initialized.status.success());

		let tuned = Command::new(env!("CARGO_BIN_EXE_vstyle"))
			.current_dir(&root)
			.args(["tune", "--language", "rust"])
			.output()
			.expect("Tune test module.");
		let rewritten = fs::read_to_string(root.join("src/lib.rs")).expect("Read test module.");

		assert!(tuned.status.success(), "{}\n{rewritten}", String::from_utf8_lossy(&tuned.stderr));
		assert!(rewritten.contains("fn keeps_test"), "Test code was deleted:\n{rewritten}");
		assert!(!rewritten.contains("super::*"), "Unused import remains:\n{rewritten}");

		let listed = Command::new("cargo")
			.current_dir(&root)
			.args(["test", "--quiet", "--", "--list"])
			.output()
			.expect("List retained tests.");

		assert!(listed.status.success(), "{}", String::from_utf8_lossy(&listed.stderr));
		assert!(String::from_utf8_lossy(&listed.stdout).contains("tests::keeps_test: test"));
	}
}
