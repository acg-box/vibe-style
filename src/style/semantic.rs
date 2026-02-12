use std::{
	collections::{BTreeMap, BTreeSet},
	fs,
	path::{Path, PathBuf},
	process::Command,
};

use serde_json::Value;

use super::shared::CargoOptions;
use crate::prelude::*;

pub(crate) fn apply_semantic_fixes(
	files: &[PathBuf],
	cargo_options: &CargoOptions,
) -> Result<usize> {
	if files.is_empty() {
		return Ok(0);
	}

	let tracked = files.iter().map(|path| normalize_path(path)).collect::<BTreeSet<_>>();
	let mut cmd = Command::new("cargo");

	cmd.arg("check");
	cmd.arg("--all-targets");
	cmd.arg("--message-format=json");

	if cargo_options.workspace {
		cmd.arg("--workspace");
	}

	for package in &cargo_options.packages {
		cmd.arg("-p");
		cmd.arg(package);
	}

	if !cargo_options.features.is_empty() {
		cmd.arg("--features");
		cmd.arg(cargo_options.features.join(","));
	}
	if cargo_options.all_features {
		cmd.arg("--all-features");
	}
	if cargo_options.no_default_features {
		cmd.arg("--no-default-features");
	}

	let output =
		cmd.output().map_err(|err| eyre::eyre!("Failed to run semantic cargo check: {err}."))?;
	let stdout = String::from_utf8(output.stdout)?;
	let warning_lines = collect_unused_super_glob_lines(&stdout);
	let mut applied = 0_usize;

	for (path, lines) in warning_lines {
		if !tracked.contains(&path) {
			continue;
		}

		applied += add_allow_unused_imports_for_super_glob(&path, &lines)?;
	}

	Ok(applied)
}

fn leading_whitespace(line: &str) -> &str {
	let cut = line
		.char_indices()
		.find_map(|(idx, ch)| (!ch.is_whitespace()).then_some(idx))
		.unwrap_or(line.len());

	&line[..cut]
}

fn normalize_path(path: &Path) -> PathBuf {
	match fs::canonicalize(path) {
		Ok(canonical) => canonical,
		Err(_) => path.to_path_buf(),
	}
}

fn collect_unused_super_glob_lines(output: &str) -> BTreeMap<PathBuf, BTreeSet<usize>> {
	let mut result: BTreeMap<PathBuf, BTreeSet<usize>> = BTreeMap::new();

	for line in output.lines() {
		let Ok(value) = serde_json::from_str::<Value>(line) else {
			continue;
		};

		if value.get("reason").and_then(Value::as_str) != Some("compiler-message") {
			continue;
		}

		let Some(message) = value.get("message") else {
			continue;
		};

		if message.get("level").and_then(Value::as_str) != Some("warning") {
			continue;
		}

		let text = message.get("message").and_then(Value::as_str).unwrap_or_default();

		if !text.contains("unused import") || !text.contains("super::*") {
			continue;
		}

		let Some(spans) = message.get("spans").and_then(Value::as_array) else {
			continue;
		};
		let Some(primary) = spans
			.iter()
			.find(|span| span.get("is_primary").and_then(Value::as_bool).unwrap_or(false))
		else {
			continue;
		};
		let Some(file_name) = primary.get("file_name").and_then(Value::as_str) else {
			continue;
		};
		let Some(line_start) = primary.get("line_start").and_then(Value::as_u64) else {
			continue;
		};
		let line_start = line_start as usize;

		if line_start == 0 {
			continue;
		}

		result.entry(normalize_path(Path::new(file_name))).or_default().insert(line_start);
	}

	result
}

fn add_allow_unused_imports_for_super_glob(path: &Path, lines: &BTreeSet<usize>) -> Result<usize> {
	if lines.is_empty() {
		return Ok(0);
	}

	let Ok(text) = fs::read_to_string(path) else {
		return Ok(0);
	};
	let has_trailing_newline = text.ends_with('\n');
	let mut all_lines = text.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
	let mut inserted = 0_usize;

	for line_no in lines.iter().rev() {
		let idx = line_no.saturating_sub(1);
		let Some(use_line) = all_lines.get(idx) else {
			continue;
		};

		if !use_line.contains("use super::*;") {
			continue;
		}

		let indent = leading_whitespace(use_line).to_owned();
		let allow_line = format!("{indent}#[allow(unused_imports)]");
		let already_allowed = idx > 0 && all_lines[idx - 1].trim() == "#[allow(unused_imports)]";

		if already_allowed {
			continue;
		}

		all_lines.insert(idx, allow_line);

		inserted += 1;
	}

	if inserted == 0 {
		return Ok(0);
	}

	let mut rewritten = all_lines.join("\n");

	if has_trailing_newline {
		rewritten.push('\n');
	}

	fs::write(path, rewritten)?;

	Ok(inserted)
}

#[cfg(test)]
mod tests {
	use std::{
		collections::BTreeSet,
		env, process,
		time::{SystemTime, UNIX_EPOCH},
	};

	use super::*;

	#[test]
	fn injects_allow_before_super_glob() {
		let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Read timestamp.").as_nanos();
		let path = env::temp_dir().join(format!("vstyle-semantic-fix-{}-{now}.rs", process::id()));
		let content =
			"#[cfg(test)]\nmod tests {\n\tuse super::*;\n\t#[test]\n\tfn sample_case() {}\n}\n";

		fs::write(&path, content).expect("Write fixture.");

		let mut lines = BTreeSet::new();

		lines.insert(3);

		let applied = add_allow_unused_imports_for_super_glob(&path, &lines).expect("Apply fix.");

		assert_eq!(applied, 1);

		let rewritten = fs::read_to_string(&path).expect("Read fixture.");

		assert!(rewritten.contains("\t#[allow(unused_imports)]\n\tuse super::*;"));

		let _ = fs::remove_file(path);
	}
}
