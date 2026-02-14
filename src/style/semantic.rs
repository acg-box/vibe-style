use std::{
	collections::{BTreeMap, BTreeSet},
	fs,
	path::{Path, PathBuf},
	process::Command,
};

use color_eyre::{Result, eyre};
use serde_json::Value;

use crate::style::shared::CargoOptions;

const MAX_IMPORT_SUGGESTION_ROUNDS: usize = 4;

#[derive(Debug, Clone)]
struct ImportSuggestion {
	line: usize,
	imports: Vec<String>,
}

pub(crate) fn apply_semantic_fixes(
	files: &[PathBuf],
	cargo_options: &CargoOptions,
) -> Result<usize> {
	if files.is_empty() {
		return Ok(0);
	}

	let tracked = files.iter().map(|path| normalize_path(path)).collect::<BTreeSet<_>>();
	let mut applied_total = 0_usize;

	for _ in 0..MAX_IMPORT_SUGGESTION_ROUNDS {
		let stdout = run_semantic_cargo_check(cargo_options)?;
		let suggestions = collect_missing_import_suggestions(&stdout);
		let mut applied_round = 0_usize;

		for (path, imports) in suggestions {
			if !tracked.contains(&path) {
				continue;
			}

			applied_round += apply_missing_import_suggestions(&path, &imports)?;
		}

		applied_total += applied_round;

		if applied_round == 0 {
			break;
		}
	}

	Ok(applied_total)
}

pub(crate) fn collect_compiler_error_files(
	files: &[PathBuf],
	cargo_options: &CargoOptions,
) -> Result<BTreeSet<PathBuf>> {
	if files.is_empty() {
		return Ok(BTreeSet::new());
	}

	let tracked = files.iter().map(|path| normalize_path(path)).collect::<BTreeSet<_>>();
	let stdout = run_semantic_cargo_check(cargo_options)?;
	let all = collect_compiler_error_files_from_output(&stdout);

	Ok(all.into_iter().filter(|path| tracked.contains(path)).collect())
}

fn run_semantic_cargo_check(cargo_options: &CargoOptions) -> Result<String> {
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

	String::from_utf8(output.stdout).map_err(Into::into)
}

fn normalize_path(path: &Path) -> PathBuf {
	match fs::canonicalize(path) {
		Ok(canonical) => canonical,
		Err(_) => path.to_path_buf(),
	}
}

fn collect_missing_import_suggestions(output: &str) -> BTreeMap<PathBuf, Vec<ImportSuggestion>> {
	let mut suggestions: BTreeMap<PathBuf, Vec<ImportSuggestion>> = BTreeMap::new();

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
		let Some(children) = message.get("children").and_then(Value::as_array) else {
			continue;
		};

		for child in children {
			if child.get("level").and_then(Value::as_str) != Some("help") {
				continue;
			}

			let Some(spans) = child.get("spans").and_then(Value::as_array) else {
				continue;
			};
			let Some(best) = select_best_import_from_help_spans(spans) else {
				continue;
			};

			suggestions.entry(best.0).or_default().push(best.1);
		}
	}

	suggestions
}

fn extract_use_statements_from_replacement(replacement: &str) -> Vec<String> {
	let mut out = replacement
		.lines()
		.map(str::trim)
		.filter(|line| line.starts_with("use ") && line.ends_with(';'))
		.filter(|line| !line.contains('*'))
		.map(ToOwned::to_owned)
		.collect::<Vec<_>>();

	out.sort();
	out.dedup();

	out
}

fn select_best_import_from_help_spans(spans: &[Value]) -> Option<(PathBuf, ImportSuggestion)> {
	let mut candidates = Vec::new();

	for span in spans {
		let Some(replacement) = span.get("suggested_replacement").and_then(Value::as_str) else {
			continue;
		};
		let Some(file_name) = span.get("file_name").and_then(Value::as_str) else {
			continue;
		};
		let Some(line) = span.get("line_start").and_then(Value::as_u64).map(|value| value as usize)
		else {
			continue;
		};
		let imports = extract_use_statements_from_replacement(replacement);

		if imports.is_empty() {
			continue;
		}

		candidates.push((
			normalize_path(Path::new(file_name)),
			ImportSuggestion { line, imports: imports.clone() },
			import_candidate_rank(&imports[0]),
		));
	}

	candidates
		.into_iter()
		.min_by(|left, right| left.2.cmp(&right.2))
		.map(|(path, suggestion, _)| (path, suggestion))
}

