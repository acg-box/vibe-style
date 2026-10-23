#![allow(missing_docs, unused_crate_dependencies)]

#[path = "support/fixture.rs"] mod fixture;

use std::{fs, process::Command};

use fixture::Fixture;

fn create_temp_crate_root() -> Fixture {
	let root = Fixture::new("type-alias-rename");

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
		.args(["tune", "--language", "rust"])
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

fn assert_tune_preserves_program(source: &str, expected: &str) -> String {
	let root = create_temp_crate_root();
	let path = root.join("src/main.rs");

	fs::write(&path, source).expect("Write program.");

	let initialized = Command::new("git")
		.current_dir(&root)
		.args(["init", "--quiet"])
		.output()
		.expect("Initialize fixture.");

	assert!(initialized.status.success());

	let before = Command::new("cargo")
		.current_dir(&root)
		.args(["run", "--quiet"])
		.output()
		.expect("Run original program.");

	assert!(before.status.success(), "{}", String::from_utf8_lossy(&before.stderr));
	assert_eq!(String::from_utf8_lossy(&before.stdout), expected);

	let tuned = Command::new(env!("CARGO_BIN_EXE_vstyle"))
		.current_dir(&root)
		.args(["tune", "--language", "rust"])
		.output()
		.expect("Tune program.");

	assert!(tuned.status.success(), "{}", String::from_utf8_lossy(&tuned.stderr));

	let rewritten = fs::read_to_string(&path).expect("Read tuned program.");
	let after = Command::new("cargo")
		.current_dir(&root)
		.args(["run", "--quiet"])
		.output()
		.expect("Run tuned program.");

	assert!(after.status.success(), "{}\n{rewritten}", String::from_utf8_lossy(&after.stderr));
	assert_eq!(after.stdout, before.stdout, "Program behavior changed:\n{rewritten}");

	rewritten
}

#[test]
fn alias_renames_preserve_enum_variants_in_the_value_namespace() {
	let source = r#"
pub type Value = std::string::String;
#[derive(Debug)]
enum Token { Value, String }
fn main() {
	let selected = Token::Value;
	let _: Option<Value> = None;
	println!("{selected:?}");
}
"#;
	let rewritten = assert_tune_preserves_program(source, "Value\n");

	assert!(!rewritten.contains("type Value"));
	assert!(rewritten.contains("Token::Value"));
}

#[test]
fn alias_renames_preserve_distinct_definitions_in_separate_modules() {
	let source = r#"
mod one {
	pub type Alias = u8;
	pub fn value() -> Alias { 0 }
}
mod two {
	pub type Alias = u16;
	pub fn value() -> Alias { 0 }
}
fn main() {
	let selected = two::value();
	println!("{}", std::mem::size_of_val(&selected));
}
"#;

	assert_tune_preserves_program(source, "2\n");
}

#[test]
fn private_alias_renames_preserve_the_qualified_target() {
	let source = r#"
mod remote {
	pub struct Target;
	impl Target {
		pub fn new() -> Self { Self }
		pub fn value(&self) -> u8 { 1 }
	}
}
struct Target;
impl Target {
	fn new() -> Self { Self }
	fn value(&self) -> u8 { 2 }
}
type Alias = remote::Target;
fn main() {
	let selected = Alias::new();
	println!("{}", selected.value());
}
"#;

	assert_tune_preserves_program(source, "1\n");
}

#[test]
fn alias_renames_respect_generic_type_and_value_shadows() {
	let source = r#"
#![allow(non_camel_case_types, non_upper_case_globals)]
type Alias = u8;
const Alias: u16 = 7;
fn size<Alias>() -> usize { std::mem::size_of::<Alias>() }
fn primitive_size<u8>() -> usize { std::mem::size_of::<Alias>() }
fn main() {
	println!("{} {} {}", size::<u32>(), primitive_size::<u64>(), Alias);
}
"#;

	assert_tune_preserves_program(source, "4 1 7\n");
}

#[test]
fn import_alias_rewrites_preserve_generic_type_bindings() {
	let source = r#"
use std::string::String as Value;
fn size<Value>() -> usize { std::mem::size_of::<Value>() }
fn main() {
	let _: Option<Value> = None;
	println!("{}", size::<u32>());
}
"#;

	assert_tune_preserves_program(source, "4\n");
}
