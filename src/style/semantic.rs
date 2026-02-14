use std::{
	collections::{BTreeMap, BTreeSet},
	fs,
	path::{Path, PathBuf},
	process::Command,
	sync::{
		OnceLock,
		atomic::{AtomicU64, Ordering},
	},
};

use color_eyre::{Result, eyre};
use serde_json::Value;

use crate::style::shared::CargoOptions;

const DEFAULT_MAX_IMPORT_SUGGESTION_ROUNDS: usize = 2;
const MAX_IMPORT_SUGGESTION_ROUNDS_ENV: &str = "VSTYLE_MAX_IMPORT_SUGGESTION_ROUNDS";
const HARD_MAX_IMPORT_SUGGESTION_ROUNDS: usize = 16;
const CACHE_DIR_SUFFIX: &str = "target/vstyle-cache/semantic";

static SEMANTIC_CACHE_HIT_COUNT: AtomicU64 = AtomicU64::new(0);
static SEMANTIC_CACHE_MISS_COUNT: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy)]
pub(crate) struct SemanticCacheStats {
	pub(crate) hits: u64,
	pub(crate) misses: u64,
}

#[derive(Debug, Clone)]
struct ImportSuggestion {
	line: usize,
	imports: Vec<String>,
}

pub(crate) fn apply_semantic_fixes(
	files: &[PathBuf],
	cargo_options: &CargoOptions,
	verbose: bool,
) -> Result<usize> {
	if files.is_empty() {
		return Ok(0);
	}

	let tracked = files.iter().map(|path| normalize_path(path)).collect::<BTreeSet<_>>();
	let mut applied_total = 0_usize;

	for _ in 0..max_import_suggestion_rounds() {
		let stdout = run_semantic_cargo_check(cargo_options, files, verbose)?;
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
	verbose: bool,
) -> Result<BTreeSet<PathBuf>> {
	if files.is_empty() {
		return Ok(BTreeSet::new());
	}

	let tracked = files.iter().map(|path| normalize_path(path)).collect::<BTreeSet<_>>();
	let stdout = run_semantic_cargo_check(cargo_options, files, verbose)?;
	let all = collect_compiler_error_files_from_output(&stdout);

	Ok(all.into_iter().filter(|path| tracked.contains(path)).collect())
}

pub(crate) fn cache_stats() -> SemanticCacheStats {
	SemanticCacheStats {
		hits: SEMANTIC_CACHE_HIT_COUNT.load(Ordering::Relaxed),
		misses: SEMANTIC_CACHE_MISS_COUNT.load(Ordering::Relaxed),
	}
}

pub(crate) fn reset_cache_stats() {
	SEMANTIC_CACHE_HIT_COUNT.store(0, Ordering::Relaxed);
	SEMANTIC_CACHE_MISS_COUNT.store(0, Ordering::Relaxed);
}

fn record_cache_hit() {
	SEMANTIC_CACHE_HIT_COUNT.fetch_add(1, Ordering::Relaxed);
}

fn record_cache_miss() {
	SEMANTIC_CACHE_MISS_COUNT.fetch_add(1, Ordering::Relaxed);
}

fn max_import_suggestion_rounds() -> usize {
	static VALUE: OnceLock<usize> = OnceLock::new();

	*VALUE.get_or_init(|| {
		let Ok(raw) = std::env::var(MAX_IMPORT_SUGGESTION_ROUNDS_ENV) else {
			return DEFAULT_MAX_IMPORT_SUGGESTION_ROUNDS;
		};
		let trimmed = raw.trim();
		let parsed = match trimmed.parse::<usize>() {
			Ok(value) => value,
			Err(err) => {
				eprintln!(
					"Invalid {MAX_IMPORT_SUGGESTION_ROUNDS_ENV} value '{trimmed}': {err}. Using default {DEFAULT_MAX_IMPORT_SUGGESTION_ROUNDS}."
				);

				return DEFAULT_MAX_IMPORT_SUGGESTION_ROUNDS;
			},
		};

		if parsed == 0 {
			eprintln!(
				"{MAX_IMPORT_SUGGESTION_ROUNDS_ENV}=0 is not valid. Using default {DEFAULT_MAX_IMPORT_SUGGESTION_ROUNDS}."
			);

			return DEFAULT_MAX_IMPORT_SUGGESTION_ROUNDS;
		}
		if parsed > HARD_MAX_IMPORT_SUGGESTION_ROUNDS {
			eprintln!(
				"{MAX_IMPORT_SUGGESTION_ROUNDS_ENV}={parsed} exceeds the hard cap {HARD_MAX_IMPORT_SUGGESTION_ROUNDS}. Using {HARD_MAX_IMPORT_SUGGESTION_ROUNDS}."
			);

			return HARD_MAX_IMPORT_SUGGESTION_ROUNDS;
		}

		parsed
	})
}

fn run_semantic_cargo_check(
	cargo_options: &CargoOptions,
	files: &[PathBuf],
	verbose: bool,
) -> Result<String> {
	let args = semantic_check_args(cargo_options);
	let cache_path = semantic_cache_path(cargo_options, files, verbose);

	if let Some(cache_path) = cache_path.as_ref() {
		match read_cached_semantic_output(cache_path, verbose) {
			Some(stdout) => {
				record_cache_hit();

				return Ok(stdout);
			},
			None => {
				record_cache_miss();
			},
		}
	} else {
		record_cache_miss();
	}

	let mut cmd = Command::new("cargo");

	for arg in args {
		cmd.arg(arg);
	}

	let output =
		cmd.output().map_err(|err| eyre::eyre!("Failed to run semantic cargo check: {err}."))?;
	let stdout = String::from_utf8(output.stdout)
		.map_err(|err| eyre::eyre!("Failed to parse cargo check output: {err}."))?;

	if let Some(cache_path) = cache_path {
		write_cached_semantic_output(&cache_path, &stdout, verbose);
	}

	Ok(stdout)
}

fn semantic_check_args(cargo_options: &CargoOptions) -> Vec<String> {
	let mut args = Vec::new();

	args.push("check".to_owned());
	args.push("--all-targets".to_owned());
	args.push("--message-format=json".to_owned());

	if cargo_options.workspace {
		args.push("--workspace".to_owned());
	}

	for package in &cargo_options.packages {
		args.push("-p".to_owned());
		args.push(package.clone());
	}

	if !cargo_options.features.is_empty() {
		args.push("--features".to_owned());
		args.push(cargo_options.features.join(","));
	}
	if cargo_options.all_features {
		args.push("--all-features".to_owned());
	}
	if cargo_options.no_default_features {
		args.push("--no-default-features".to_owned());
	}

	args
}

fn semantic_cache_path(
	cargo_options: &CargoOptions,
	files: &[PathBuf],
	verbose: bool,
) -> Option<PathBuf> {
	let key = match semantic_cache_key(cargo_options, files, verbose) {
		Ok(key) => key,
		Err(err) => {
			log_verbose_error(verbose, &format!("Semantic cache key generation failed: {err}."));

			return None;
		},
	};
	let cache_dir = semantic_cache_dir(verbose)?;

	Some(cache_dir.join(format!("{key}.txt")))
}

fn semantic_cache_key(
	cargo_options: &CargoOptions,
	files: &[PathBuf],
	verbose: bool,
) -> Result<String> {
	let args = semantic_check_args(cargo_options);
	let mut input = String::new();

	input.push_str("vstyle=");
	input.push_str(env!("CARGO_PKG_VERSION"));
	input.push('\n');
	input.push_str("vstyle_git=");
	input.push_str(env!("VERGEN_GIT_SHA"));
	input.push('\n');
	input.push_str("vstyle_target=");
	input.push_str(env!("VERGEN_CARGO_TARGET_TRIPLE"));
	input.push('\n');
	input.push_str("rustc_version=");
	input.push_str(&rustc_version_signature(verbose));
	input.push('\n');
	input.push_str("cargo_lock=");
	input.push_str(&cargo_lock_fingerprint(verbose).unwrap_or_else(|| "missing".to_owned()));
	input.push('\n');
	input.push_str("cargo_args=");
	input.push_str(&args.join(" "));
	input.push('\n');
	input.push_str("tracked_files=");

	let mut tracked_files = files.to_vec();

	tracked_files.sort();

	for file in tracked_files {
		let fingerprint = file_fingerprint(&file, verbose)?;

		input.push('\n');
		input.push_str(&normalize_cache_path(&fingerprint.0));
		input.push(':');
		input.push_str(&fingerprint.1);
	}

	Ok(stable_hash_hex(input.as_bytes()))
}

fn semantic_cache_dir(verbose: bool) -> Option<PathBuf> {
	let base = match std::env::current_dir() {
		Ok(current) => current,
		Err(err) => {
			log_verbose_error(verbose, &format!("Failed to resolve current directory: {err}."));

			return None;
		},
	};
	let cache_dir = base.join(CACHE_DIR_SUFFIX);

	if let Err(err) = fs::create_dir_all(&cache_dir) {
		log_verbose_error(verbose, &format!("Failed to create cache directory: {err}."));

		return None;
	}

	Some(cache_dir)
}

fn read_cached_semantic_output(cache_path: &Path, verbose: bool) -> Option<String> {
	let output = match fs::read_to_string(cache_path) {
		Ok(contents) => contents,
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
		Err(err) => {
			log_verbose_error(
				verbose,
				&format!("Could not read semantic cache file '{}': {err}.", cache_path.display()),
			);

			return None;
		},
	};

	Some(output)
}

fn write_cached_semantic_output(cache_path: &Path, stdout: &str, verbose: bool) {
	let temp_path = cache_path.with_file_name(format!(
		".{}.{}.tmp",
		cache_path.file_name().and_then(|name| name.to_str()).unwrap_or("semantic-cache"),
		std::process::id()
	));

	if let Err(err) = fs::write(&temp_path, stdout) {
		log_verbose_error(
			verbose,
			&format!("Could not write semantic cache temp file '{}': {err}.", temp_path.display()),
		);

		return;
	}
	if let Err(err) = fs::rename(&temp_path, cache_path) {
		let _ = fs::remove_file(&temp_path);

		log_verbose_error(
			verbose,
			&format!("Could not write semantic cache file '{}': {err}.", cache_path.display()),
		);
	}
}

fn file_fingerprint(path: &Path, verbose: bool) -> Result<(PathBuf, String)> {
	let absolute = if path.is_absolute() {
		path.to_path_buf()
	} else {
		let cwd = std::env::current_dir().map_err(|err| {
			eyre::eyre!("Failed to resolve current directory for cache fingerprint: {err}.")
		})?;

		cwd.join(path)
	};
	let bytes = match fs::read(&absolute) {
		Ok(bytes) => bytes,
		Err(err) => {
			log_verbose_error(
				verbose,
				&format!(
					"Failed to read tracked file '{}' for cache fingerprint: {err}.",
					absolute.display()
				),
			);

			return Err(eyre::eyre!(
				"Failed to read tracked file '{}' for cache fingerprint: {err}.",
				absolute.display()
			));
		},
	};
	let hash = stable_hash_hex(&bytes);
	let canonical = normalize_path(&absolute);

	Ok((canonical, hash))
}

fn cargo_lock_fingerprint(verbose: bool) -> Option<String> {
	let cwd = match std::env::current_dir() {
		Ok(cwd) => cwd,
		Err(err) => {
			log_verbose_error(verbose, &format!("Failed to resolve current directory: {err}."));

			return None;
		},
	};
	let lock_path = cwd.join("Cargo.lock");
	let bytes = match fs::read(&lock_path) {
		Ok(contents) => contents,
		Err(err) => {
			log_verbose_error(verbose, &format!("Failed to read Cargo.lock for cache key: {err}."));

			return None;
		},
	};

	Some(stable_hash_hex(&bytes))
}

fn rustc_version_signature(verbose: bool) -> String {
	let output = match Command::new("rustc").arg("-Vv").output() {
		Ok(output) => output,
		Err(err) => {
			log_verbose_error(verbose, &format!("Failed to run rustc -Vv for cache key: {err}."));

			return "unknown".to_owned();
		},
	};

	match String::from_utf8(output.stdout) {
		Ok(version) => version.trim_end().to_owned(),
		Err(err) => {
			log_verbose_error(verbose, &format!("Failed to decode rustc -Vv output: {err}."));

			"unknown".to_owned()
		},
	}
}

fn normalize_cache_path(path: &Path) -> String {
	path.to_string_lossy().replace('\\', "/")
}

fn stable_hash_hex(value: &[u8]) -> String {
	let mut hash = 0xcbf29ce484222325u64;

	for &byte in value {
		hash ^= byte as u64;
		hash = hash.wrapping_mul(0x100000001b3);
	}

	format!("{hash:016x}")
}

fn log_verbose_error(verbose: bool, message: &str) {
	if verbose {
		eprintln!("{message}");
	}
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
