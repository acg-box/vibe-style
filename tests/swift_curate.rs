#![allow(missing_docs, unused_crate_dependencies)]

use std::{
	env, fs,
	path::{Path, PathBuf},
	process::{self, Command},
	time::{SystemTime, UNIX_EPOCH},
};

fn create_temp_workspace_root() -> PathBuf {
	let stamp = SystemTime::now().duration_since(UNIX_EPOCH).expect("Clock.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-swift-curate-{}-{stamp}", process::id()));
	let _ = fs::remove_dir_all(&root);

	fs::create_dir_all(root.join("crates/app/src")).expect("Create Rust package src.");
	fs::create_dir_all(root.join("Sources/App")).expect("Create Swift sources.");
	fs::create_dir_all(root.join("Tests")).expect("Create Swift tests.");
	fs::write(
		root.join("Cargo.toml"),
		r#"
[workspace]
members = ["crates/app"]
resolver = "2"
"#,
	)
	.expect("Write workspace manifest.");
	fs::write(
		root.join("crates/app/Cargo.toml"),
		r#"
[package]
name = "vstyle-swift-curate-fixture"
version = "0.1.0"
edition = "2021"
"#,
	)
	.expect("Write package manifest.");
	fs::write(root.join("crates/app/src/lib.rs"), "").expect("Write Rust lib.");
	fs::write(
		root.join(".gitignore"),
		"/target\n/Sources/App/Ignored.swift\n/Sources/App/IgnoredTracked.swift\n",
	)
	.expect("Write gitignore.");

	root
}

fn write_swift_fixture(temp_dir: &Path) {
	let long_body = (0..121).map(|idx| format!("\tlet value{idx} = {idx}\n")).collect::<String>();
	let swift_source = format!(
		"import Foundation\n\
import struct Foundation.UUID\n\
typealias UserId = UUID\n\
\n\
func load() {{\n\
\tlet id = maybeId!\n\
\tlet forced = try! make()\n\
\tlet count = 10000\n\
}}\n\
\n\
func tooLong() {{\n\
{long_body}}}\n"
	);
	let swift_test_source = "\
func testForceAllowed() {
\tlet value = try! make()
\t_ = value
}
";

	fs::write(temp_dir.join("Sources/App/mod.swift"), swift_source).expect("write Swift source");
	fs::write(temp_dir.join("Sources/App/Untracked.swift"), "let value = missing!\n")
		.expect("write untracked Swift source");
	fs::write(temp_dir.join("Sources/App/Ignored.swift"), "let value = ignored!\n")
		.expect("write ignored Swift source");
	fs::write(temp_dir.join("Sources/App/IgnoredTracked.swift"), "let value = trackedIgnored!\n")
		.expect("write tracked ignored Swift source");
	fs::write(temp_dir.join("Tests/AppTests.swift"), swift_test_source)
		.expect("write Swift test source");
}

fn initialize_git_fixture(temp_dir: &Path) {
	let status =
		Command::new("git").current_dir(temp_dir).args(["init"]).output().expect("git init");

	assert!(
		status.status.success(),
		"expected git init to succeed, stderr: {}",
		String::from_utf8_lossy(&status.stderr)
	);

	let status = Command::new("git")
		.current_dir(temp_dir)
		.args([
			"add",
			"-f",
			"Cargo.toml",
			"crates/app/Cargo.toml",
			"crates/app/src/lib.rs",
			"Sources/App/IgnoredTracked.swift",
			"Sources/App/mod.swift",
			"Tests/AppTests.swift",
		])
		.output()
		.expect("git add");

	assert!(
		status.status.success(),
		"expected git add to succeed, stderr: {}",
		String::from_utf8_lossy(&status.stderr)
	);
}

fn assert_curate_requires_explicit_language(temp_dir: &Path) {
	let output = Command::new(env!("CARGO_BIN_EXE_vstyle"))
		.current_dir(temp_dir)
		.args(["curate", "--workspace"])
		.output()
		.expect("run vstyle");

	assert!(!output.status.success(), "curate without an explicit language should fail");

	let stderr = String::from_utf8_lossy(&output.stderr);

	assert!(stderr.contains("--language"), "expected missing language diagnostic:\n{stderr}");
}

fn assert_rust_curate_ignores_swift(temp_dir: &Path) {
	let output = Command::new(env!("CARGO_BIN_EXE_vstyle"))
		.current_dir(temp_dir)
		.args(["curate", "--language", "rust", "--workspace"])
		.output()
		.expect("run vstyle");

	assert!(
		output.status.success(),
		"explicit Rust curate should ignore Swift violations, stderr: {}",
		String::from_utf8_lossy(&output.stderr)
	);

	let stdout = String::from_utf8_lossy(&output.stdout);

	assert!(
		!stdout.contains("Sources/App/mod.swift"),
		"explicit Rust curate should not scan Swift files:\n{stdout}"
	);
}

fn assert_swift_curate_reports_expected_violations(temp_dir: &Path) {
	let output = Command::new(env!("CARGO_BIN_EXE_vstyle"))
		.current_dir(temp_dir)
		.args(["curate", "--language", "swift", "--workspace"])
		.output()
		.expect("run vstyle");

	assert!(!output.status.success(), "curate should fail when Swift violations are reported");

	let stdout = String::from_utf8_lossy(&output.stdout);

	for rule in [
		"SWIFT-STYLE-FILE-001",
		"SWIFT-STYLE-IMPORT-004",
		"SWIFT-STYLE-TYPE-001",
		"SWIFT-STYLE-RUNTIME-001",
		"SWIFT-STYLE-NUM-002",
		"SWIFT-STYLE-READ-002",
	] {
		assert!(stdout.contains(rule), "expected {rule} in vstyle output:\n{stdout}");
	}

	assert!(
		!stdout.contains("Tests/AppTests.swift:2:1: [SWIFT-STYLE-RUNTIME-001]"),
		"Swift test files should not report force operators:\n{stdout}"
	);
	assert!(
		stdout.contains("Untracked.swift"),
		"non-ignored Swift files should be scanned regardless of tracking state"
	);
	assert!(!stdout.contains("Ignored.swift"), "git-ignored Swift files should not be scanned");
	assert!(
		!stdout.contains("IgnoredTracked.swift"),
		"git-ignored Swift files should not be scanned even when tracked"
	);
}

#[test]
fn curate_reports_swift_workspace_violations_from_non_ignored_files() {
	let temp_dir = create_temp_workspace_root();

	write_swift_fixture(&temp_dir);
	initialize_git_fixture(&temp_dir);
	assert_curate_requires_explicit_language(&temp_dir);
	assert_rust_curate_ignores_swift(&temp_dir);
	assert_swift_curate_reports_expected_violations(&temp_dir);
}
