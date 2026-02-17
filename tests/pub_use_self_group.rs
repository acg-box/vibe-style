use std::{
	fs,
	path::PathBuf,
	process::Command,
	time::{SystemTime, UNIX_EPOCH},
};

fn create_temp_crate_root() -> PathBuf {
	let stamp = SystemTime::now().duration_since(UNIX_EPOCH).expect("Clock.").as_nanos();
	let root = std::env::temp_dir().join(format!("vstyle-pub-use-self-group-{}", stamp));
	let _ = fs::remove_dir_all(&root);

	fs::create_dir_all(root.join("src")).expect("Create src.");
	fs::write(
		root.join("Cargo.toml"),
		r#"
[package]
name = "vstyle-pub-use-self-group-fixture"
version = "0.1.0"
edition = "2021"
"#,
	)
	.expect("Write cargo manifest.");
	fs::write(root.join(".gitignore"), "/target\n").expect("Write gitignore.");

	root
}

#[test]
fn local_module_pub_use_items_are_converged_to_self_group_by_tune() {
	let temp_dir = create_temp_crate_root();
	let lib_source = r#"
mod add_event;
mod add_note;

pub use add_event::{AddEventRequest, AddEventResponse};

pub use add_note::{AddNoteRequest, AddNoteResponse};
"#;
	let add_event_source = r#"
pub struct AddEventRequest;
pub struct AddEventResponse;
"#;
	let add_note_source = r#"
pub struct AddNoteRequest;
pub struct AddNoteResponse;
"#;

	fs::write(temp_dir.join("src/lib.rs"), lib_source).expect("write lib source");
	fs::write(temp_dir.join("src/add_event.rs"), add_event_source).expect("write add_event source");
	fs::write(temp_dir.join("src/add_note.rs"), add_note_source).expect("write add_note source");

	let output =
		Command::new("git").current_dir(&temp_dir).args(["init"]).output().expect("git init");

	assert!(output.status.success());

	let status = Command::new("git")
		.current_dir(&temp_dir)
		.args(["add", "Cargo.toml", "src/lib.rs", "src/add_event.rs", "src/add_note.rs"])
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

	let lib_after = fs::read_to_string(temp_dir.join("src/lib.rs")).expect("read lib file");
	let compact = lib_after.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();

	assert!(
		compact.contains(
			"pubuseself::{add_event::{AddEventRequest,AddEventResponse},add_note::{AddNoteRequest,AddNoteResponse}};"
		),
		"expected grouped self re-export, got:\n{lib_after}"
	);
}