fn import_candidate_rank(import_stmt: &str) -> (usize, usize, usize) {
	let path = import_stmt.trim_start_matches("use ").trim_end_matches(';').trim();
	let root = path.split("::").next().unwrap_or_default();
	let root_penalty = match root {
		"crate" | "self" | "super" => 2,
		"std" | "core" | "alloc" => 0,
		_ => 1,
	};
	let segments = path.split("::").count();

	(root_penalty, segments, path.len())
}

fn apply_missing_import_suggestions(
	path: &Path,
	suggestions: &[ImportSuggestion],
) -> Result<usize> {
	if suggestions.is_empty() {
		return Ok(0);
	}

	let Ok(mut text) = fs::read_to_string(path) else {
		return Ok(0);
	};
	let mut applied = 0_usize;
	let mut ordered = suggestions.to_vec();

	ordered.sort_by(|left, right| right.line.cmp(&left.line));

	for suggestion in ordered {
		let line_start = line_start_offset(&text, suggestion.line).unwrap_or(0);
		let indent = indentation_of_line_at(&text, line_start);
		let mut block = String::new();
		let mut inserted_any = false;

		for import in suggestion.imports {
			if has_use_near_line(&text, suggestion.line, &import) {
				continue;
			}

			block.push_str(&indent);
			block.push_str(&import);
			block.push('\n');

			inserted_any = true;
		}

		if !inserted_any {
			continue;
		}

		text.insert_str(line_start, &block);

		applied += 1;
	}

	if applied == 0 {
		return Ok(0);
	}

	fs::write(path, text)?;

	Ok(applied)
}

fn line_start_offset(text: &str, line: usize) -> Option<usize> {
	if line <= 1 {
		return Some(0);
	}

	let mut current_line = 1_usize;
	let mut offset = 0_usize;

	for segment in text.split_inclusive('\n') {
		current_line += 1;
		offset += segment.len();

		if current_line == line {
			return Some(offset);
		}
	}

	None
}

fn indentation_of_line_at(text: &str, line_start: usize) -> String {
	text.get(line_start..)
		.and_then(|tail| tail.lines().next())
		.unwrap_or_default()
		.chars()
		.take_while(|ch| ch.is_whitespace())
		.collect()
}

fn has_use_near_line(text: &str, line: usize, import: &str) -> bool {
	let target = import.trim();
	let start = line.saturating_sub(3);
	let end = line + 3;

	for (idx, candidate) in text.lines().enumerate() {
		let line_no = idx + 1;

		if line_no < start || line_no > end {
			continue;
		}
		if candidate.trim() == target {
			return true;
		}
	}

	false
}

fn collect_compiler_error_files_from_output(output: &str) -> BTreeSet<PathBuf> {
	let mut result = BTreeSet::new();

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

		if message.get("level").and_then(Value::as_str) != Some("error") {
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

		result.insert(normalize_path(Path::new(file_name)));
	}

	result
}

#[cfg(test)]
mod tests {
	use std::{
		env,
		path::Path,
		process,
		time::{SystemTime, UNIX_EPOCH},
	};

	use crate::style::semantic::{
		ImportSuggestion, apply_missing_import_suggestions, collect_missing_import_suggestions, fs,
		normalize_path,
	};

	#[test]
	fn extracts_use_suggestions_from_rustc_help_replacement() {
		let output = r#"{"reason":"compiler-message","message":{"children":[{"level":"help","spans":[{"file_name":"a.rs","line_start":1,"suggested_replacement":"use std::collections::HashMap;\n\n"}]}]}}"#;
		let suggestions = collect_missing_import_suggestions(output);
		let imports =
			suggestions.get(&normalize_path(Path::new("a.rs"))).expect("import suggestions");

		assert!(
			imports
				.iter()
				.flat_map(|suggestion| suggestion.imports.iter())
				.any(|import| import == "use std::collections::HashMap;")
		);
	}

	#[test]
	fn applies_missing_import_suggestions_once() {
		let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("Read timestamp.").as_nanos();
		let path =
			env::temp_dir().join(format!("vstyle-semantic-imports-{}-{now}.rs", process::id()));
		let content = "fn run() {\n\tlet _ = HashMap::<u8, u8>::new();\n}\n";

		fs::write(&path, content).expect("Write fixture.");

		let suggestions = vec![ImportSuggestion {
			line: 1,
			imports: vec!["use std::collections::HashMap;".to_owned()],
		}];
		let applied =
			apply_missing_import_suggestions(&path, &suggestions).expect("Apply suggestions.");
		let rewritten = fs::read_to_string(&path).expect("Read rewritten file.");

		assert_eq!(applied, 1);
		assert!(rewritten.contains("use std::collections::HashMap;"));

		let second = apply_missing_import_suggestions(&path, &suggestions).expect("Re-apply.");

		assert_eq!(second, 0);
	}
}
