#![allow(missing_docs, unused_crate_dependencies)]

use std::{
	env, fs,
	path::PathBuf,
	process::Command,
	time::{SystemTime, UNIX_EPOCH},
};

fn create_temp_crate_root() -> PathBuf {
	let stamp = SystemTime::now().duration_since(UNIX_EPOCH).expect("Clock.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-let-mut-{}", stamp));
	let _ = fs::remove_dir_all(&root);

	fs::create_dir_all(root.join("src")).expect("Create src.");
	fs::write(
		root.join("Cargo.toml"),
		r#"
[package]
name = "vstyle-let-mut-reorder-fixture"
version = "0.1.0"
edition = "2021"
"#,
	)
	.expect("Write cargo manifest.");
	fs::write(root.join(".gitignore"), "/target\n").expect("Write gitignore.");
	fs::write(
		root.join("src/main.rs"),
		r#"mod safe;
mod r#unsafe;

fn main() {}
"#,
	)
	.expect("Write main.");

	root
}

fn create_atomicity_crate_root() -> PathBuf {
	let stamp = SystemTime::now().duration_since(UNIX_EPOCH).expect("Clock.").as_nanos();
	let root = env::temp_dir().join(format!("vstyle-tune-atomicity-{stamp}"));
	let _ = fs::remove_dir_all(&root);

	fs::create_dir_all(root.join("src")).expect("Create source directory.");
	fs::write(
		root.join("Cargo.toml"),
		r#"
[package]
name = "vstyle-tune-atomicity-fixture"
version = "0.1.0"
edition = "2024"
build = "build.rs"
"#,
	)
	.expect("Write Cargo manifest.");
	fs::write(root.join(".gitignore"), "/target\n").expect("Write gitignore.");

	root
}

#[test]
fn let_mut_reorder_is_semantically_validated_by_compiler() {
	let temp_dir = create_temp_crate_root();
	let safe_source = r#"
pub fn safe_case() -> usize {
	let mut mutable_value = 1usize;
	let immutable_value = 2usize;
	mutable_value + immutable_value
}
"#;
	let unsafe_source = r#"
pub fn closure_carries_binding() {
	let mut value = String::from("value");
	let _trace = format!("{}\n", value);
	let deferred = || value;
	let _ = deferred();
}
"#;

	fs::write(temp_dir.join("src/safe.rs"), safe_source).expect("write safe source");
	fs::write(temp_dir.join("src/unsafe.rs"), unsafe_source).expect("write unsafe source");

	let output =
		Command::new("git").current_dir(&temp_dir).args(["init"]).output().expect("git init");

	assert!(output.status.success());

	let status = Command::new("git")
		.current_dir(&temp_dir)
		.args(["add", "Cargo.toml", "src/main.rs", "src/safe.rs", "src/unsafe.rs"])
		.output()
		.expect("git add");

	assert!(status.status.success());

	let output = Command::new(env!("CARGO_BIN_EXE_vstyle"))
		.current_dir(&temp_dir)
		.args(["tune", "--language", "rust"])
		.output()
		.expect("run vstyle");

	assert!(output.status.success());

	let stderr = String::from_utf8_lossy(&output.stderr);
	let safe_path = temp_dir.join("src/safe.rs");
	let unsafe_path = temp_dir.join("src/unsafe.rs");
	let safe_after = fs::read_to_string(&safe_path).expect("read safe file");
	let unsafe_after = fs::read_to_string(&unsafe_path).expect("read unsafe file");
	let safe_mut_pos =
		safe_after.find("let mut mutable_value").expect("safe file retains mutable let");
	let safe_immutable_pos =
		safe_after.find("let immutable_value").expect("safe file retains immutable let");

	assert!(safe_immutable_pos < safe_mut_pos);
	assert_eq!(unsafe_after, unsafe_source);
	assert!(
		stderr.contains("vstyle tune: starting initial scan."),
		"expected initial scan telemetry on captured stderr:\n{stderr}"
	);
	assert!(
		stderr.contains("vstyle tune: initial scan checked"),
		"expected initial scan summary telemetry on captured stderr:\n{stderr}"
	);
	assert!(
		stderr.contains("vstyle tune: round 1/"),
		"expected fix round telemetry on captured stderr:\n{stderr}"
	);
	assert!(
		stderr.contains("vstyle tune: starting final scan."),
		"expected final scan start telemetry on captured stderr:\n{stderr}"
	);
	assert!(
		stderr.contains("vstyle tune: final scan checked"),
		"expected final scan summary telemetry on captured stderr:\n{stderr}"
	);
	assert!(
		!stderr.contains("Skipped RUST-STYLE-LET-001 reorder in"),
		"did not expect a semantic validation skip diagnostic for the unfixable fixture"
	);
}

