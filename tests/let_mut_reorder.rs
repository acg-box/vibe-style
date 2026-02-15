use std::{
	fs,
	path::PathBuf,
	process::Command,
	time::{SystemTime, UNIX_EPOCH},
};

fn create_temp_crate_root() -> PathBuf {
	let stamp = SystemTime::now().duration_since(UNIX_EPOCH).expect("Clock.").as_nanos();
	let root = std::env::temp_dir().join(format!("vstyle-let-mut-{}", stamp));
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
		.arg("tune")
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
		stderr.contains("Skipped RUST-STYLE-LET-001 reorder in"),
		"expected skip diagnostic for dependency-sensitive reorder"
	);
	assert!(stderr.contains("src/unsafe.rs"));
}
