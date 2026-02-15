use std::{
	fs,
	path::PathBuf,
	process::Command,
	time::{SystemTime, UNIX_EPOCH},
};

fn create_temp_crate_root() -> PathBuf {
	let stamp = SystemTime::now().duration_since(UNIX_EPOCH).expect("Clock.").as_nanos();
	let root = std::env::temp_dir().join(format!("vstyle-type-alias-{}", stamp));
	let _ = fs::remove_dir_all(&root);

	fs::create_dir_all(root.join("src")).expect("Create src.");
	fs::write(
		root.join("Cargo.toml"),
		r#"
[package]
name = "vstyle-type-alias-fixture"
version = "0.1.0"
edition = "2021"
"#,
	)
	.expect("Write cargo manifest.");
	fs::write(root.join(".gitignore"), "/target\n").expect("Write gitignore.");

	root
}

#[test]
fn type_alias_rename_is_fixed_by_tune_across_module_files() {
	let temp_dir = create_temp_crate_root();
	let main_source = r#"
mod user;

pub type MyError = std::io::Error;

pub fn demo() -> Result<(), MyError> {
	Err(std::io::Error::new(std::io::ErrorKind::Other, "boom"))
}

fn main() {}
"#;
	let user_source = r#"
use crate::MyError;

pub fn build_error() -> MyError {
	MyError::new(std::io::ErrorKind::Other, "boom")
}
"#;

	fs::write(temp_dir.join("src/main.rs"), main_source).expect("write main source");
	fs::write(temp_dir.join("src/user.rs"), user_source).expect("write user source");

	let output =
		Command::new("git").current_dir(&temp_dir).args(["init"]).output().expect("git init");

	assert!(output.status.success());

	let status = Command::new("git")
		.current_dir(&temp_dir)
		.args(["add", "Cargo.toml", "src/main.rs", "src/user.rs"])
		.output()
		.expect("git add");

	assert!(status.status.success());

	let output = Command::new(env!("CARGO_BIN_EXE_vstyle"))
		.current_dir(&temp_dir)
		.arg("tune")
		.output()
		.expect("run vstyle");

	assert!(
		output.status.success(),
		"expected vstyle tune to succeed, stderr: {}",
		String::from_utf8_lossy(&output.stderr)
	);

	let main_after = fs::read_to_string(temp_dir.join("src/main.rs")).expect("read main file");
	let user_after = fs::read_to_string(temp_dir.join("src/user.rs")).expect("read user file");

	assert!(!main_after.contains("type MyError"));
	assert!(main_after.contains("pub use std::io::Error;"));
	assert!(!main_after.contains("MyError"));

	assert!(!user_after.contains("MyError"));
	assert!(user_after.contains("crate::Error") || user_after.contains("use crate::Error;"));
}