#[test]
fn let_mut_reorder_reuses_semantic_output_across_cold_and_warm_runs() {
	let temp_dir = create_temp_crate_root();
	let safe_source = r#"
pub fn safe_case() -> usize {
	let mut mutable_value = 1usize;
	let immutable_value = 2usize;
	mutable_value + immutable_value
}
"#;
	let unsafe_source = r#"
pub fn closure_carries_binding() {
	let mut value = String::from("value");
	let _trace = format!("{}\n", value);
	let deferred = || value;
	let _ = deferred();
}
"#;

	fs::write(temp_dir.join("src/safe.rs"), safe_source).expect("write safe source");
	fs::write(temp_dir.join("src/unsafe.rs"), unsafe_source).expect("write unsafe source");

	let output =
		Command::new("git").current_dir(&temp_dir).args(["init"]).output().expect("git init");

	assert!(output.status.success());

	let status = Command::new("git")
		.current_dir(&temp_dir)
		.args(["add", "Cargo.toml", "src/main.rs", "src/safe.rs", "src/unsafe.rs"])
		.output()
		.expect("git add");

	assert!(status.status.success());

	let status = Command::new("cargo")
		.current_dir(&temp_dir)
		.arg("generate-lockfile")
		.output()
		.expect("cargo generate-lockfile");

	assert!(status.status.success());

	let cold = Command::new(env!("CARGO_BIN_EXE_vstyle"))
		.current_dir(&temp_dir)
		.args(["tune", "--language", "rust", "--verbose"])
		.output()
		.expect("run cold vstyle");

	assert!(cold.status.success());

	let cold_output = format!(
		"{}{}",
		String::from_utf8_lossy(&cold.stdout),
		String::from_utf8_lossy(&cold.stderr)
	);

	assert!(
		cold_output.contains("Semantic cache: 0 hit(s), 2 miss(es)."),
		"expected cold pre-edit and post-edit semantic misses, output: {cold_output}"
	);

	fs::write(temp_dir.join("src/safe.rs"), safe_source).expect("restore safe source");
	fs::write(temp_dir.join("src/unsafe.rs"), unsafe_source).expect("restore unsafe source");

	let warm = Command::new(env!("CARGO_BIN_EXE_vstyle"))
		.current_dir(&temp_dir)
		.args(["tune", "--language", "rust", "--verbose"])
		.output()
		.expect("run warm vstyle");

	assert!(warm.status.success());

	let warm_output = format!(
		"{}{}",
		String::from_utf8_lossy(&warm.stdout),
		String::from_utf8_lossy(&warm.stderr)
	);

	assert!(
		warm_output.contains("Semantic cache: 2 hit(s), 0 miss(es)."),
		"expected warm pre-edit and post-edit semantic hits, output: {warm_output}"
	);
}

#[test]
fn tune_keeps_io_result_qualified_during_semantic_validation() {
	let temp_dir = create_temp_crate_root();
	let main_source = r#"fn business_result() -> Result<(), i32> {
	Ok(())
}

fn read_state() -> std::io::Result<String> {
	Ok(String::new())
}

fn apply_test_override() -> Result<(), i32> {
	Ok(())
}

