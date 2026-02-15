use std::{
	fs,
	path::PathBuf,
	process::Command,
	time::{SystemTime, UNIX_EPOCH},
};

fn create_temp_crate_root() -> PathBuf {
	let stamp = SystemTime::now().duration_since(UNIX_EPOCH).expect("Clock.").as_nanos();
	let root = std::env::temp_dir().join(format!("vstyle-mod007-{}", stamp));
	let _ = fs::remove_dir_all(&root);

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
		.arg("tune")
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

	let has_helper_use = used_after.lines().any(|line| {
		let trimmed = line.trim_start();

		trimmed.starts_with("use ") && trimmed.contains("helper")
	});

	assert!(has_helper_use, "expected helper to be explicitly imported after tune");
}