fn main() {
	let value = 10f32;

	println!("{value}");
}
"#;

	fs::write(temp_dir.join("src/main.rs"), main_source).expect("Write main source.");

	let output = Command::new("git")
		.current_dir(&temp_dir)
		.args(["init"])
		.output()
		.expect("Initialize Git repository.");

	assert!(output.status.success());

	let output = Command::new(env!("CARGO_BIN_EXE_vstyle"))
		.current_dir(&temp_dir)
		.args(["tune", "--language", "rust", "--verbose"])
		.output()
		.expect("Run vstyle tune.");
	let main_after = fs::read_to_string(temp_dir.join("src/main.rs")).expect("Read main source.");
	let stderr = String::from_utf8_lossy(&output.stderr);

	if !output.status.success() {
		assert_eq!(main_after, main_source, "Failed tune did not roll back its edits.");
	}

	assert!(output.status.success(), "tune unexpectedly failed:\n{stderr}");
	assert!(!main_after.contains("use std::io::Result;"));
	assert!(main_after.contains("fn read_state() -> std::io::Result<String>"));
	assert!(main_after.contains("fn apply_test_override() -> Result<(), i32>"));
	assert!(main_after.contains("let value = 10_f32;"));
	assert!(
		stderr.contains("validating the edited files"),
		"missing semantic validation telemetry:\n{stderr}"
	);

	let output = Command::new("cargo")
		.current_dir(&temp_dir)
		.arg("check")
		.output()
		.expect("Check tuned fixture.");

	assert!(
		output.status.success(),
		"tuned fixture did not compile:\n{}",
		String::from_utf8_lossy(&output.stderr)
	);

	let output = Command::new(env!("CARGO_BIN_EXE_vstyle"))
		.current_dir(&temp_dir)
		.args(["curate", "--language", "rust", "--strict"])
		.output()
		.expect("Run strict vstyle check.");

	assert!(
		output.status.success(),
		"strict check failed:\n{}{}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	);

	let output = Command::new(env!("CARGO_BIN_EXE_vstyle"))
		.current_dir(&temp_dir)
		.args(["tune", "--language", "rust", "--strict"])
		.output()
		.expect("Repeat vstyle tune.");
	let repeated = fs::read_to_string(temp_dir.join("src/main.rs")).expect("Read tuned source.");

	assert!(
		output.status.success(),
		"repeated tune failed:\n{}{}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	);
	assert_eq!(repeated, main_after);

	let _ = fs::remove_dir_all(temp_dir);
}

#[test]
fn tune_rolls_back_the_run_when_semantic_validation_fails() {
	let temp_dir = create_atomicity_crate_root();
	let main_source = r#"fn business_result() -> Result<(), i32> {
	Ok(())
}

fn read_state() -> std::io::Result<String> {
	Ok(String::new())
}

fn apply_test_override() -> Result<(), i32> {
	Ok(())
}

fn main() {
	let value = 10f32;

	println!("{value}");
}
"#;
	let build_source = r#"use std::fs;

fn main() {
	let _build_value = 20f32;

	println!("cargo:rerun-if-changed=src/main.rs");

	let source = fs::read_to_string("src/main.rs").unwrap_or_default();

	if source.contains("10_f32") {
		panic!("reject the edited fixture");
	}
}
"#;

	fs::write(temp_dir.join("src/main.rs"), main_source).expect("Write main source.");
	fs::write(temp_dir.join("build.rs"), build_source).expect("Write build source.");

	let output = Command::new("git")
		.current_dir(&temp_dir)
		.args(["init"])
		.output()
		.expect("Initialize Git repository.");

	assert!(output.status.success());

	let output = Command::new(env!("CARGO_BIN_EXE_vstyle"))
		.current_dir(&temp_dir)
		.args(["tune", "--language", "rust"])
		.output()
		.expect("Run vstyle tune.");
	let main_after = fs::read_to_string(temp_dir.join("src/main.rs")).expect("Read main source.");
	let build_after = fs::read_to_string(temp_dir.join("build.rs")).expect("Read build source.");
	let stderr = String::from_utf8_lossy(&output.stderr);

	assert!(!output.status.success(), "tune unexpectedly succeeded:\n{stderr}");
	assert_eq!(main_after, main_source);
	assert_eq!(build_after, build_source);
	assert!(stderr.contains("semantic validation"), "missing semantic error:\n{stderr}");

	let _ = fs::remove_dir_all(temp_dir);
}
