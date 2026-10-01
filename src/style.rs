mod bindings;
mod file;
mod fixes;
mod generics;
mod impls;
mod imports;
mod module;
mod quality;
mod semantic;
mod shared;
mod spacing;
mod swift;
mod test_modules;
mod types;

pub(crate) use shared::{CargoOptions, RunSummary, StyleLanguage};

use std::{
	collections::{BTreeMap, BTreeSet},
	fs, mem,
	path::{Path, PathBuf},
	time::{Duration, Instant},
};

use color_eyre::{Result, eyre};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};

use semantic::SemanticCacheStats;
use shared::{Edit, FileContext, STYLE_RULE_IDS, Violation};

type FixRoundScope = (Vec<PathBuf>, CargoOptions);

const FILE_BATCH_SIZE: usize = 64;
const MAX_FIX_PASSES: usize = 8;
const MAX_TUNE_ROUNDS: usize = 4;

#[derive(Debug)]
struct FileFixOutcome {
	path: PathBuf,
	original_text: Option<String>,
	rewritten_text: Option<String>,
	import_fallback_source_text: Option<String>,
	had_let_mut_reorder_edits: bool,
	had_type_alias_rename_edits: bool,
	applied_count: usize,
}

#[derive(Debug)]
struct ImportFallbackPlan {
	path: PathBuf,
	source_text: String,
}

#[derive(Clone, Copy, Debug, Default)]
struct FileViolationCounts {
	violation_count: usize,
	unfixable_count: usize,
}

#[derive(Debug)]
struct FileCheckOutcome {
	path: PathBuf,
	violations: Vec<Violation>,
}

#[derive(Debug)]
struct CheckState {
	counts_by_file: BTreeMap<PathBuf, FileViolationCounts>,
	violation_count: usize,
	unfixable_count: usize,
}
impl CheckState {
	fn fixable_count(&self) -> usize {
		self.violation_count.saturating_sub(self.unfixable_count)
	}

	fn apply_updates(&mut self, updates: BTreeMap<PathBuf, FileViolationCounts>) {
		for (path, counts) in updates {
			let previous = self.counts_by_file.insert(path, counts).unwrap_or_default();

			self.violation_count = self
				.violation_count
				.saturating_sub(previous.violation_count)
				.saturating_add(counts.violation_count);
			self.unfixable_count = self
				.unfixable_count
				.saturating_sub(previous.unfixable_count)
				.saturating_add(counts.unfixable_count);
		}
	}
}

#[derive(Debug, Default)]
struct TypeAliasRenamePlan {
	renames: BTreeMap<String, String>,
	definition_edits: BTreeMap<PathBuf, Vec<Edit>>,
}

#[derive(Clone, Debug, Default)]
struct FixRoundSummary {
	applied_count: usize,
	requires_follow_up_round: bool,
	changed_files: Vec<PathBuf>,
}

struct SemanticValidationFallbacks<'a> {
	files: &'a [PathBuf],
	semantic_cargo_options: &'a CargoOptions,
	baseline_error_files: &'a BTreeSet<PathBuf>,
	post_error_files: Option<&'a BTreeSet<PathBuf>>,
	verbose: bool,
	progress: bool,
	import_fallbacks: &'a BTreeMap<PathBuf, ImportFallbackPlan>,
	changed_originals: &'a BTreeMap<PathBuf, (PathBuf, String)>,
	let_mut_reorder_files: &'a BTreeMap<PathBuf, ()>,
	type_alias_rename_files: &'a BTreeMap<PathBuf, ()>,
}

#[derive(Debug)]
struct FileFixApplication {
	changed_files: Vec<PathBuf>,
	total_applied: usize,
	import_fallbacks: BTreeMap<PathBuf, ImportFallbackPlan>,
	changed_originals: BTreeMap<PathBuf, (PathBuf, String)>,
	let_mut_reorder_files: BTreeMap<PathBuf, ()>,
	type_alias_rename_files: BTreeMap<PathBuf, ()>,
}

#[derive(Debug)]
struct CollectedFileFixes {
	outcomes: Vec<FileFixOutcome>,
	changed_files: Vec<PathBuf>,
	total_applied: usize,
}

pub(crate) fn run_check(cargo_options: &CargoOptions) -> Result<RunSummary> {
	let (summary, _state) = run_check_with_state(cargo_options)?;

	Ok(summary)
}

pub(crate) fn semantic_cache_stats() -> SemanticCacheStats {
	semantic::cache_stats()
}

pub(crate) fn print_coverage() {
	for rule in STYLE_RULE_IDS {
		println!("{rule}\timplemented");
	}
}

pub(crate) fn run_fix(
	cargo_options: &CargoOptions,
	verbose: bool,
	progress: bool,
) -> Result<RunSummary> {
	semantic::reset_cache_stats();

	let files = shared::resolve_files(cargo_options)?;
	let run_start_snapshot = collect_file_snapshots(&files);
	let result = run_fix_inner(cargo_options, verbose, progress);

	match result {
		Ok(summary) => Ok(summary),
		Err(error) => {
			let changed_files = changed_files_since_snapshot(&run_start_snapshot);

			if changed_files.is_empty() {
				return Err(error);
			}

			if let Err(rollback_error) = restore_file_snapshots(&run_start_snapshot) {
				return Err(eyre::eyre!(
					"vstyle tune failed and rollback also failed. Original error: {error:?}. Rollback error: {rollback_error:?}."
				));
			}

			eprintln!(
				"vstyle tune: rolled back {} file(s) because the run failed.",
				changed_files.len()
			);

			Err(error)
		},
	}
}

fn run_fix_inner(
	cargo_options: &CargoOptions,
	verbose: bool,
	progress: bool,
) -> Result<RunSummary> {
	let (initial_checked, mut check_state) = run_initial_check(cargo_options, progress)?;
	let mut total_applied = 0_usize;
	let mut previous_fixable_count = check_state.fixable_count();

	if should_skip_tune_rounds(previous_fixable_count) {
		if progress {
			eprintln!(
				"vstyle tune: initial scan found no fixable violations; skipping fix rounds."
			);
		}

		return Ok(initial_checked);
	}

	let mut non_decreasing_rounds = 0_usize;

	for round in 0..MAX_TUNE_ROUNDS {
		let fix_round_scopes = resolve_fix_round_scopes(cargo_options)?;
		let parallel_scopes = should_parallelize_fix_scopes(&fix_round_scopes);

		if progress {
			let mode = if parallel_scopes { "parallel" } else { "serial" };

			eprintln!(
				"vstyle tune: round {}/{} with {} scope(s) ({mode}).",
				round + 1,
				MAX_TUNE_ROUNDS,
				fix_round_scopes.len(),
			);
		}

		let scope_summaries = run_fix_rounds_for_scopes(&fix_round_scopes, verbose, progress)?;
		let mut applied_this_round = 0_usize;
		let mut requires_follow_up_round = false;
		let mut changed_files = BTreeSet::<PathBuf>::new();

		for scope_summary in scope_summaries {
			applied_this_round += scope_summary.applied_count;
			requires_follow_up_round |= scope_summary.requires_follow_up_round;

			changed_files.extend(scope_summary.changed_files);
		}

		total_applied += applied_this_round;

		let changed_files = changed_files.into_iter().collect::<Vec<_>>();

		if !changed_files.is_empty() {
			refresh_check_state_for_files(&changed_files, &mut check_state)?;
		}

		let fixable_count = check_state.fixable_count();
		let needs_follow_up_round =
			requires_follow_up_round || fixable_count < previous_fixable_count;
		let (should_stop, next_non_decreasing_rounds) = should_stop_tune_round(
			applied_this_round,
			fixable_count,
			previous_fixable_count,
			non_decreasing_rounds,
			needs_follow_up_round,
		);

		if should_stop {
			if progress {
				eprintln!(
					"vstyle tune: stopping after round {} (applied {}, remaining {}).",
					round + 1,
					applied_this_round,
					fixable_count,
				);
			}

			break;
		}

		non_decreasing_rounds = next_non_decreasing_rounds;
		previous_fixable_count = fixable_count;
	}

	let mut checked = run_final_check(cargo_options, initial_checked, total_applied, progress)?;

	checked.applied_fix_count = total_applied;

	Ok(RunSummary {
		file_count: checked.file_count,
		violation_count: checked.violation_count,
		unfixable_count: checked.unfixable_count,
		applied_fix_count: total_applied,
		output_lines: checked.output_lines,
	})
}

fn run_initial_check(
	cargo_options: &CargoOptions,
	progress: bool,
) -> Result<(RunSummary, CheckState)> {
	if progress {
		eprintln!("vstyle tune: starting initial scan.");
	}

	let started = Instant::now();
	let (summary, state) = run_check_with_state(cargo_options)?;

	if progress {
		eprintln!(
			"vstyle tune: initial scan checked {} file(s), found {} violation(s), {} fixable, {} manual ({}).",
			summary.file_count,
			summary.violation_count,
			state.fixable_count(),
			summary.unfixable_count,
			format_duration(started.elapsed()),
		);
	}

	Ok((summary, state))
}

fn run_final_check(
	cargo_options: &CargoOptions,
	initial_checked: RunSummary,
	total_applied: usize,
	progress: bool,
) -> Result<RunSummary> {
	if total_applied == 0 {
		return Ok(initial_checked);
	}
	if progress {
		eprintln!("vstyle tune: starting final scan.");
	}

	let started = Instant::now();
	let checked = run_check(cargo_options)?;

	if progress {
		eprintln!(
			"vstyle tune: final scan checked {} file(s), found {} remaining violation(s), {} manual ({}).",
			checked.file_count,
			checked.violation_count,
			checked.unfixable_count,
			format_duration(started.elapsed()),
		);
	}

	Ok(checked)
}

fn format_duration(duration: Duration) -> String {
	let millis = duration.as_millis();

	if millis < 1_000 {
		return format!("{millis}ms");
	}

	format!("{}.{:03}s", millis / 1_000, millis % 1_000)
}

fn run_check_with_state(cargo_options: &CargoOptions) -> Result<(RunSummary, CheckState)> {
	let files = shared::resolve_files(cargo_options)?;
	let outcomes = collect_check_outcomes(&files)?;
	let mut violations: Vec<Violation> = Vec::new();
	let mut counts_by_file = BTreeMap::<PathBuf, FileViolationCounts>::new();

	for outcome in outcomes {
		let counts = FileViolationCounts {
			violation_count: outcome.violations.len(),
			unfixable_count: outcome
				.violations
				.iter()
				.filter(|violation| !violation.fixable)
				.count(),
		};

		counts_by_file.insert(outcome.path.clone(), counts);
		violations.extend(outcome.violations);
	}

	violations
		.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)).then(a.rule.cmp(b.rule)));

	let unfixable_count = violations.iter().filter(|v| !v.fixable).count();
	let output_lines = violations.into_iter().map(|v| v.format()).collect::<Vec<_>>();
	let violation_count = output_lines.len();
	let summary = RunSummary {
		file_count: files.len(),
		violation_count,
		unfixable_count,
		applied_fix_count: 0,
		output_lines,
	};
	let state = CheckState { counts_by_file, violation_count, unfixable_count };

	Ok((summary, state))
}

fn should_stop_tune_round(
	applied_this_round: usize,
	fixable_count: usize,
	previous_fixable_count: usize,
	non_decreasing_rounds: usize,
	requires_follow_up_round: bool,
) -> (bool, usize) {
	if applied_this_round == 0 || fixable_count == 0 {
		return (true, non_decreasing_rounds);
	}
	if !requires_follow_up_round {
		return (true, non_decreasing_rounds);
	}

	let next_non_decreasing_rounds =
		if fixable_count < previous_fixable_count { 0 } else { non_decreasing_rounds + 1 };

	if next_non_decreasing_rounds >= 2 {
		return (true, next_non_decreasing_rounds);
	}

	(false, next_non_decreasing_rounds)
}

fn should_skip_tune_rounds(initial_fixable_count: usize) -> bool {
	initial_fixable_count == 0
}

fn resolve_fix_round_scopes(cargo_options: &CargoOptions) -> Result<Vec<FixRoundScope>> {
	let files = shared::resolve_files(cargo_options)?;

	if !cargo_options.workspace || !cargo_options.packages.is_empty() {
		return Ok(vec![(files, cargo_options.clone())]);
	}

	let Some(files_by_package) = shared::package_file_map_for_files(&files)? else {
		return Ok(vec![(files, cargo_options.clone())]);
	};
	let mut scopes = Vec::new();

	for (package, scoped_files) in files_by_package {
		let mut scoped_options = cargo_options.clone();

		scoped_options.workspace = false;
		scoped_options.packages = vec![package];

		if scoped_files.is_empty() {
			continue;
		}

		scopes.push((scoped_files, scoped_options));
	}

	if scopes.is_empty() {
		return Ok(vec![(files, cargo_options.clone())]);
	}

	Ok(scopes)
}

fn run_fix_rounds_for_scopes(
	fix_round_scopes: &[FixRoundScope],
	verbose: bool,
	progress: bool,
) -> Result<Vec<FixRoundSummary>> {
	if should_parallelize_fix_scopes(fix_round_scopes) {
		let results = fix_round_scopes
			.par_iter()
			.map(|(scope_files, scope_options)| {
				run_fix_round(scope_files, scope_options, verbose, progress)
			})
			.collect::<Vec<_>>();
		let mut summaries = Vec::with_capacity(results.len());

		for result in results {
			summaries.push(result?);
		}

		return Ok(summaries);
	}

	let mut summaries = Vec::with_capacity(fix_round_scopes.len());

	for (scope_files, scope_options) in fix_round_scopes {
		summaries.push(run_fix_round(scope_files, scope_options, verbose, progress)?);
	}

	Ok(summaries)
}

fn should_parallelize_fix_scopes(fix_round_scopes: &[FixRoundScope]) -> bool {
	fix_round_scopes.len() > 1 && fix_scope_files_are_disjoint(fix_round_scopes)
}

fn fix_scope_files_are_disjoint(fix_round_scopes: &[FixRoundScope]) -> bool {
	let mut seen_files = BTreeSet::<PathBuf>::new();

	for (scope_files, _scope_options) in fix_round_scopes {
		for file in scope_files {
			if !seen_files.insert(file.clone()) {
				return false;
			}
		}
	}

	true
}

fn validate_pre_edit_semantic_baseline(
	files: &[PathBuf],
	cargo_options: &CargoOptions,
	scope_label: &str,
	verbose: bool,
	progress: bool,
) -> Result<BTreeSet<PathBuf>> {
	if progress {
		eprintln!("vstyle tune: [{scope_label}] validating the pre-edit semantic baseline.");
	}

	let (stdout, error_files) = semantic::collect_compiler_error_files_with_output(
		files,
		cargo_options,
		verbose,
		progress,
	)?;

	if semantic::semantic_check_succeeded(&stdout) != Some(true) {
		return Err(eyre::eyre!(
			"vstyle tune: [{scope_label}] pre-edit semantic validation failed; no fixes were written."
		));
	}

	Ok(error_files)
}

fn apply_semantic_validation(
	scope_label: &str,
	file_fixes: &mut FileFixApplication,
	cargo_options: &CargoOptions,
	baseline_error_files: &BTreeSet<PathBuf>,
	verbose: bool,
	progress: bool,
) -> Result<usize> {
	if progress {
		eprintln!("vstyle tune: [{scope_label}] validating the edited files.");
	}

	let semantic_started = Instant::now();
	let (post_edit_stdout, post_edit_error_files) =
		semantic::collect_compiler_error_files_with_output(
			&file_fixes.changed_files,
			cargo_options,
			verbose,
			progress,
		)?;
	let post_edit_succeeded = semantic::semantic_check_succeeded(&post_edit_stdout) == Some(true);
	let semantic_phase_start_snapshot = collect_file_snapshots(&file_fixes.changed_files);
	let semantic_applied = semantic::apply_semantic_fixes(
		&file_fixes.changed_files,
		cargo_options,
		Some(post_edit_stdout),
		verbose,
		progress,
	)?;

	file_fixes.total_applied += semantic_applied;

	if progress {
		eprintln!(
			"vstyle tune: [{scope_label}] semantic validation found {} post-edit error file(s) and applied {} import fix(es) ({}).",
			post_edit_error_files.len(),
			semantic_applied,
			format_duration(semantic_started.elapsed()),
		);
	}
	if semantic_applied > 0 {
		file_fixes.total_applied += apply_post_semantic_cleanup(
			&file_fixes.changed_files,
			&semantic_phase_start_snapshot,
			&mut file_fixes.import_fallbacks,
			&mut file_fixes.changed_originals,
			&mut file_fixes.let_mut_reorder_files,
		)?;
	}

	handle_semantic_validation_fallbacks(SemanticValidationFallbacks {
		files: &file_fixes.changed_files,
		semantic_cargo_options: cargo_options,
		baseline_error_files,
		post_error_files: (semantic_applied == 0).then_some(&post_edit_error_files),
		verbose,
		progress,
		import_fallbacks: &file_fixes.import_fallbacks,
		changed_originals: &file_fixes.changed_originals,
		let_mut_reorder_files: &file_fixes.let_mut_reorder_files,
		type_alias_rename_files: &file_fixes.type_alias_rename_files,
	})?;

	if semantic_applied > 0 || !post_edit_succeeded {
		let (final_stdout, _final_error_files) =
			semantic::collect_compiler_error_files_with_output(
				&file_fixes.changed_files,
				cargo_options,
				verbose,
				progress,
			)?;

		if semantic::semantic_check_succeeded(&final_stdout) != Some(true) {
			if recover_without_binding_rewrites(file_fixes, cargo_options, verbose, progress)? {
				return Ok(0);
			}

			return Err(eyre::eyre!(
				"vstyle tune: [{scope_label}] final semantic validation failed after fixer recovery."
			));
		}
	}

	Ok(semantic_applied)
}

fn recover_without_binding_rewrites(
	file_fixes: &mut FileFixApplication,
	cargo_options: &CargoOptions,
	verbose: bool,
	progress: bool,
) -> Result<bool> {
	let mut applied = 0;

	// A changed binding can break a different file. Restore the complete scope from
	// its pre-edit sources before retaining independent style edits.
	for (path, original) in file_fixes.changed_originals.values() {
		let (text, count, _, _) = apply_filtered_fix_passes(path, original, false, false)?;

		fs::write(path, text)?;

		applied += count;
	}

	let (stdout, _) = semantic::collect_compiler_error_files_with_output(
		&file_fixes.changed_files,
		cargo_options,
		verbose,
		progress,
	)?;
	let recovered = semantic::semantic_check_succeeded(&stdout) == Some(true);

	if recovered {
		file_fixes.total_applied = applied;

		eprintln!(
			"Retained {applied} non-binding fixes after restoring failed binding rewrites; remaining violations are still reported."
		);
	}

	Ok(recovered)
}

fn run_fix_round(
	files: &[PathBuf],
	cargo_options: &CargoOptions,
	verbose: bool,
	progress: bool,
) -> Result<FixRoundSummary> {
	let scope_label = fix_scope_label(cargo_options);

	if progress {
		eprintln!("vstyle tune: [{scope_label}] collecting fixes for {} file(s).", files.len());
	}

	let round_start_snapshot = collect_file_snapshots(files);
	let collected = collect_file_fixes(files, &scope_label, progress)?;

	if collected.changed_files.is_empty() {
		if progress {
			eprintln!(
				"vstyle tune: [{scope_label}] applied {} fix(es) this round.",
				collected.total_applied
			);
		}

		return Ok(FixRoundSummary::default());
	}

	let semantic_cargo_options =
		scoped_semantic_cargo_options(cargo_options, &collected.changed_files)?;
	let baseline_error_files = validate_pre_edit_semantic_baseline(
		&collected.changed_files,
		&semantic_cargo_options,
		&scope_label,
		verbose,
		progress,
	)?;
	let mut file_fixes = apply_collected_file_fixes(collected)?;
	let semantic_applied = apply_semantic_validation(
		&scope_label,
		&mut file_fixes,
		&semantic_cargo_options,
		&baseline_error_files,
		verbose,
		progress,
	)?;
	let net_changed_files = changed_files_since_snapshot(&round_start_snapshot);

	// Count this round as no-op when all edits are eventually rolled back.
	if file_fixes.total_applied > 0 && net_changed_files.is_empty() {
		return Ok(FixRoundSummary::default());
	}

	let requires_follow_up_round = if semantic_applied > 0 && !net_changed_files.is_empty() {
		has_fixable_violations_in_files(&net_changed_files)?
	} else {
		false
	};

	if progress {
		eprintln!(
			"vstyle tune: [{scope_label}] applied {} fix(es) this round across {} net changed file(s).",
			file_fixes.total_applied,
			net_changed_files.len(),
		);
	}

	Ok(FixRoundSummary {
		applied_count: file_fixes.total_applied,
		requires_follow_up_round,
		changed_files: net_changed_files,
	})
}

fn collect_file_fixes(
	files: &[PathBuf],
	scope_label: &str,
	progress: bool,
) -> Result<CollectedFileFixes> {
	let collect_started = Instant::now();
	let type_alias_plan = collect_type_alias_rename_plan(files)?;
	let outcomes_all = collect_fix_outcomes(files, &type_alias_plan, scope_label, progress)?;
	let changed_files = changed_file_paths(&outcomes_all);
	let total_applied = outcomes_all.iter().map(|outcome| outcome.applied_count).sum::<usize>();

	if progress {
		eprintln!(
			"vstyle tune: [{scope_label}] collected {} fix(es) across {} changed file(s) ({}).",
			total_applied,
			changed_files.len(),
			format_duration(collect_started.elapsed()),
		);
	}

	Ok(CollectedFileFixes { outcomes: outcomes_all, changed_files, total_applied })
}

fn apply_collected_file_fixes(collected: CollectedFileFixes) -> Result<FileFixApplication> {
	let mut application = FileFixApplication {
		changed_files: collected.changed_files,
		total_applied: collected.total_applied,
		import_fallbacks: BTreeMap::new(),
		changed_originals: BTreeMap::new(),
		let_mut_reorder_files: BTreeMap::new(),
		type_alias_rename_files: BTreeMap::new(),
	};

	for outcome in collected.outcomes {
		apply_file_fix_outcome(outcome, &mut application)?;
	}

	Ok(application)
}

fn apply_file_fix_outcome(
	outcome: FileFixOutcome,
	application: &mut FileFixApplication,
) -> Result<()> {
	if let Some(text) = outcome.rewritten_text {
		fs::write(&outcome.path, text)?;
	}
	if let Some(original_text) = outcome.original_text {
		application
			.changed_originals
			.insert(normalize_path(&outcome.path), (outcome.path.clone(), original_text));
	}
	if let Some(source_text) = outcome.import_fallback_source_text {
		let normalized = normalize_path(&outcome.path);

		application
			.import_fallbacks
			.insert(normalized, ImportFallbackPlan { path: outcome.path.clone(), source_text });
	}

	if outcome.had_let_mut_reorder_edits {
		application.let_mut_reorder_files.insert(normalize_path(&outcome.path), ());
	}
	if outcome.had_type_alias_rename_edits {
		application.type_alias_rename_files.insert(normalize_path(&outcome.path), ());
	}

	Ok(())
}

fn fix_scope_label(cargo_options: &CargoOptions) -> String {
	if !cargo_options.packages.is_empty() {
		return cargo_options.packages.join(",");
	}

	if cargo_options.workspace {
		return "workspace".to_owned();
	}

	"default".to_owned()
}

fn apply_post_semantic_cleanup(
	files: &[PathBuf],
	semantic_phase_start_snapshot: &BTreeMap<PathBuf, String>,
	import_fallbacks: &mut BTreeMap<PathBuf, ImportFallbackPlan>,
	changed_originals: &mut BTreeMap<PathBuf, (PathBuf, String)>,
	let_mut_reorder_files: &mut BTreeMap<PathBuf, ()>,
) -> Result<usize> {
	let mut applied_total = 0_usize;

	for path in files {
		let Some(before_semantic) = semantic_phase_start_snapshot.get(path) else {
			continue;
		};
		let Ok(after_semantic) = fs::read_to_string(path) else {
			continue;
		};

		if after_semantic == *before_semantic {
			continue;
		}

		let normalized = normalize_path(path);

		changed_originals
			.entry(normalized.clone())
			.or_insert_with(|| (path.clone(), before_semantic.clone()));

		let (rewritten, applied, had_import_shortening_edits, had_let_mut_reorder_edits) =
			apply_fix_passes(path, &after_semantic, true)?;

		if rewritten == after_semantic || applied == 0 {
			continue;
		}

		fs::write(path, &rewritten)?;

		applied_total += applied;

		if had_import_shortening_edits {
			import_fallbacks.insert(
				normalized.clone(),
				ImportFallbackPlan { path: path.clone(), source_text: after_semantic.clone() },
			);
		}
		if had_let_mut_reorder_edits {
			let_mut_reorder_files.insert(normalized, ());
		}
	}

	Ok(applied_total)
}

fn collect_file_snapshots(files: &[PathBuf]) -> BTreeMap<PathBuf, String> {
	let mut snapshots = BTreeMap::new();

	for path in files {
		if let Ok(text) = fs::read_to_string(path) {
			snapshots.insert(path.clone(), text);
		}
	}

	snapshots
}

fn restore_file_snapshots(snapshots: &BTreeMap<PathBuf, String>) -> Result<()> {
	for (path, original) in snapshots {
		if fs::read_to_string(path).is_ok_and(|current| current == *original) {
			continue;
		}

		fs::write(path, original)?;
	}

	Ok(())
}

fn changed_files_since_snapshot(snapshots: &BTreeMap<PathBuf, String>) -> Vec<PathBuf> {
	snapshots
		.iter()
		.filter_map(|(path, before)| {
			fs::read_to_string(path)
				.ok()
				.and_then(|after| (after != *before).then_some(path.clone()))
		})
		.collect::<Vec<_>>()
}

#[cfg(test)]
fn has_net_file_changes(snapshots: &BTreeMap<PathBuf, String>) -> bool {
	!changed_files_since_snapshot(snapshots).is_empty()
}

fn has_fixable_violations_in_files(files: &[PathBuf]) -> Result<bool> {
	for path in files {
		let Some(ctx) = shared::read_file_context(path)? else {
			continue;
		};
		let (violations, _edits) = collect_violations(&ctx, false);

		if violations.iter().any(|violation| violation.fixable) {
			return Ok(true);
		}
	}

	Ok(false)
}

fn handle_semantic_validation_fallbacks(ctx: SemanticValidationFallbacks<'_>) -> Result<()> {
	if ctx.import_fallbacks.is_empty() && ctx.changed_originals.is_empty() {
		return Ok(());
	}

	let computed_post_error_files;
	let post_error_files = if let Some(post_error_files) = ctx.post_error_files {
		post_error_files
	} else {
		computed_post_error_files = semantic::collect_compiler_error_files(
			ctx.files,
			ctx.semantic_cargo_options,
			ctx.verbose,
			ctx.progress,
		)?;

		&computed_post_error_files
	};
	let new_errors =
		post_error_files.difference(ctx.baseline_error_files).cloned().collect::<Vec<_>>();
	let mut handled = BTreeMap::<PathBuf, ()>::new();

	if !ctx.type_alias_rename_files.is_empty()
		&& new_errors.iter().any(|path| ctx.type_alias_rename_files.contains_key(path))
	{
		for normalized in ctx.type_alias_rename_files.keys() {
			if let Some((path, original_text)) = ctx.changed_originals.get(normalized) {
				fs::write(path, original_text)?;

				handled.insert(normalized.clone(), ());

				eprintln!(
					"Skipped RUST-STYLE-TYPE-001 rename in {} due to failed semantic validation.",
					path.display()
				);
			}
		}
	}

	for normalized in post_error_files.difference(ctx.baseline_error_files) {
		if handled.contains_key(normalized) {
			continue;
		}

		if let Some((path, original_text)) = ctx.changed_originals.get(normalized)
			&& ctx.let_mut_reorder_files.contains_key(normalized)
		{
			fs::write(path, original_text)?;

			handled.insert(normalized.clone(), ());

			eprintln!(
				"Skipped RUST-STYLE-LET-001 reorder in {} due to failed semantic validation.",
				path.display()
			);

			continue;
		}
		if let Some(import_fallback) = ctx.import_fallbacks.get(normalized) {
			let fallback =
				compute_import_fallback(&import_fallback.path, &import_fallback.source_text)?;
			let current_text = fs::read_to_string(&import_fallback.path).unwrap_or_default();

			if fallback == current_text {
				continue;
			}

			fs::write(&import_fallback.path, fallback)?;

			handled.insert(normalized.clone(), ());

			if ctx.verbose {
				eprintln!(
					"Applied import-shortening fallback in {} due to failed semantic validation.",
					import_fallback.path.display()
				);
			}
		}
	}
	for normalized in post_error_files.difference(ctx.baseline_error_files) {
		if handled.contains_key(normalized) {
			continue;
		}

		if let Some((path, original_text)) = ctx.changed_originals.get(normalized) {
			fs::write(path, original_text)?;

			if ctx.verbose {
				eprintln!("Reverted {} due to failed semantic validation.", path.display());
			}
			if ctx.let_mut_reorder_files.contains_key(normalized) {
				eprintln!(
					"Skipped RUST-STYLE-LET-001 reorder in {} due to failed semantic validation.",
					path.display()
				);
			}
		}
	}

	Ok(())
}

fn collect_type_alias_rename_plan(files: &[PathBuf]) -> Result<TypeAliasRenamePlan> {
	let mut plan = TypeAliasRenamePlan::default();

	for file in files {
		let Some(ctx) = shared::read_file_context(file)? else {
			continue;
		};
		let fixes = types::collect_type_alias_rename_fixes(&ctx);

		for fix in fixes {
			if let Some(existing) = plan.renames.get(&fix.alias) {
				if existing != &fix.target {
					continue;
				}
			} else {
				plan.renames.insert(fix.alias.clone(), fix.target);
			}

			plan.definition_edits.entry(file.clone()).or_default().extend(fix.definition_edits);
		}
	}

	Ok(plan)
}

fn apply_type_alias_rename_plan(
	path: &Path,
	initial_text: &str,
	plan: &TypeAliasRenamePlan,
) -> Result<(String, usize, bool)> {
	if plan.renames.is_empty() && plan.definition_edits.is_empty() {
		return Ok((initial_text.to_owned(), 0, false));
	}

	let mut text = initial_text.to_owned();
	let Some(ctx) = shared::read_file_context_from_text(path, mem::take(&mut text))? else {
		return Ok((text, 0, false));
	};
	let mut edits = Vec::<Edit>::new();

	if let Some(def_edits) = plan.definition_edits.get(path) {
		edits.extend(def_edits.iter().cloned());
	}

	let skip_ranges = edits.iter().map(|edit| (edit.start, edit.end)).collect::<Vec<_>>();
	let usage_edits = types::build_type_alias_usage_rename_edits(&ctx, &plan.renames, &skip_ranges);

	edits.extend(usage_edits);

	let mut text = ctx.text;
	let applied = fixes::apply_edits(&mut text, edits)?;

	Ok((text, applied, applied > 0))
}

fn collect_fix_outcomes(
	files: &[PathBuf],
	type_alias_plan: &TypeAliasRenamePlan,
	scope_label: &str,
	progress: bool,
) -> Result<Vec<FileFixOutcome>> {
	let batch_count = files.len().div_ceil(FILE_BATCH_SIZE);
	let mut outcomes_all = Vec::<FileFixOutcome>::new();
	let mut processed = 0_usize;

	for (batch_idx, batch) in files.chunks(FILE_BATCH_SIZE).enumerate() {
		let batch_started = Instant::now();
		let outcomes = batch
			.par_iter()
			.map(|file| -> Result<FileFixOutcome> {
				let original_text = match fs::read_to_string(file) {
					Ok(text) => text,
					Err(_) => {
						return Ok(FileFixOutcome {
							path: file.clone(),
							original_text: None,
							rewritten_text: None,
							import_fallback_source_text: None,
							had_let_mut_reorder_edits: false,
							had_type_alias_rename_edits: false,
							applied_count: 0,
						});
					},
				};
				let (base_text, type_alias_applied, had_type_alias_rename_edits) =
					apply_type_alias_rename_plan(file, &original_text, type_alias_plan)?;
				let (text, pass_applied, had_import_shortening_edits, had_let_mut_reorder_edits) =
					apply_fix_passes(file, &base_text, true)?;
				let did_change = text != original_text;
				let applied_count = if did_change { type_alias_applied + pass_applied } else { 0 };
				let import_fallback_source_text =
					(had_import_shortening_edits && did_change).then_some(base_text);

				Ok(FileFixOutcome {
					path: file.clone(),
					original_text: if did_change { Some(original_text) } else { None },
					rewritten_text: if did_change { Some(text) } else { None },
					import_fallback_source_text,
					had_let_mut_reorder_edits,
					had_type_alias_rename_edits,
					applied_count,
				})
			})
			.collect::<Vec<_>>();
		let mut batch_applied = 0_usize;
		let mut batch_changed = 0_usize;

		for outcome in outcomes {
			let outcome = outcome?;

			batch_applied += outcome.applied_count;

			if outcome.rewritten_text.is_some() {
				batch_changed += 1;
			}

			outcomes_all.push(outcome);
		}

		processed += batch.len();

		if progress && batch_count > 1 {
			eprintln!(
				"vstyle tune: [{scope_label}] fix scan batch {}/{} checked {}/{} file(s), changed {}, found {} fix(es) ({}).",
				batch_idx + 1,
				batch_count,
				processed,
				files.len(),
				batch_changed,
				batch_applied,
				format_duration(batch_started.elapsed()),
			);
		}
	}

	Ok(outcomes_all)
}

fn changed_file_paths(outcomes: &[FileFixOutcome]) -> Vec<PathBuf> {
	outcomes
		.iter()
		.filter(|outcome| outcome.rewritten_text.is_some())
		.map(|outcome| outcome.path.clone())
		.collect::<Vec<_>>()
}

fn scoped_semantic_cargo_options(
	cargo_options: &CargoOptions,
	changed_files: &[PathBuf],
) -> Result<CargoOptions> {
	let mut options = cargo_options.clone();

	if options.workspace
		&& options.packages.is_empty()
		&& !changed_files.is_empty()
		&& let Some(packages) = shared::package_names_for_files(changed_files)?
		&& packages.len() == 1
	{
		options.workspace = false;
		options.packages = packages;
	}

	Ok(options)
}

fn apply_fix_passes(
	path: &Path,
	initial_text: &str,
	with_import_shortening: bool,
) -> Result<(String, usize, bool, bool)> {
	apply_filtered_fix_passes(path, initial_text, with_import_shortening, true)
}

fn apply_filtered_fix_passes(
	path: &Path,
	initial_text: &str,
	with_import_shortening: bool,
	with_binding_rewrites: bool,
) -> Result<(String, usize, bool, bool)> {
	let mut text = initial_text.to_owned();
	let mut pass = 0_usize;
	let mut applied_count = 0_usize;
	let mut had_import_shortening_edits = false;
	let mut had_let_mut_reorder_edits = false;

	while pass < MAX_FIX_PASSES {
		pass += 1;

		let Some(ctx) = shared::read_file_context_from_text(path, mem::take(&mut text))? else {
			break;
		};
		let (_violations, mut edits) =
			collect_filtered_violations(&ctx, true, with_import_shortening, with_binding_rewrites);
		let mut current_text = ctx.text;

		if !with_binding_rewrites {
			edits.retain(|edit| {
				!edit.rule.starts_with("RUST-STYLE-IMPORT-")
					&& !matches!(edit.rule, "RUST-STYLE-LET-001" | "RUST-STYLE-MOD-007")
			});
		}
		if with_import_shortening {
			if edits.iter().any(|edit| is_import_shortening_rule(edit.rule)) {
				had_import_shortening_edits = true;
			}
		} else {
			edits.retain(|edit| !is_import_shortening_rule(edit.rule));
		}
		if edits.iter().any(|edit| edit.rule == "RUST-STYLE-LET-001") {
			had_let_mut_reorder_edits = true;
		}
		if edits.is_empty() {
			text = current_text;

			break;
		}

		let applied = fixes::apply_edits(&mut current_text, edits)?;

		if applied == 0 {
			text = current_text;

			break;
		}

		applied_count += applied;
		text = current_text;
	}

	Ok((text, applied_count, had_import_shortening_edits, had_let_mut_reorder_edits))
}

fn compute_import_fallback(path: &Path, source_text: &str) -> Result<String> {
	let (fallback, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
		apply_fix_passes(path, source_text, false)?;

	Ok(fallback)
}

fn collect_check_outcomes(files: &[PathBuf]) -> Result<Vec<FileCheckOutcome>> {
	let mut outcomes_all = Vec::<FileCheckOutcome>::new();

	for batch in files.chunks(FILE_BATCH_SIZE) {
		let outcomes = batch
			.par_iter()
			.map(|file| -> Result<FileCheckOutcome> {
				let violations = if shared::is_swift_file(file) {
					swift::collect_violations_from_file(file)?
				} else if let Some(ctx) = shared::read_file_context(file)? {
					let (found, _edits) = collect_violations(&ctx, false);

					found
				} else {
					Vec::new()
				};

				Ok(FileCheckOutcome { path: file.clone(), violations })
			})
			.collect::<Vec<_>>();

		for outcome in outcomes {
			outcomes_all.push(outcome?);
		}
	}

	Ok(outcomes_all)
}

fn collect_file_violation_counts(
	files: &[PathBuf],
) -> Result<BTreeMap<PathBuf, FileViolationCounts>> {
	let outcomes = collect_check_outcomes(files)?;
	let mut counts_by_file = BTreeMap::<PathBuf, FileViolationCounts>::new();

	for outcome in outcomes {
		counts_by_file.insert(
			outcome.path,
			FileViolationCounts {
				violation_count: outcome.violations.len(),
				unfixable_count: outcome
					.violations
					.iter()
					.filter(|violation| !violation.fixable)
					.count(),
			},
		);
	}

	Ok(counts_by_file)
}

fn refresh_check_state_for_files(files: &[PathBuf], check_state: &mut CheckState) -> Result<()> {
	if files.is_empty() {
		return Ok(());
	}

	let updated_counts = collect_file_violation_counts(files)?;

	check_state.apply_updates(updated_counts);

	Ok(())
}

fn is_import_shortening_rule(rule: &str) -> bool {
	matches!(rule, "RUST-STYLE-IMPORT-008" | "RUST-STYLE-IMPORT-009")
}

fn normalize_path(path: &Path) -> PathBuf {
	match fs::canonicalize(path) {
		Ok(canonical) => canonical,
		Err(_) => path.to_path_buf(),
	}
}

fn collect_violations(ctx: &FileContext, with_fixes: bool) -> (Vec<Violation>, Vec<Edit>) {
	collect_violations_with_import_shortening(ctx, with_fixes, true)
}

fn collect_violations_with_import_shortening(
	ctx: &FileContext,
	with_fixes: bool,
	with_import_shortening: bool,
) -> (Vec<Violation>, Vec<Edit>) {
	collect_filtered_violations(ctx, with_fixes, with_import_shortening, true)
}

fn collect_filtered_violations(
	ctx: &FileContext,
	with_fixes: bool,
	with_import_shortening: bool,
	with_binding_rewrites: bool,
) -> (Vec<Violation>, Vec<Edit>) {
	let mut violations = Vec::new();
	let mut edits = Vec::new();

	file::check_mod_rs(ctx, &mut violations);

	if with_binding_rewrites {
		file::check_error_rs_no_use(ctx, &mut violations, &mut edits, with_fixes);
		bindings::check_let_mut_reorder(ctx, &mut violations, &mut edits, with_fixes);
		test_modules::check_test_module_super_glob(ctx, &mut edits, with_fixes);
		imports::check_import_rules(
			ctx,
			&mut violations,
			&mut edits,
			with_fixes,
			with_import_shortening,
		);
	}

	generics::check_unnecessary_turbofish(ctx, &mut violations, &mut edits, with_fixes);
	generics::check_turbofish_canonicalization(ctx, &mut violations, &mut edits, with_fixes);
	types::check_type_alias_renames(ctx, &mut violations);
	module::check_module_order(ctx, &mut violations, &mut edits, with_fixes);
	impls::check_impl_adjacency(ctx, &mut violations, &mut edits, with_fixes);
	impls::check_impl_rules(ctx, &mut violations, &mut edits, with_fixes);
	impls::check_inline_trait_bounds(ctx, &mut violations);
	quality::check_logging_quality(ctx, &mut violations);
	quality::check_expect_unwrap(ctx, &mut violations, &mut edits, with_fixes);
	quality::check_numeric_literals(ctx, &mut violations, &mut edits, with_fixes);
	quality::check_function_length(ctx, &mut violations);
	spacing::check_vertical_spacing(ctx, &mut violations, &mut edits, with_fixes);
	quality::check_test_rules(ctx, &mut violations);

	(violations, edits)
}

#[cfg(test)]
fn violation_signature(violation: &Violation) -> (usize, &'static str, &str, bool) {
	(violation.line, violation.rule, violation.message.as_str(), violation.fixable)
}

#[cfg(test)]
mod tests {
	use std::{
		collections::{BTreeMap, BTreeSet},
		env, fs,
		path::{Path, PathBuf},
		process, slice,
	};

	use ra_ap_syntax::{AstNode as _, ast::Use};

	use crate::style::{
		self, Edit, MAX_FIX_PASSES, fixes, imports, module, semantic, shared, types,
		violation_signature,
	};

	#[test]
	fn suffix_rewrite_works() {
		let text = "let x = 10f32;\n";
		let ctx = shared::read_file_context_from_text(Path::new("a.rs"), text.to_owned())
			.expect("context")
			.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.is_empty());
		assert!(edits.iter().any(|edit| edit.rule == "RUST-STYLE-NUM-001"));
	}

	#[test]
	fn runtime_reports_unwrap_without_autofix() {
		let original = "fn demo_case(value: Option<usize>) -> usize {\n\tvalue.unwrap()\n}\n";
		let ctx = shared::read_file_context_from_text(
			Path::new("runtime_unwrap_check.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(
			violations
				.iter()
				.any(|violation| violation.rule == "RUST-STYLE-RUNTIME-001" && !violation.fixable)
		);
		assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-RUNTIME-001"));
	}

	#[test]
	fn runtime_does_not_enforce_expect_sentence_style() {
		let original =
			"fn demo_case(value: Option<usize>) -> usize {\n\tvalue.expect(\"missing value\")\n}\n";
		let ctx = shared::read_file_context_from_text(
			Path::new("runtime_expect_fix.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|violation| violation.rule == "RUST-STYLE-RUNTIME-002"));
		assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-RUNTIME-002"));
	}

	#[test]
	fn runtime_reports_expect_empty_message_without_autofix() {
		let original = "fn demo_case(value: Option<usize>) -> usize {\n\tvalue.expect(\"\")\n}\n";
		let ctx = shared::read_file_context_from_text(
			Path::new("runtime_expect_empty_message.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|violation| {
			violation.rule == "RUST-STYLE-RUNTIME-002"
				&& violation.message == "expect() message must not be empty."
				&& !violation.fixable
		}));
		assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-RUNTIME-002"));
	}

	#[test]
	fn log_rule_does_not_enforce_sentence_style() {
		let text = "fn run() {\n\ttracing::info!(\"missing value\")\n}\n";
		let ctx = shared::read_file_context_from_text(
			Path::new("log_sentence_style.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, _edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|violation| {
			violation.rule == "RUST-STYLE-LOG-002"
				&& violation.message
					== "Log messages should be complete sentences with capitalization and punctuation."
		}));
	}

	#[test]
	fn runtime_rules_ignore_cfg_test_module_calls() {
		let text = "#[cfg(test)]\nmod tests {\n\t#[test]\n\tfn sample_case() {\n\t\tlet value = Some(1usize);\n\t\tlet _ = value.unwrap();\n\t}\n}\n";
		let ctx =
			shared::read_file_context_from_text(Path::new("runtime_cfg_test.rs"), text.to_owned())
				.expect("context")
				.expect("has ctx");
		let (violations, _edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().all(|violation| !matches!(
			violation.rule,
			"RUST-STYLE-RUNTIME-001" | "RUST-STYLE-RUNTIME-002"
		)));
	}

	#[test]
	fn runtime_rules_classify_standalone_test_files_without_matching_production_names() {
		let text = "fn fixture(value: Option<usize>) { let _ = value.unwrap(); let _ = value.expect(\"\"); }";

		for (path, test_file) in [
			("tests.rs", true),
			("src/agent_prompt_edit/tests.rs", true),
			(r"src\agent_prompt_edit\tests.rs", true),
			("src/tests/helper.rs", true),
			("src/journal_test.rs", true),
			("src/journal_tests.rs", true),
			("src/journal_tests_support.rs", false),
			("src/contests.rs", false),
			("src/tests_support.rs", false),
			("src/tests.rs/helper.rs", false),
			("src/production.rs", false),
		] {
			let ctx = shared::read_file_context_from_text(Path::new(path), text.to_owned())
				.expect("context")
				.expect("has ctx");
			let (violations, edits) = style::collect_violations(&ctx, true);

			for rule in ["RUST-STYLE-RUNTIME-001", "RUST-STYLE-RUNTIME-002"] {
				assert_eq!(
					violations.iter().any(|violation| violation.rule == rule),
					!test_file,
					"{path}: {rule}"
				);
				assert!(!edits.iter().any(|edit| edit.rule == rule));
			}
		}
	}

	#[test]
	fn runtime_rules_follow_external_test_module_owners() {
		let now = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.expect("Timestamp.")
			.as_nanos();
		let root = env::temp_dir().join(format!("vstyle-test-owner-{}-{now}", process::id()));
		let source = root.join("src");
		let helper = source.join("support.rs");
		let text = "pub fn fixture(value: Option<usize>) { let _ = value.unwrap(); let _ = value.expect(\"\"); }";

		fs::create_dir_all(&source).expect("Create fixture.");
		fs::write(
			root.join("Cargo.toml"),
			"[package]\nname = \"test-owner-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
		)
		.expect("Write manifest.");
		fs::write(&helper, text).expect("Write helper.");
		fs::create_dir_all(source.join("nested")).expect("Create nested directory.");
		fs::write(source.join("bridge.rs"), "#[path = \"support.rs\"] mod fixture;")
			.expect("Write intermediate owner.");

		for (owner, production) in [
			("#[cfg(test)]\n#[path = \"support.rs\"] mod fixture;", false),
			("#[cfg(test)] mod support;", false),
			("#[cfg(test)] mod bridge;", false),
			("#[cfg(test)] mod nested { #[path = \"../support.rs\"] mod fixture; }", false),
			(
				"#[cfg(test)]\n#[path = \"support.rs\"] mod fixture;\n#[path = \"support.rs\"] mod production;",
				true,
			),
			("#[cfg(not(test))] mod support;", true),
			("#[cfg(any(test, feature = \"extra\"))] mod support;", true),
		] {
			fs::write(source.join("lib.rs"), owner).expect("Write owner.");

			let ctx = shared::read_file_context_from_text(&helper, text.to_owned())
				.expect("Read helper.")
				.expect("Get context.");
			let (violations, _) = style::collect_violations(&ctx, false);

			for rule in ["RUST-STYLE-RUNTIME-001", "RUST-STYLE-RUNTIME-002"] {
				assert_eq!(
					violations.iter().any(|v| v.rule == rule),
					production,
					"{owner}: {rule}"
				);
			}
		}

		fs::remove_dir_all(root).expect("Remove fixture.");
	}

	#[test]
	fn style_fixes_preserve_serde_default_contracts() {
		// Option fields can still need defaults for sequence input, custom defaults,
		// and custom deserializers. Their type alone cannot prove redundancy.
		for attribute in [
			"#[serde(default)]",
			r#"#[serde(default, rename = "value")]"#,
			r#"#[serde(default = "default_value")]"#,
			r#"#[serde(default, deserialize_with = "read_value")]"#,
		] {
			for field_type in ["Option<String>", "std::option::Option<String>", "String"] {
				let original = format!(
					"#[derive(Deserialize)]\nstruct Payload {{\n\t{attribute}\n\tvalue: {field_type},\n}}\n"
				);
				let ctx = shared::read_file_context_from_text(
					Path::new("serde_defaults.rs"),
					original.clone(),
				)
				.expect("context")
				.expect("has ctx");
				let (violations, edits) = style::collect_violations(&ctx, true);
				let mut rewritten = original;

				assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-SERDE-001"));

				fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

				assert!(rewritten.contains(attribute), "{rewritten}");
			}
		}
	}

	#[test]
	fn style_fixes_preserve_serde_defaults_in_macro_input() {
		let original = r#"
define_payload! {
	struct Payload {
		#[serde(default)]
		value: Option<String>,
		#[serde(default = "defaults::fallback")]
		fallback: Option<String>,
	}
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("serde_macro_defaults.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);
		let mut rewritten = original.to_owned();

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-SERDE-001"));

		fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(rewritten.contains("#[serde(default)]"));
		assert!(rewritten.contains(r#"#[serde(default = "defaults::fallback")]"#));
	}

	#[test]
	fn import005_fix_rewrites_error_rs_to_fully_qualified_paths() {
		let original = r#"
use std::fmt::{Display, Formatter};
use tonic::Status;

#[derive(Debug)]
pub enum ApiError {
	Internal,
}

impl Display for ApiError {
	fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
		write!(f, "boom")
	}
}

pub fn to_status() -> Status {
	Status::internal("boom")
}
"#;
		let ctx = shared::read_file_context_from_text(Path::new("error.rs"), original.to_owned())
			.expect("context")
			.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-005" && v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-005"));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 3);
		assert!(!rewritten.contains("\nuse std::fmt::{Display, Formatter};"));
		assert!(!rewritten.contains("\nuse tonic::Status;"));
		assert!(rewritten.contains("impl std::fmt::Display for ApiError"));
		assert!(rewritten.contains("f: &mut std::fmt::Formatter"));
		assert!(rewritten.contains("pub fn to_status() -> tonic::Status"));
		assert!(rewritten.contains("tonic::Status::internal(\"boom\")"));
	}

	#[test]
	fn import005_in_error_rs_is_not_fixable_when_symbol_is_ambiguous() {
		let text = r#"
use a::A;
use b::A;

pub enum Error {
	Value(A),
}
"#;
		let ctx = shared::read_file_context_from_text(Path::new("error.rs"), text.to_owned())
			.expect("context")
			.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-005" && !v.fixable));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-005"));
	}

	#[test]
	fn import007_reports_non_fixable_glob_without_safe_expansion() {
		let original = r#"
use crate::prelude::*;
use sqlx::*;
use std::collections::HashMap;

fn run() {
	let _ = HashMap::<u8, u8>::new();
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import010_glob.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-007" && !v.fixable));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-007"));

		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(rewritten.contains("use crate::prelude::*;"));
		assert!(rewritten.contains("use sqlx::*;"));
		assert!(rewritten.contains("use std::collections::HashMap;"));
	}

	#[test]
	fn import007_fixes_super_glob_when_used_symbols_can_be_resolved() {
		let original = r#"
fn helper() {}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn sample_case() {
		helper();
	}
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import010_nested_glob.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-007" && v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-007"));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-MOD-007"));

		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(!rewritten.contains("use super::*;"));
		assert!(rewritten.contains("use super::{"));
		assert!(rewritten.contains("helper"));
	}

	#[test]
	fn import007_super_glob_fix_ignores_symbol_names_only_mentioned_in_strings() {
		let original = r#"
pub struct ChunkingConfig {
	pub max_tokens: u32,
	pub overlap_tokens: u32,
}
pub struct Tokenizer;

pub fn load_tokenizer(_name: &str) -> Result<Tokenizer, String> {
	Ok(Tokenizer)
}
pub fn split_text(_text: &str, _cfg: &ChunkingConfig, _tokenizer: &Tokenizer) -> Vec<String> {
	Vec::new()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn splits_into_chunks_with_overlap() {
		let cfg = ChunkingConfig { max_tokens: 10, overlap_tokens: 2 };
		let tokenizer = load_tokenizer("demo").expect("Tokenizer loading should succeed.");
		let chunks = split_text("One. Two. Three. Four.", &cfg, &tokenizer);

		assert!(!chunks.is_empty() || chunks.is_empty());
	}
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import007_super_glob_string_symbol.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (_violations, _edits) = style::collect_violations(&ctx, true);
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import007_super_glob_string_symbol.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(!rewritten.contains("use super::*;"));
		assert!(!rewritten.contains("use super::{"));
	}

	#[test]
	fn import007_fix_expands_crate_prelude_glob_when_module_exports_are_known() {
		let now = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.expect("Read timestamp.")
			.as_nanos();
		let root = env::temp_dir().join(format!("vstyle-prelude-expand-{}-{now}", process::id()));
		let src_dir = root.join("src");
		let sample_path = src_dir.join("sample.rs");
		let original = "use crate::prelude::*;\n\nfn run() -> Result<()> {\n\tOk(())\n}\n";

		fs::create_dir_all(&src_dir).expect("Create temp src directory.");
		fs::write(
			root.join("Cargo.toml"),
			"[package]\nname = \"vstyle-prelude-expand\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
		)
		.expect("Write Cargo manifest.");
		fs::write(
			root.join("src/main.rs"),
			"mod prelude {\n\tpub use color_eyre::{Result, eyre};\n}\n",
		)
		.expect("Write crate root.");
		fs::write(&sample_path, original).expect("Write sample source.");

		let ctx =
			shared::read_file_context(&sample_path).expect("Read context.").expect("Have context.");
		let (_violations, edits) = style::collect_violations(&ctx, true);
		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("Apply edits.");

		assert!(rewritten.contains("use crate::prelude::{Result};"));
		assert!(!rewritten.contains("use crate::prelude::*;"));
	}

	#[test]
	fn import007_fix_expands_crate_prelude_glob_inside_braced_use() {
		let now = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.expect("Read timestamp.")
			.as_nanos();
		let root = env::temp_dir().join(format!("vstyle-prelude-braced-{}-{now}", process::id()));
		let src_dir = root.join("src");
		let sample_path = src_dir.join("sample.rs");
		let original = "use crate::{\n\tprelude::*,\n\tstyle::RunSummary,\n};\n\nfn run(summary: RunSummary) -> Result<()> {\n\tlet _ = summary;\n\tOk(())\n}\n";

		fs::create_dir_all(&src_dir).expect("Create temp src directory.");
		fs::write(
			root.join("Cargo.toml"),
			"[package]\nname = \"vstyle-prelude-braced\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
		)
		.expect("Write Cargo manifest.");
		fs::write(
			root.join("src/main.rs"),
			"mod prelude {\n\tpub use color_eyre::{Result, eyre};\n}\nmod style { pub struct RunSummary; }\n",
		)
		.expect("Write crate root.");
		fs::write(&sample_path, original).expect("Write sample source.");

		let ctx =
			shared::read_file_context(&sample_path).expect("Read context.").expect("Have context.");
		let (_violations, edits) = style::collect_violations(&ctx, true);
		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("Apply edits.");
		let compact = rewritten.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();

		assert!(compact.contains("usecrate::{"));
		assert!(compact.contains("prelude::{Result}"));
		assert!(compact.contains("style::RunSummary"));
		assert!(!compact.contains("prelude::*"));
	}

	#[test]
	fn import007_fix_preserves_cfg_attribute_when_expanding_pub_glob() {
		let original = r#"
pub mod api_code {
	#[cfg(feature = "pubfi")]
	mod pubfi {
		pub const ERR_A: i16 = -1;
		pub const ERR_B: i16 = -2;
	}

	#[cfg(feature = "pubfi")]
	pub use self::pubfi::*;
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import007_preserves_cfg_pub_glob.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-007" && v.fixable));

		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(rewritten.contains("#[cfg(feature = \"pubfi\")]"));
		assert!(rewritten.contains("pub use self::pubfi::{ERR_A, ERR_B};"));
	}

	#[test]
	fn import007_fix_preserves_inline_cfg_attribute_when_expanding_pub_glob() {
		let original = r#"
pub mod api_code {
	#[cfg(feature = "pubfi")] mod pubfi {
		pub const ERR_A: i16 = -1;
		pub const ERR_B: i16 = -2;
	}

	#[cfg(feature = "pubfi")] pub use self::pubfi::*;
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import007_preserves_inline_cfg_pub_glob.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-007" && v.fixable));

		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(
			rewritten.contains("#[cfg(feature = \"pubfi\")] pub use self::pubfi::{ERR_A, ERR_B};")
		);
	}

	#[test]
	fn import007_fix_expands_long_pub_glob_without_custom_layout() {
		let original = r#"
pub mod api_code {
	#[cfg(feature = "pubfi")]
	mod pubfi {
		pub const ERR_000: i16 = -1;
		pub const ERR_001: i16 = -2;
		pub const ERR_002: i16 = -3;
		pub const ERR_003: i16 = -4;
		pub const ERR_004: i16 = -5;
		pub const ERR_005: i16 = -6;
		pub const ERR_006: i16 = -7;
		pub const ERR_007: i16 = -8;
		pub const ERR_008: i16 = -9;
		pub const ERR_009: i16 = -10;
		pub const ERR_010: i16 = -11;
		pub const ERR_011: i16 = -12;
	}

	#[cfg(feature = "pubfi")]
	pub use self::pubfi::*;
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import007_long_pub_glob_multiline.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-007" && v.fixable));

		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(rewritten.contains("pub use self::pubfi::{ERR_000"));
		assert!(rewritten.contains("ERR_011};"));
		assert!(!rewritten.contains("pub use self::pubfi::*;"));
	}

	#[test]
	fn import007_fix_rewrites_rayon_prelude_glob_to_traits() {
		let original = "use rayon::prelude::*;\n\nfn sample(batch: &[usize]) {\n\tlet _ = batch.par_iter().map(|value| value + 1).count();\n}\n";
		let ctx = shared::read_file_context_from_text(
			Path::new("import007_rayon.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (_violations, edits) = style::collect_violations(&ctx, true);
		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(!rewritten.contains("use rayon::prelude::*;"));
		assert!(rewritten.contains("use rayon::iter::{"));
		assert!(rewritten.contains("IntoParallelRefIterator"));
		assert!(rewritten.contains("ParallelIterator"));
	}

	#[test]
	fn import010_fix_rewrites_top_level_super_import_to_crate_absolute() {
		let original = "use super::shared::Edit;\n\nfn run(edit: Edit) {\n\tlet _ = edit;\n}\n";
		let ctx = shared::read_file_context_from_text(
			Path::new("src/style/imports.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-010" && v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-010"));

		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(rewritten.contains("use crate::style::shared::Edit;"));
		assert!(!rewritten.contains("use super::shared::Edit;"));
	}

	#[test]
	fn import004_preserves_bindings_used_by_external_children() {
		let now = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.expect("Timestamp.")
			.as_nanos();
		let root = env::temp_dir().join(format!("vstyle-child-import-{}-{now}", process::id()));
		let src = root.join("src");
		let parent = src.join("lib.rs");
		let original = "use std::fs::read;\n#[path = \"consumer.rs\"] mod child;\npub fn run() { let _ = read(\"fixture\"); }\n";

		fs::create_dir_all(&src).expect("Create source directory.");
		fs::write(
			root.join("Cargo.toml"),
			"[package]\nname = \"child-import-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
		)
		.expect("Write manifest.");
		fs::write(&parent, original).expect("Write parent.");

		for child in [
			"use super::*; pub fn run() { let _ = read(\"fixture\"); }",
			"use super::{self, *}; pub fn run() { let _ = read(\"fixture\"); }",
			"use super::read as load; pub fn run() { let _ = load(\"fixture\"); }",
			"pub fn run() { let _ = super::read(\"fixture\"); }",
			"use crate::read as load; pub fn run() { let _ = load(\"fixture\"); }",
			"mod nested { pub fn run() { let _ = super::super::read(\"fixture\"); } }",
		] {
			fs::write(src.join("consumer.rs"), child).expect("Write child.");

			let ctx =
				shared::read_file_context(&parent).expect("Read context.").expect("Have context.");
			let (violations, edits) = style::collect_violations(&ctx, true);

			assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-004"), "{child}");
			assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-004" && !v.fixable));
		}

		fs::write(src.join("consumer.rs"), "pub fn run() { let _ = std::fs::read(\"fixture\"); }")
			.expect("Write independent child.");

		let ctx =
			shared::read_file_context(&parent).expect("Read context.").expect("Have context.");
		let (_, edits) = style::collect_violations(&ctx, true);

		assert!(
			edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-004"),
			"Independent child must not block qualification."
		);

		fs::remove_dir_all(root).expect("Remove fixture.");
	}

	#[test]
	fn import003_preserves_aliases_used_by_external_children() {
		let now = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.expect("Timestamp.")
			.as_nanos();
		let root = env::temp_dir().join(format!("vstyle-child-alias-{}-{now}", process::id()));
		let src = root.join("src");
		let parent = src.join("lib.rs");

		fs::create_dir_all(&src).expect("Create source directory.");
		fs::write(
			root.join("Cargo.toml"),
			"[package]\nname = \"child-alias-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
		)
		.expect("Write manifest.");
		fs::write(
			&parent,
			"use std::io::Error as State;\nmod child;\npub fn run() -> Option<State> { None }\n",
		)
		.expect("Write parent.");

		for child in [
			"use super::{self, *}; pub fn run() -> Option<State> { None }",
			"pub fn run() -> Option<super::State> { None }",
		] {
			fs::write(src.join("child.rs"), child).expect("Write child.");

			let ctx =
				shared::read_file_context(&parent).expect("Read context.").expect("Have context.");
			let (violations, edits) = style::collect_violations(&ctx, true);

			assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-003"), "{child}");
			assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-003" && !v.fixable));
		}

		fs::write(src.join("child.rs"), "pub fn run() -> Option<std::io::Error> { None }")
			.expect("Write independent child.");

		let ctx =
			shared::read_file_context(&parent).expect("Read context.").expect("Have context.");
		let (_, edits) = style::collect_violations(&ctx, true);

		assert!(edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-003"));

		fs::remove_dir_all(root).expect("Remove fixture.");
	}

	#[test]
	fn import007_preserves_parent_glob_with_inherited_symbols() {
		let text = "use std::io::*;\nstruct Marker;\n#[cfg(test)] mod tests { use super::*; fn run() { let _: Option<Error> = None; let _ = Marker; } }";
		let ctx =
			shared::read_file_context_from_text(Path::new("inherited_glob.rs"), text.to_owned())
				.expect("Read context.")
				.expect("Have context.");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-007"));
		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-007" && !v.fixable));
	}

	#[test]
	fn import010_resolves_binary_entrypoint_as_crate_root() {
		let now = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.expect("Timestamp.")
			.as_nanos();
		let root = env::temp_dir().join(format!("vstyle-bin-owner-{}-{now}", process::id()));
		let bin = root.join("src/bin");

		fs::create_dir_all(&bin).expect("Create binary directory.");
		fs::write(
			root.join("Cargo.toml"),
			"[package]\nname = \"bin-owner-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
		)
		.expect("Write manifest.");

		let path = bin.join("tool.rs");
		let original = "const VALUE: u32 = 1;\nfn main() {}\n#[cfg(test)] mod tests { use super::VALUE; fn value() -> u32 { VALUE } }";

		fs::write(&path, original).expect("Write binary.");

		let ctx = shared::read_file_context(&path).expect("Read context.").expect("Have context.");
		let (_, edits) = style::collect_violations(&ctx, true);
		let edits = edits.into_iter().filter(|edit| edit.rule == "RUST-STYLE-IMPORT-010").collect();
		let mut rewritten = original.to_owned();

		fixes::apply_edits(&mut rewritten, edits).expect("Rewrite parent path.");

		assert!(rewritten.contains("use crate::VALUE;"), "{rewritten}");

		fs::remove_dir_all(root).expect("Remove fixture.");
	}

	#[test]
	fn import010_respects_path_attribute_module_ownership() {
		let now = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.expect("Read timestamp.")
			.as_nanos();
		let root = env::temp_dir().join(format!("vstyle-path-module-{}-{now}", process::id()));
		let src = root.join("src");
		let child = src.join("migration_tests.rs");
		let original = "use super::APPLICATION_ID;\nfn fixture() -> u32 { APPLICATION_ID }\n";

		fs::create_dir_all(&src).expect("Create source directory.");
		fs::write(
			root.join("Cargo.toml"),
			"[package]\nname = \"path-module-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
		)
		.expect("Write manifest.");
		fs::write(src.join("lib.rs"), "mod migrations;\n").expect("Write root.");
		fs::write(
			src.join("migrations.rs"),
			"const APPLICATION_ID: u32 = 1;\n#[cfg(test)]\n#[path = \"migration_tests.rs\"]\nmod tests;\n",
		)
		.expect("Write parent.");
		fs::write(&child, original).expect("Write child.");

		let ctx = shared::read_file_context(&child).expect("Read context.").expect("Have context.");
		let (_, edits) = style::collect_violations(&ctx, true);
		let edits = edits.into_iter().filter(|edit| edit.rule == "RUST-STYLE-IMPORT-010").collect();
		let mut rewritten = original.to_owned();

		fixes::apply_edits(&mut rewritten, edits).expect("Apply path rewrite.");

		assert!(rewritten.contains("use crate::migrations::APPLICATION_ID;"), "{rewritten}");

		fs::write(
			src.join("lib.rs"),
			"mod migrations;\n#[path = \"migration_tests.rs\"]\nmod alternate;\n",
		)
		.expect("Write ambiguous ownership.");

		let ctx =
			shared::read_file_context(&child).expect("Refresh context.").expect("Have context.");
		let (_, edits) = style::collect_violations(&ctx, true);

		assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-010"));

		fs::remove_dir_all(root).expect("Remove fixture.");
	}

	#[test]
	fn import010_preserves_aliases_and_nested_parent_paths() {
		for (original, expected) in [
			("use super::{Kind as K};", "Kind as K"),
			("use super :: Kind as K;", "crate::parent::Kind as K"),
			("use super::{super::Root, Kind};", "crate::Root"),
		] {
			let ctx = shared::read_file_context_from_text(
				Path::new("src/parent/child.rs"),
				original.to_owned(),
			)
			.expect("Read context.")
			.expect("Have context.");
			let (_, edits) = style::collect_violations(&ctx, true);
			let edits =
				edits.into_iter().filter(|edit| edit.rule == "RUST-STYLE-IMPORT-010").collect();
			let mut rewritten = original.to_owned();

			fixes::apply_edits(&mut rewritten, edits).expect("Apply import rewrite.");

			assert!(rewritten.contains(expected), "{rewritten}");
			assert!(!rewritten.contains("::super"), "{rewritten}");
		}
	}

	#[test]
	fn import010_does_not_report_self_prefix_use() {
		let original = r#"
pub mod api_code {
	#[cfg(feature = "pubfi")]
	pub use self::pubfi::{ERR_A, ERR_B};
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import010_self_prefix.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-010"));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-010"));

		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert_eq!(rewritten.trim_start_matches('\n'), original.trim_start_matches('\n'));
	}

	#[test]
	fn fix_passes_rewrite_cfg_pub_use_glob_and_keep_macro_block_order() {
		let original = r#"
#[cfg(feature = "pubfi")]
mod pubfi {
	def_api_codes! {
		ERR_A = -1,
		ERR_B = -2,
	}
}

def_api_codes! {
	OUTSIDE_A = -3,
}

#[cfg(feature = "pubfi")]
pub use self::pubfi::*;

def_api_codes! {
	OUTSIDE_B = -4,
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(Path::new("import007_mod001_macro_block.rs"), original, true)
				.expect("apply fix passes");
		let compact = rewritten.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
		let use_pos =
			compact.find("pubuseself::pubfi::{ERR_A,ERR_B};").expect("has rewritten pub use");
		let first_macro_pos = compact.find("OUTSIDE_A=-3").expect("has first macro call");
		let second_macro_pos = compact.find("OUTSIDE_B=-4").expect("has second macro call");

		assert!(rewritten.contains("pub use self::pubfi::{ERR_A, ERR_B};"));
		assert!(rewritten.contains("#[cfg(feature = \"pubfi\")]"));
		assert!(use_pos < first_macro_pos);
		assert!(first_macro_pos < second_macro_pos);
	}

	#[test]
	fn fix_passes_rewrite_cfg_pub_use_self_path_and_keep_macro_block_order() {
		let original = r#"
def_api_codes! {
	OUTSIDE_A = -3,
}

#[cfg(feature = "pubfi")]
pub use self::pubfi::{ERR_A, ERR_B};

def_api_codes! {
	OUTSIDE_B = -4,
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(Path::new("import010_mod001_macro_block.rs"), original, true)
				.expect("apply fix passes");
		let compact = rewritten.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
		let use_pos =
			compact.find("pubuseself::pubfi::{ERR_A,ERR_B};").expect("has rewritten pub use");
		let first_macro_pos = compact.find("OUTSIDE_A=-3").expect("has first macro call");
		let second_macro_pos = compact.find("OUTSIDE_B=-4").expect("has second macro call");

		assert!(rewritten.contains("pub use self::pubfi::{ERR_A, ERR_B};"));
		assert!(rewritten.contains("#[cfg(feature = \"pubfi\")]"));
		assert!(use_pos < first_macro_pos);
		assert!(first_macro_pos < second_macro_pos);
	}

	#[test]
	fn import010_fix_rewrites_nested_super_chain_to_crate_absolute() {
		let original = r#"
mod inner {
	use super::super::bar;

	fn run(item: bar::Baz) {
		let _ = item;
	}
}
"#;
		let ctx =
			shared::read_file_context_from_text(Path::new("src/style/foo.rs"), original.to_owned())
				.expect("context")
				.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-010" && v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-010"));

		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(rewritten.contains("use crate::style::bar;"));
		assert!(!rewritten.contains("use super::super::bar;"));
	}

	#[test]
	fn import010_reports_non_fixable_when_super_depth_exceeds_module_depth() {
		let text = "use super::shared::Edit;\n";
		let ctx = shared::read_file_context_from_text(Path::new("src/lib.rs"), text.to_owned())
			.expect("context")
			.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-010" && !v.fixable));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-010"));
	}

	#[test]
	fn does_not_apply_edits_inside_string_literals() {
		let mut text = r#"let spec = "ISO-8601 2025";"#.to_owned();
		let start = text.find("8601").expect("match");
		let end = start + "8601".len();
		let edits = vec![Edit { start, end, replacement: "8_601".to_owned(), rule: "TEST" }];
		let applied = fixes::apply_edits(&mut text, edits).expect("apply edits");

		assert_eq!(applied, 0);
		assert_eq!(text, r#"let spec = "ISO-8601 2025";"#);
	}

	#[test]
	fn applies_edits_after_lifetime_annotations() {
		let mut text = "fn f(index: &TextIndex<'_>) { let mut consumed = 0usize; }\n".to_owned();
		let start = text.find("usize").expect("usize");
		let edits = vec![Edit { start, end: start, replacement: "_".to_owned(), rule: "TEST" }];
		let applied = fixes::apply_edits(&mut text, edits).expect("apply edits");

		assert_eq!(applied, 1);
		assert!(text.contains("0_usize"));
	}

	#[test]
	fn check_and_fix_collect_same_violations() {
		let text = r#"
use crate::prelude::*;
use crate::foo::bar;

fn example() {
	let x = 10f32;
	let y = 10000;
	let spec = "ISO-8601, 2025";
	let _ = std::format!("{x}");
	println!("{spec}");
}
"#;
		let ctx = shared::read_file_context_from_text(Path::new("c.rs"), text.to_owned())
			.expect("context")
			.expect("has ctx");
		let (check_violations, _check_edits) = style::collect_violations(&ctx, false);
		let (fix_violations, _fix_edits) = style::collect_violations(&ctx, true);
		let mut check_set = check_violations.iter().map(violation_signature).collect::<Vec<_>>();
		let mut fix_set = fix_violations.iter().map(violation_signature).collect::<Vec<_>>();

		check_set.sort();
		fix_set.sort();

		assert_eq!(check_set, fix_set);
	}

	#[test]
	fn impl_fix_does_not_break_foreign_usage_paths() {
		let original = r#"
impl Usage {
	pub fn from_rig(local: Usage, usage: rig::completion::Usage) -> Usage {
		let _ = usage;
		local
	}
}
"#;
		let ctx = shared::read_file_context_from_text(Path::new("impl.rs"), original.to_owned())
			.expect("context")
			.expect("has ctx");
		let (_violations, edits) = style::collect_violations(&ctx, true);
		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied > 0);
		assert!(rewritten.contains("local: Self"));
		assert!(rewritten.contains("usage: rig::completion::Usage"));
		assert!(rewritten.contains("-> Self"));
		assert!(!rewritten.contains(":: Self"));
	}

	#[test]
	fn impl_fix_skips_trait_impl_signatures() {
		let original = r#"
struct UserData;
mod grpc {
	pub struct UserData;
}
impl From<UserData> for grpc::UserData {
	fn from(user: UserData) -> grpc::UserData {
		let _ = user;
		grpc::UserData
	}
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("impl_trait_signature_guard.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPL-001"));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPL-001"));
	}

	#[test]
	fn impl_fix_does_not_rewrite_generic_target_variants_to_self() {
		let original = r#"
struct Inference<T> {
	output: T,
}
impl<T> Inference<T> {
	fn map_output<U>(self, f: impl FnOnce(T) -> U) -> Inference<U> {
		Inference { output: f(self.output) }
	}
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("impl_generic_self_guard.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPL-001"));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPL-001"));
	}

	#[test]
	fn impl003_does_not_flag_std_trait_order_for_unqualified_display_import() {
		let original = r#"
use std::fmt::{Display, Formatter};

#[derive(Debug)]
enum DispatchError {
	QueueFull,
}
impl Display for DispatchError {
	fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
		write!(f, "{f}")
	}
}
impl std::error::Error for DispatchError {}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("impl003_std_display_import_order.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, _edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPL-003"));
	}

	#[test]
	fn impl003_fix_reorders_impl_blocks_by_origin_group() {
		let original = r#"
struct Sample;

impl ext_crate::ExternalTrait for Sample {}

impl std::fmt::Display for Sample {
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		write!(f, "sample")
	}
}

impl Sample {
	fn new() -> Self {
		Self
	}
}

impl crate::WorkspaceTrait for Sample {}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("impl003_reorder_by_origin_group.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPL-003" && v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPL-003"));

		let impl003_edits =
			edits.into_iter().filter(|edit| edit.rule == "RUST-STYLE-IMPL-003").collect::<Vec<_>>();
		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, impl003_edits).expect("apply edits");

		assert!(applied > 0);

		let inherent_idx = rewritten.find("impl Sample {").expect("inherent impl");
		let std_trait_idx = rewritten
			.find("impl std::fmt::Display for Sample")
			.or_else(|| rewritten.find("impl Display for Sample"))
			.expect("std trait impl");
		let third_party_idx = rewritten
			.find("impl ext_crate::ExternalTrait for Sample")
			.or_else(|| rewritten.find("impl ExternalTrait for Sample"))
			.expect("third-party trait impl");
		let workspace_idx = rewritten
			.find("impl crate::WorkspaceTrait for Sample")
			.or_else(|| rewritten.find("impl WorkspaceTrait for Sample"))
			.expect("workspace trait impl");

		assert!(inherent_idx < std_trait_idx);
		assert!(std_trait_idx < third_party_idx);
		assert!(third_party_idx < workspace_idx);
	}

	#[test]
	fn numeric_rules_ignore_multiline_string_literals() {
		let text = r##"
const PROMPT: &str = r#"
- time_window values must be ISO-8601 durations.
- Absolute dates/years (example: 2025, by 2050) are not time_window.
"#;
"##;
		let ctx = shared::read_file_context_from_text(Path::new("num_prompt.rs"), text.to_owned())
			.expect("context")
			.expect("has ctx");
		let (violations, _edits) = style::collect_violations(&ctx, true);

		assert!(
			!violations
				.iter()
				.any(|v| matches!(v.rule, "RUST-STYLE-NUM-001" | "RUST-STYLE-NUM-002"))
		);
	}

	#[test]
	fn numeric_fix_applies_to_usize_and_float_integer_part() {
		let original = r#"
fn sample() {
	let a = 0usize;
	let b = 80000.0;
}
	"#;
		let ctx = shared::read_file_context_from_text(Path::new("num_fix.rs"), original.to_owned())
			.expect("context")
			.expect("has ctx");
		let (_violations, edits) = style::collect_violations(&ctx, true);
		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 2);
		assert!(rewritten.contains("0_usize"));
		assert!(rewritten.contains("80_000.0"));
	}

	#[test]
	fn numeric_fix_applies_inside_macro_token_trees() {
		let original = r#"
fn sample() {
	let v = vec![0.0f32; 4];
}
"#;
		let ctx =
			shared::read_file_context_from_text(Path::new("num_macro_fix.rs"), original.to_owned())
				.expect("context")
				.expect("has ctx");
		let (_violations, edits) = style::collect_violations(&ctx, true);
		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains("vec![0.0_f32; 4]"));
	}

	#[test]
	fn import_group_fix_normalizes_spacing_without_reordering_groups() {
		let original = r#"
use std::collections::HashSet;


use anyhow::Result;
use crate::z::Z;
"#;
		let ctx =
			shared::read_file_context_from_text(Path::new("import_fix.rs"), original.to_owned())
				.expect("context")
				.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-001"));
		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-002" && v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert_eq!(
			rewritten,
			r#"
use std::collections::HashSet;

use anyhow::Result;

use crate::z::Z;
"#
		);
	}

	#[test]
	fn import_group_fix_applies_with_cfg_attribute_string_literal() {
		let original = r#"#[cfg(feature = "test")]
use crate::z::Z;

use std::collections::HashSet;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import_group_cfg.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.is_empty());
		assert!(edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-002"));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert_eq!(
			rewritten,
			r#"use std::collections::HashSet;

#[cfg(feature = "test")]
use crate::z::Z;
"#
		);
	}

	#[test]
	fn pub_use_group_fix_removes_blank_lines_for_same_root() {
		let original = r#"
pub use tokenizers::Tokenizer;

pub use tokenizers::Error;

use unicode_segmentation::UnicodeSegmentation;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("pub_use_group_fix.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-002" && v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert_eq!(
			rewritten,
			r#"
pub use tokenizers::Tokenizer;
pub use tokenizers::Error;

use unicode_segmentation::UnicodeSegmentation;
"#
		);
	}

	#[test]
	fn pub_use_spacing_preserves_visibility_boundaries() {
		for (outer, inner) in [
			("pub", "pub(crate)"),
			("pub(crate)", "pub(super)"),
			("pub(super)", "pub(in crate::scope)"),
		] {
			let original =
				format!("{outer} use external::Public;\n\n{inner} use external::Internal;\n");
			let ctx = shared::read_file_context_from_text(
				Path::new("pub_use_visibility.rs"),
				original.clone(),
			)
			.expect("context")
			.expect("has ctx");
			let (violations, edits) = style::collect_violations(&ctx, true);
			let mut rewritten = original.clone();

			assert!(
				!violations
					.iter()
					.any(|v| { matches!(v.rule, "RUST-STYLE-IMPORT-002" | "RUST-STYLE-MOD-002") })
			);

			fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

			assert_eq!(rewritten, original);
		}
	}

	#[test]
	fn pub_use_spacing_repairs_each_visibility_group_and_converges() {
		let original = "pub use external::Public;\npub(crate) use external::First;\n\npub(crate) use external::Second;\n";
		let expected = "pub use external::Public;\n\npub(crate) use external::First;\npub(crate) use external::Second;\n";
		let mut rewritten = original.to_owned();

		for _ in 0..2 {
			let ctx = shared::read_file_context_from_text(
				Path::new("pub_use_convergence.rs"),
				rewritten.clone(),
			)
			.expect("context")
			.expect("has ctx");
			let (_, edits) = style::collect_violations(&ctx, true);

			fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

			assert_eq!(rewritten, expected);
		}
	}

	#[test]
	fn pub_use_group_fix_converges_local_module_reexports_to_self_group() {
		let original = r#"
mod add_event;
mod add_note;

pub use add_event::{AddEventRequest, AddEventResponse};

pub use add_note::{AddNoteRequest, AddNoteResponse};
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("pub_use_local_self_group_fix.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-002"
				&& v.fixable
				&& v.message
					== "Prefer converging local module re-exports into `pub use self::{...};`."
		}));
		assert!(edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-002"));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		let compact = rewritten.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();

		assert!(applied >= 1);
		assert!(compact.contains(
			"pubuseself::{add_event::{AddEventRequest,AddEventResponse},add_note::{AddNoteRequest,AddNoteResponse}};"
		));
	}

	#[test]
	fn pub_use_group_fix_converges_pub_super_local_reexports_with_cfg_tail() {
		let original = r#"
mod cache;
mod diversity;
mod policy;
mod text;

pub(super) use cache::{build_cached_scores, hash_query};
pub(super) use diversity::{build_rerank_ranks, select_diverse_results};
pub(super) use policy::{build_policy_snapshot, resolve_scopes};
pub(super) use text::{merge_matched_fields, tokenize_query};
#[cfg(test)] pub(super) use policy::BlendSegment;
#[cfg(test)] pub(super) use text::{lexical_overlap_ratio, scope_description_boost};
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("pub_use_local_self_group_pub_super_fix.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-002"
				&& v.fixable
				&& v.message
					== "Prefer converging local module re-exports into `pub use self::{...};`."
		}));
		assert!(edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-002"));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		let compact = rewritten.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();

		assert!(applied >= 1);
		assert!(compact.contains(
			"pub(super)useself::{cache::{build_cached_scores,hash_query},diversity::{build_rerank_ranks,select_diverse_results},policy::{build_policy_snapshot,resolve_scopes},text::{merge_matched_fields,tokenize_query}};"
		));
		assert!(compact.contains(
			"#[cfg(test)]pub(super)useself::{policy::BlendSegment,text::{lexical_overlap_ratio,scope_description_boost}};"
		));
	}

	#[test]
	fn import_group_fix_does_not_rewrite_unknown_separator_comments() {
		let original = r#"
use crate::z::Z;
// keep this comment
use std::collections::HashSet;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import_fix_unknown_comment.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-001" && !v.fixable));
		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-002" && !v.fixable));
		assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-001"));
	}

	#[test]
	fn import_group_treats_workspace_members_as_self_group_for_spacing() {
		let workspace_root = env!("CARGO_PKG_NAME").replace('-', "_");
		let original = format!("\nuse anyhow::Result;\nuse {workspace_root}::internal::Alpha;\n");
		let ctx = shared::read_file_context_from_text(
			Path::new("import_workspace_member.rs"),
			original.clone(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-001"));
		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-002" && v.fixable));

		let mut rewritten = original.clone();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert_eq!(
			rewritten,
			format!("\nuse anyhow::Result;\n\nuse {workspace_root}::internal::Alpha;\n")
		);
	}

	#[test]
	fn import_group_treats_file_local_mod_roots_as_self_group() {
		let original = r#"
use clap::Parser;
use tonic::transport::Server;
use tonic_health::server;
use tracing_subscriber::EnvFilter;
use auth::AuthInterceptor;
use cli::Cli;
use config::GatewayConfig;
use context::Context;
use db::Database;
use grpc::gateway_service_server::GatewayServiceServer;
use prelude::Core;
use service::GatewayService;
use types::App;

mod auth;
mod cli;
mod config;
mod context;
mod db;
mod error;
mod mail;
mod rate_limit;
mod service;
mod types;
mod grpc {
	tonic::include_proto!("pubfi.gateway");
}
mod prelude {
	pub use color_eyre::{Result, eyre};
	pub use time::OffsetDateTime;

	pub(crate) use crate::{error::SqlxStatusExt, grpc};
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import_local_mod_roots.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-001"));
		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-002" && v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(
			rewritten.contains("use tracing_subscriber::EnvFilter;\n\nuse auth::AuthInterceptor;")
		);
		assert!(
			rewritten.contains("use prelude::Core;\nuse service::GatewayService;\nuse types::App;")
		);
	}

	#[test]
	fn import_group_fix_reorders_origin_groups() {
		let original = r#"
use anyhow::Result;
use std::collections::HashSet;
use crate::z::Z;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import_group_reorder.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-001" && v.fixable));
		assert!(edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-002"));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);

		let std_idx = rewritten.find("use std::collections::HashSet;").expect("std");
		let third_party_idx = rewritten.find("use anyhow::Result;").expect("third-party");
		let self_idx = rewritten.find("use crate::z::Z;").expect("self");

		assert!(std_idx < third_party_idx);
		assert!(third_party_idx < self_idx);
	}

	#[test]
	fn import_group_reorder_still_applies_in_fallback_mode_with_import009_present() {
		let original = "use crate::z::Z;\n\nuse std::fmt::Result;\n\nfn parse() -> Result<()> {\n\tOk(())\n}\n";
		let ctx = shared::read_file_context_from_text(
			Path::new("import_group_fallback_import009.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (fallback_violations, fallback_edits) =
			super::collect_violations_with_import_shortening(&ctx, true, false);

		assert!(
			!fallback_edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-009"),
			"fallback mode should not collect import-shortening edits"
		);
		assert!(
			!fallback_violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009"),
			"fallback mode should not collect import-shortening violations"
		);
		assert!(
			fallback_edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-002"),
			"fallback mode should still include import-group edits, got edits={:?}, violations={:?}",
			fallback_edits.iter().map(|edit| edit.rule).collect::<Vec<_>>(),
			fallback_violations.iter().map(|violation| violation.rule).collect::<Vec<_>>()
		);

		let (rewritten, applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import_group_fallback_import009.rs"),
				original,
				false,
			)
			.expect("apply fix passes");

		assert!(applied_count > 0);
		assert!(rewritten.contains("use std::fmt::Result;\n\nuse crate::z::Z;\n"));
		assert!(rewritten.contains("fn parse() -> Result<()> {\n\tOk(())\n}\n"));
		assert!(rewritten.contains("use std::fmt::Result;"));
	}

	#[test]
	fn collect_fix_outcomes_stores_lazy_import_fallback_source_text() {
		let nanos = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.expect("current timestamp")
			.as_nanos();
		let path =
			env::temp_dir().join(format!("vstyle-lazy-fallback-{}-{}.rs", process::id(), nanos));
		let original =
			"use std::fmt::Result;\n\nfn parse() -> std::fmt::Result<()> {\n\tOk(())\n}\n";

		fs::write(&path, original).expect("seed temp file");

		let outcomes = super::collect_fix_outcomes(
			slice::from_ref(&path),
			&super::TypeAliasRenamePlan::default(),
			"test",
			false,
		)
		.expect("collect fix outcomes");
		let outcome = outcomes.first().expect("one outcome");

		assert!(outcome.rewritten_text.is_some());
		assert_eq!(outcome.import_fallback_source_text.as_deref(), Some(original));

		let fallback = super::compute_import_fallback(
			&path,
			outcome.import_fallback_source_text.as_deref().expect("fallback source"),
		)
		.expect("compute fallback");

		assert_eq!(fallback, original);

		let _ = fs::remove_file(&path);
	}

	#[test]
	fn import_fix_normalizes_mixed_self_child_use_tree() {
		let original = r#"
use crate::alpha::{beta, beta::Gamma};

fn sample(value: beta::Gamma) -> beta::Gamma {
	value
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import_self_child.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-002"
				&& v.message
					== "Normalize imports like `use a::{b, b::c}` to `use a::{b::{self, c}}`."
				&& v.fixable
		}));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains("use crate::alpha::{beta::{self, Gamma}};"));
	}

	#[test]
	fn import_fix_does_not_rewrite_already_grouped_multiline_use_tree() {
		let original = r#"
	use std::{
		future::{self, Future, Ready},
		pin::Pin,
		rc::Rc,
		task::{Context, Poll},
	};
	"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import_already_grouped_multiline.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-002"
				&& v.message
					== "Normalize imports like `use a::{b, b::c}` to `use a::{b::{self, c}}`."
		}));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-002"));
	}

	#[test]
	fn import_fix_does_not_collapse_multiline_braced_use_tree_when_semantics_are_unchanged() {
		let original = r#"
	use qdrant_client::{
		Qdrant, QdrantError,
		qdrant::{
			CreateCollectionBuilder, Distance, Modifier, SparseVectorParamsBuilder,
			SparseVectorsConfigBuilder, VectorParamsBuilder, VectorsConfigBuilder,
		},
	};
	"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import_multiline_braced_third_party.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);
		let edits_debug = format!("{edits:?}");
		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-003"));
		assert_eq!(applied, 0, "Expected no edits.\nEdits: {edits_debug}\nRewritten:\n{rewritten}");
		assert_eq!(rewritten, original);
	}

	#[test]
	fn import002_fix_normalizes_mixed_self_child_use_tree_with_aliases() {
		let original = r#"
	use crate::{grpc, grpc::{ReferralCode as ProtoReferralCode, gateway_service_server::GatewayService as GrpcGatewayService}};
	"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import002_mixed_self_child_with_aliases.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (_violations, edits) = style::collect_violations(&ctx, true);
		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(rewritten.contains(
			"use crate::{grpc::{ReferralCode as ProtoReferralCode, gateway_service_server::GatewayService as GrpcGatewayService}};"
		));
		assert!(!rewritten.contains("use crate::{grpc, grpc::"));
	}

	#[test]
	fn import002_fix_drops_unused_self_from_nested_use_group() {
		let original = r#"
use crate::{grpc::{self, ReferralCode as ProtoReferralCode, VerifyMailCodeRequest}};
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import002_drop_unused_self_in_nested_group.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (_violations, edits) = style::collect_violations(&ctx, true);
		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(rewritten.contains(
			"use crate::{grpc::{ReferralCode as ProtoReferralCode, VerifyMailCodeRequest}};"
		));
		assert!(!rewritten.contains("grpc::{self,"));
	}

	#[test]
	fn import003_fix_rewrites_trait_keep_alive_simple_use_to_as_underscore() {
		let original = r#"
use std::io::Read;

fn sample(mut data: &[u8]) -> usize {
	let mut out = [0_u8; 1];
	let _ = data.read(&mut out);
	out[0] as usize
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import003_trait_keep_alive_simple.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-003"
				&& v.message.contains("Trait keep-alive import `Read` should use `as _`")
				&& v.fixable
		}));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-003"));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains("use std::io::Read as _;"));
	}

	#[test]
	fn import003_fix_rewrites_trait_keep_alive_braced_use_to_as_underscore() {
		let original = r#"
use std::io::{Read, Write};

fn sample(mut data: &[u8], mut sink: Vec<u8>) -> usize {
	let mut out = [0_u8; 1];
	let _ = data.read(&mut out);
	let _ = sink.write(&out);
	out[0] as usize
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import003_trait_keep_alive_braced.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-003"
				&& v.message.contains("Trait keep-alive import `Read` should use `as _`")
				&& v.fixable
		}));
		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-003"
				&& v.message.contains("Trait keep-alive import `Write` should use `as _`")
				&& v.fixable
		}));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-003"));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains("use std::io::{Read as _, Write as _};"));
	}

	#[test]
	fn import003_does_not_require_as_underscore_when_trait_name_is_referenced() {
		let text = r#"
use std::io::Read;

fn read_one<R: Read>(mut reader: R) -> usize {
	let mut out = [0_u8; 1];
	let _ = reader.read(&mut out);
	out[0] as usize
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import003_trait_name_referenced.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-003"
				&& v.message.contains("Trait keep-alive import `Read` should use `as _`")
		}));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-003"));
	}

	#[test]
	fn import003_does_not_rewrite_trait_imports_in_parent_module_with_child_decls() {
		let text = r#"
mod child;

use serde::Deserialize;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import003_parent_module_child_decl.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-003"));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-003"));
	}

	#[test]
	fn import003_preserves_explicit_keep_alive_with_external_child_modules() {
		for imports in [
			"use std::io::Read as _;",
			"use std::io::{Read as _, Write as _};",
			"use std::io::{Read as _, Write};",
			"#[cfg(unix)] use std::os::unix::fs::MetadataExt as _;",
		] {
			let original = format!("mod child;\n\n{imports}\n");
			let ctx = shared::read_file_context_from_text(
				Path::new("import003_explicit_keep_alive_parent.rs"),
				original,
			)
			.expect("context")
			.expect("has ctx");
			let (violations, edits) = style::collect_violations(&ctx, true);

			assert!(
				!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-003"),
				"must retain explicit keep-alive and named child imports: {imports}"
			);
			assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-003"));
		}
	}

	#[test]
	fn import003_restores_directly_referenced_trait_name_with_external_child_modules() {
		let original = "mod child;\n\nuse std::io::Read as _;\n\nfn sample<T: Read>() {}\n";
		let ctx = shared::read_file_context_from_text(
			Path::new("import003_referenced_trait_parent.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-003" && v.message.contains("referenced directly")
		}));

		let mut rewritten = original.to_owned();

		fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(rewritten.contains("use std::io::Read;"));
		assert!(!rewritten.contains("Read as _"));
	}

	#[test]
	fn import003_fix_dedupes_plain_and_keep_alive_trait_imports_when_referenced() {
		let original = r#"
use serde::{Deserialize, Deserialize as _, Serialize, Serialize as _};

#[derive(Deserialize, Serialize)]
struct Payload {
	value: String,
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import003_trait_dedupe_when_referenced.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (_violations, edits) = style::collect_violations(&ctx, true);
		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(rewritten.contains("use serde::{Deserialize"));
		assert!(rewritten.contains("Serialize"));
		assert!(!rewritten.contains("Deserialize as _"));
		assert!(!rewritten.contains("Serialize as _"));
	}

	#[test]
	fn import003_fix_dedupes_plain_and_keep_alive_trait_imports_when_unreferenced() {
		let original = r#"
use std::io::{Read, Read as _};

fn noop() {}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import003_trait_dedupe_when_unreferenced.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (_violations, edits) = style::collect_violations(&ctx, true);
		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(rewritten.contains("use std::io::{Read as _};"));
		assert!(!rewritten.contains("use std::io::{Read, Read as _};"));
	}

	#[test]
	fn import003_fix_normalizes_unreferenced_trait_alias_and_avoids_ambiguous_symbol() {
		let original = r#"
use color_eyre::eyre::{Context as EyreContext, Result, eyre};
use std::task::Context;

fn run() -> Result<()> {
	let _ = eyre!("boom").wrap_err("failed");
	let _ = core::option::Option::<&Context<'_>>::None;
	Ok(())
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import003_trait_alias_context_ambiguous.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-004"
				&& v.message
					== "Do not import free functions or macros into scope; prefer qualified module paths."
				&& v.fixable
		}));
		assert!(!violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-003"
				&& v.message == "Import aliases are not allowed except `as _` keep-alive imports."
				&& !v.fixable
		}));
		assert!(
			edits
				.iter()
				.any(|e| matches!(e.rule, "RUST-STYLE-IMPORT-003" | "RUST-STYLE-IMPORT-004"))
		);

		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(
			rewritten.contains("use color_eyre::eyre::{self, Context as EyreContext, Result};"),
			"{rewritten}"
		);
		assert!(rewritten.contains("let _ = eyre::eyre!(\"boom\").wrap_err(\"failed\");"));

		let rewritten_ctx = shared::read_file_context_from_text(
			Path::new("import003_trait_alias_context_ambiguous.rs"),
			rewritten,
		)
		.expect("context")
		.expect("has ctx");
		let (rewritten_violations, _rewritten_edits) =
			style::collect_violations(&rewritten_ctx, true);

		assert!(!rewritten_violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-004"
				&& v.message
					== "Ambiguous imported symbol `Context` is not allowed; use fully qualified paths."
		}));

		let (
			stabilized,
			_second_pass_applied,
			_had_import_shortening_edits,
			_had_let_mut_reorder_edits,
		) = style::apply_fix_passes(
			Path::new("import003_trait_alias_context_ambiguous.rs"),
			&rewritten_ctx.text,
			true,
		)
		.expect("apply fix passes");

		assert!(
			stabilized.contains("use color_eyre::eyre::{self, Context as _, Result};"),
			"{stabilized}"
		);
		assert!(!stabilized.contains("Context as EyreContext"));
	}

	#[test]
	fn import003_alias_is_non_fixable_when_alias_identifier_is_referenced() {
		let text = r#"
use color_eyre::eyre::Context as EyreContext;

fn needs_context<T: EyreContext>(value: T) -> T {
	value
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import003_trait_alias_referenced.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-003"
				&& v.message
					== "Import alias `EyreContext` is not allowed; use a fully qualified path at use sites."
				&& v.fixable
		}));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-003"));

		let mut rewritten = text.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(!rewritten.contains("as EyreContext"));
		assert!(
			rewritten.contains("fn needs_context<T: color_eyre::eyre::Context>(value: T) -> T {")
		);
	}

	#[test]
	fn import003_fix_rewrites_non_trait_alias_to_qualified_paths() {
		let original = r#"
use crate::error::{Error, Result};
use pubfi_ai::{AgentEmbed, Error as AiError};

fn should_retry_embed(error: &AiError) -> bool {
	matches!(error, AiError::Reqwest(_))
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import003_non_trait_alias_qualified_paths.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-003" && v.message.contains("`AiError`") && v.fixable
		}));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-003"));

		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(rewritten.contains("use pubfi_ai::{AgentEmbed};"));
		assert!(rewritten.contains("error: &pubfi_ai::Error"));
		assert!(rewritten.contains("pubfi_ai::Error::Reqwest"), "{rewritten}");
		assert!(!rewritten.contains("Error as AiError"));
	}

	#[test]
	fn import003_fix_rewrites_nested_braced_aliases_to_qualified_paths() {
		let original = r#"
use crate::{
	grpc::{
		ReferralCode as ProtoReferralCode,
		UserData,
		gateway_service_server::GatewayService as GrpcGatewayService,
	},
};

fn map_types(
	_code: ProtoReferralCode,
	_service: GrpcGatewayService,
	_user_data: UserData,
) -> (crate::grpc::ReferralCode, crate::grpc::gateway_service_server::GatewayService) {
	todo!()
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import003_nested_braced_aliases.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(
			violations.iter().any(|v| {
				v.rule == "RUST-STYLE-IMPORT-003"
					&& v.message.contains("`ProtoReferralCode`")
					&& v.fixable
			}),
			"{violations:#?}"
		);
		assert!(
			violations.iter().any(|v| {
				v.rule == "RUST-STYLE-IMPORT-003"
					&& v.message.contains("`GrpcGatewayService`")
					&& v.fixable
			}),
			"{violations:#?}"
		);
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-003"));

		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(rewritten.contains("use crate::{grpc::{UserData}};"), "{rewritten}");
		assert!(rewritten.contains("_code: crate::grpc::ReferralCode"));
		assert!(
			rewritten.contains("_service: crate::grpc::gateway_service_server::GatewayService")
		);
		assert!(!rewritten.contains("ReferralCode as ProtoReferralCode"));
		assert!(!rewritten.contains("GatewayService as GrpcGatewayService"));
	}

	#[test]
	fn import_check_does_not_report_ambiguous_symbol_for_keep_alive_alias() {
		let text = r#"
use color_eyre::eyre::Context as _;
use std::task::Context;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import_keep_alive_ambiguous_false_positive.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, _edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-004"
				&& v.message
					== "Ambiguous imported symbol `Context` is not allowed; use fully qualified paths."
		}));
	}

	#[test]
	fn import012_reports_unused_crate_keep_alive_import() {
		let text = r#"
use regex as _;

fn main() {}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import012_unused_crate_keep_alive.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-012"
				&& v.message
					== "Do not add crate keep-alive imports `use dep as _;` unless another path in the package uses that crate."
				&& !v.fixable
		}));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-012"));
	}

	#[test]
	fn import012_allows_crate_keep_alive_import_with_other_crate_path_usage() {
		let text = r#"
use regex as _;

fn compile() {
	let _ = regex::Regex::new("a+");
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import012_used_crate_keep_alive.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-012"));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-012"));
	}

	#[test]
	fn import012_allows_crate_keep_alive_import_with_cross_file_path_usage() {
		let now = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.expect("Read timestamp.")
			.as_nanos();
		let root =
			env::temp_dir().join(format!("vstyle-import012-cross-file-{}-{now}", process::id()));
		let src_dir = root.join("src");
		let main_path = src_dir.join("main.rs");
		let helper_path = src_dir.join("helper.rs");

		fs::create_dir_all(&src_dir).expect("Create temp src directory.");
		fs::write(
			root.join("Cargo.toml"),
			"[package]\nname = \"vstyle-import012-cross-file\"\nversion = \"0.0.0\"\nedition = \"2021\"\n[dependencies]\nregex = \"1\"\n",
		)
		.expect("Write Cargo manifest.");
		fs::write(
			&main_path,
			"use regex as _;\n\nmod helper;\n\nfn main() {\n\thelper::compile();\n}\n",
		)
		.expect("Write main source.");
		fs::write(
			&helper_path,
			"pub(crate) fn compile() {\n\tlet _ = regex::Regex::new(\"a+\");\n}\n",
		)
		.expect("Write helper source.");

		let ctx =
			shared::read_file_context(&main_path).expect("Read context.").expect("Have context.");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-012"));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-012"));

		let _ = fs::remove_dir_all(&root);
	}

	#[test]
	fn import012_does_not_report_trait_keep_alive_imports() {
		let text = r#"
use std::fmt::Write as _;

fn main() {
	let mut out = String::new();
	let _ = write!(out, "ok");
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import012_trait_keep_alive.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-012"));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-012"));
	}

	#[test]
	fn import008_fix_rewrites_existing_keep_alive_trait_import_to_plain_when_symbol_is_used() {
		let original = r#"
use serde::{Deserialize as _};

fn decode<T: serde::Deserialize<'static>>() {}
"#;
		let mut rewritten = original.to_owned();

		for _ in 0..4 {
			let ctx = shared::read_file_context_from_text(
				Path::new("import008_trait_keep_alive_upgrade.rs"),
				rewritten.clone(),
			)
			.expect("context")
			.expect("has ctx");
			let (_violations, edits) = style::collect_violations(&ctx, true);

			if edits.is_empty() {
				break;
			}

			let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		}

		assert!(rewritten.contains("use serde::{Deserialize};"), "{rewritten}");
		assert!(!rewritten.contains("Deserialize as _"));
		assert!(rewritten.contains("fn decode<T: Deserialize<'static>>() {}"));
	}

	#[test]
	fn import_check_reports_ambiguous_symbol_imports() {
		let text = r#"
use foo::Client;
use bar::Client;
"#;
		let ctx =
			shared::read_file_context_from_text(Path::new("import_ambiguous.rs"), text.to_owned())
				.expect("context")
				.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-004"
				&& v.message
					== "Ambiguous imported symbol `Client` is not allowed; use fully qualified paths."
				&& !v.fixable
		}));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-004"));
	}

	#[test]
	fn import_check_does_not_report_brace_artifact_symbol() {
		let text = r#"
use async_nats::{
	ConnectOptions, Event, HeaderMap,
	jetstream::{
		self, Context,
		consumer::{AckPolicy, PullConsumer},
	},
};
use crate::{
	config::{ConnectionConfig, ConsumeConfig, PublishConfig},
	error::{Error, Result},
	events::{DocumentIngestedEvent, FeedMatchEvent},
	msg_id,
};
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import_brace_artifact.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, _edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-004"
				&& v.message.contains("Ambiguous imported symbol `}` is not allowed")
		}));
	}

	#[test]
	fn import_check_does_not_report_ambiguous_self_from_braced_use_tree() {
		let text = r#"
use futures::stream::{self, TryStreamExt};
use crate::{store::{self, InsightInsertOutcome}};
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import_self_ambiguous_false_positive.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, _edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-004"
				&& v.message.contains("Ambiguous imported symbol `self` is not allowed")
		}));
	}

	#[test]
	fn import_fix_qualifies_unqualified_function_calls() {
		let original = r#"
use crate::math::sum;

fn sample() -> usize {
	sum(1, 2)
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import004_function_fix.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-004"
				&& v.message
					== "Do not import free functions or macros into scope; prefer qualified module paths."
				&& v.fixable
		}));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-004"));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 2);
		assert!(!rewritten.contains("use crate::math::sum;"));
		assert!(rewritten.contains("use crate::math;"));
		assert!(rewritten.contains("math::sum(1, 2)"));
	}

	#[test]
	fn import_fix_uses_full_path_when_parent_module_name_is_ambiguous() {
		let original = r#"
use other::math;
use crate::math::sum;

fn sample() -> usize {
	sum(1, 2)
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import004_function_fix_ambiguous_module.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(!rewritten.contains("use crate::math;"));
		assert!(rewritten.contains("crate::math::sum(1, 2)"), "{rewritten}");
	}

	#[test]
	fn import004_shortens_existing_full_path_free_function_call() {
		let original = r#"
fn sample() {
	crate::app::build_auth_state();
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import004_existing_full_path_function.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(rewritten.contains("use crate::app;"), "{rewritten}");
		assert!(rewritten.contains("app::build_auth_state();"), "{rewritten}");
		assert!(!rewritten.contains("crate::app::build_auth_state();"), "{rewritten}");
	}

	#[test]
	fn import004_does_not_hoist_import_out_of_cfg_feature_expression() {
		let original = r#"
async fn entrypoint() {
	#[cfg(feature = "process-test-support")]
	if std::env::var("PROCESS_TEST").is_ok() {
		return crate::process_test_support::serve_disabled_artifact_runtime().await;
	}
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import004_cfg_feature_expression.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert_eq!(rewritten, original);
	}

	#[test]
	fn import004_keeps_existing_full_path_when_parent_module_name_is_ambiguous() {
		let original = r#"
use other::app;

fn sample() {
	crate::app::build_auth_state();
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import004_existing_full_path_function_ambiguous.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(!rewritten.contains("use crate::app;"), "{rewritten}");
		assert!(rewritten.contains("crate::app::build_auth_state();"), "{rewritten}");
	}

	#[test]
	fn import004_merges_parent_module_import_when_shortening_existing_full_path_call() {
		let original = r#"
use crate::{app::AuthState};

fn sample() {
	let _ = AuthState;
	crate::app::build_auth_state();
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import004_existing_full_path_function_merge.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(rewritten.contains("use crate::{app::{self, AuthState}};"), "{rewritten}");
		assert!(rewritten.contains("app::build_auth_state();"), "{rewritten}");
		assert!(!rewritten.contains("crate::app::build_auth_state();"), "{rewritten}");
	}

	#[test]
	fn import004_shortens_existing_full_path_call_inside_cfg_test_module() {
		let original = r#"
#[cfg(test)]
mod tests {
	use crate::app::AuthState;

	#[test]
	fn sample() {
		let _ = AuthState;
		crate::app::build_auth_state();
	}
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import004_existing_full_path_function_cfg_test.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(rewritten.contains("\tuse crate::app::{self, AuthState};"), "{rewritten}");
		assert!(rewritten.contains("\t\tapp::build_auth_state();"), "{rewritten}");
		assert!(!rewritten.contains("crate::app::build_auth_state();"), "{rewritten}");
	}

	#[test]
	fn import004_shortens_existing_full_path_call_inside_macro_tokens() {
		let original = r#"
fn sample() {
	assert!(crate::app::build_auth_state().is_ok());
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import004_existing_full_path_function_macro_tokens.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(rewritten.contains("use crate::app;"), "{rewritten}");
		assert!(rewritten.contains("assert!(app::build_auth_state().is_ok());"), "{rewritten}");
		assert!(!rewritten.contains("crate::app::build_auth_state()"), "{rewritten}");
	}

	#[test]
	fn import004_preserves_turbofish_when_shortening_existing_full_path_call() {
		let original = r#"
fn sample() {
	crate::app::build_auth_state::<AuthState>();
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import004_existing_full_path_function_turbofish.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(rewritten.contains("use crate::app;"), "{rewritten}");
		assert!(rewritten.contains("app::build_auth_state::<AuthState>();"), "{rewritten}");
		assert!(!rewritten.contains("crate::app::build_auth_state::<AuthState>();"), "{rewritten}");
	}

	#[test]
	fn import004_keeps_existing_full_path_when_root_qualified_time_path_would_conflict() {
		let original = r#"
fn sample() {
	let _ = time::OffsetDateTime::UNIX_EPOCH;
	tokio::time::sleep();
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import004_existing_full_path_function_root_time_conflict.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(!rewritten.contains("use tokio::time;"), "{rewritten}");
		assert!(rewritten.contains("tokio::time::sleep();"), "{rewritten}");
	}

	#[test]
	fn import004_keeps_existing_full_path_when_imported_root_time_use_would_conflict() {
		let original = r#"
use time::OffsetDateTime;

fn sample() {
	let _ = OffsetDateTime::UNIX_EPOCH;
	tokio::time::sleep();
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import004_existing_full_path_function_imported_time_conflict.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(rewritten.contains("use time::OffsetDateTime;"), "{rewritten}");
		assert!(!rewritten.contains("use tokio::time;"), "{rewritten}");
		assert!(rewritten.contains("tokio::time::sleep();"), "{rewritten}");
	}

	#[test]
	fn import004_keeps_existing_full_path_when_root_qualified_product_path_would_conflict() {
		let original = r#"
fn sample() {
	let _ = product::Catalog::default();
	crate::product::build();
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import004_existing_full_path_function_root_product_conflict.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(!rewritten.contains("use crate::product;"), "{rewritten}");
		assert!(rewritten.contains("product::build();"), "{rewritten}");
	}

	#[test]
	fn import004_keeps_existing_full_path_when_imported_root_product_use_would_conflict() {
		let original = r#"
use product::Catalog;

fn sample() {
	let _ = Catalog::default();
	crate::product::build();
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import004_existing_full_path_function_imported_product_conflict.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(rewritten.contains("use product::Catalog;"), "{rewritten}");
		assert!(!rewritten.contains("use crate::product;"), "{rewritten}");
		assert!(rewritten.contains("product::build();"), "{rewritten}");
	}

	#[test]
	fn import_fix_rewrites_std_iter_once_to_parent_module_path() {
		let original = r#"
#[cfg(test)]
mod tests {
	use std::iter::once;

	#[test]
	fn sample() {
		let _ = once(1usize);
	}
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(Path::new("import004_std_iter_once.rs"), original, true)
				.expect("apply fix passes");

		assert!(rewritten.contains("\tuse std::iter;"), "{rewritten}");
		assert!(rewritten.contains("\t\tlet _ = iter::once("), "{rewritten}");
		assert!(!rewritten.contains("use std::iter::once;"), "{rewritten}");
	}

	#[test]
	fn import004_does_not_rewrite_method_calls_through_parent_module_imports() {
		let original = r#"
use std::iter;

fn sample() {
	let ordered = [1usize, 2usize];
	let _ = ordered.iter().copied().collect::<Vec<_>>();
	let _ = iter::once(3usize);
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import004_method_call_parent_module_import.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(rewritten.contains("ordered.iter().copied()"), "{rewritten}");
		assert!(!rewritten.contains("ordered.std::iter()"), "{rewritten}");
		assert!(rewritten.contains("iter::once("), "{rewritten}");
	}

	#[test]
	fn import004_does_not_rewrite_method_calls_inside_macro_tokens() {
		let original = r#"
use std::iter;

fn sample() {
	let ordered = [1usize, 2usize];
	let _ = format!(
		"{:?}",
		ordered.iter().map(|entry| entry.saturating_add(1)).collect::<Vec<_>>()
	);
	let _ = iter::once(3usize);
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import004_method_call_inside_macro_tokens.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(rewritten.contains("ordered.iter().map("), "{rewritten}");
		assert!(!rewritten.contains("ordered.std::iter()"), "{rewritten}");
		assert!(rewritten.contains("iter::once("), "{rewritten}");
	}

	#[test]
	fn import004_keeps_full_path_when_free_function_import_root_time_use_would_conflict() {
		let original = r#"
use time::OffsetDateTime;
use tokio::time::sleep;

fn sample() {
	let _ = OffsetDateTime::UNIX_EPOCH;
	sleep();
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import004_free_function_import_imported_time_conflict.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(rewritten.contains("use time::OffsetDateTime;"), "{rewritten}");
		assert!(!rewritten.contains("use tokio::time;"), "{rewritten}");
		assert!(!rewritten.contains("use tokio::time::sleep;"), "{rewritten}");
		assert!(rewritten.contains("tokio::time::sleep();"), "{rewritten}");
		assert!(!rewritten.contains("\n\ttime::sleep();"), "{rewritten}");
	}

	#[test]
	fn cfg_test_module_import_group_and_free_function_rules_apply_inside_tests_module() {
		let original = r#"
#[cfg(test)]
mod tests {
	use crate::math::sum;
	use anyhow::Result;
	use std::collections::HashSet;

	#[test]
	fn sample() -> Result<()> {
		let values = HashSet::new();
		let _ = values;
		Ok(sum(1, 2))
	}
}
"#;
		let path = Path::new("cfg_test_module_import_scope_gap.rs");
		let ctx = shared::read_file_context_from_text(path, original.to_owned())
			.expect("context")
			.expect("has ctx");
		let (violations, _edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-001" && v.fixable));
		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-002" && v.fixable));
		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-004"
				&& v.message
					== "Do not import free functions or macros into scope; prefer qualified module paths."
				&& v.fixable
		}));

		let (rewritten, applied, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(path, original, true).expect("apply fix passes");

		assert!(applied >= 3);

		let std_use_idx =
			rewritten.find("use std::collections::HashSet;").expect("std import must remain");
		let anyhow_use_idx =
			rewritten.find("use anyhow::Result;").expect("third-party import must remain");

		assert!(std_use_idx < anyhow_use_idx);
		assert!(rewritten.contains(
			"use std::collections::HashSet;\n\n\tuse anyhow::Result;\n\n\tuse crate::math;"
		));
		assert!(rewritten.contains("Ok(math::sum(1, 2))"));
		assert!(!rewritten.contains("\tuse crate::math::sum;"));
	}

	#[test]
	fn import006_allows_file_top_level_use() {
		let text = r#"
use std::collections::BTreeMap;

fn sample() {
	let _ = BTreeMap::<u8, u8>::new();
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import006_file_top_level.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-006"));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-006"));
	}

	#[test]
	fn import006_allows_inline_module_top_level_use() {
		let text = r#"
mod nested {
	use std::collections::BTreeMap;

	fn sample() {
		let _ = BTreeMap::<u8, u8>::new();
	}
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import006_inline_module_top_level.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-006"));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-006"));
	}

	#[test]
	fn import006_reports_function_local_use() {
		let text = r#"
fn sample() {
	use std::collections::BTreeMap;

	let _ = BTreeMap::<u8, u8>::new();
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import006_function_local_violation.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-006"
				&& v.message == "Use items must appear only at file top level or module top level."
				&& v.fixable
		}));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-006"));
	}

	#[test]
	fn import006_reports_nested_block_local_use() {
		let text = r#"
fn sample() {
	if true {
		use std::collections::BTreeMap;

		let _ = BTreeMap::<u8, u8>::new();
	}
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import006_nested_block_violation.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-006"
				&& v.message == "Use items must appear only at file top level or module top level."
				&& v.fixable
		}));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-006"));
	}

	#[test]
	fn import006_fix_hoists_simple_function_local_use_to_file_scope() {
		let original = r#"
fn sample() {
	use std::collections::BTreeMap;

	let _ = BTreeMap::<u8, u8>::new();
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(Path::new("import006_file_scope_fix.rs"), original, true)
				.expect("apply fix passes");

		assert!(rewritten.contains("use std::collections::BTreeMap;"));
		assert!(rewritten.contains("use std::collections::BTreeMap;\n\nfn sample()"));
		assert!(!rewritten.contains("\n\tuse std::collections::BTreeMap;\n"));
	}

	#[test]
	fn import006_fix_hoists_simple_function_local_use_to_enclosing_module_scope() {
		let original = r#"
mod nested {
	fn sample() {
		use std::collections::BTreeMap;

		let _ = BTreeMap::<u8, u8>::new();
	}
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(Path::new("import006_module_scope_fix.rs"), original, true)
				.expect("apply fix passes");

		assert!(
			rewritten.contains("mod nested {\n\tuse std::collections::BTreeMap;"),
			"{rewritten}"
		);
		assert!(rewritten.contains("\n\tfn sample()"));
		assert!(!rewritten.contains("\n\t\tuse std::collections::BTreeMap;\n"));
	}

	#[test]
	fn import006_fix_deletes_local_use_when_equivalent_module_scope_import_already_exists() {
		let original = r#"
mod nested {
	use std::collections::BTreeMap;

	fn sample() {
		use std::collections::BTreeMap;

		let _ = BTreeMap::<u8, u8>::new();
	}
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import006_delete_duplicate_local_use.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert_eq!(rewritten.matches("use std::collections::BTreeMap;").count(), 1);
		assert!(!rewritten.contains("\n\t\tuse std::collections::BTreeMap;\n"));
	}

	#[test]
	fn import006_cfg_gated_function_local_use_is_reported_non_fixable() {
		let text = r#"
#[cfg(feature = "cli")]
fn sample() {
	use std::collections::BTreeMap;

	let _ = BTreeMap::<u8, u8>::new();
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import006_cfg_gated_local_use.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-006"
				&& v.message == "Use items must appear only at file top level or module top level."
				&& !v.fixable
		}));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-006"));
	}

	#[test]
	fn shared_import_insertions_preserve_distinct_conditions() {
		let edits = vec![
			Edit {
				start: 0,
				end: 0,
				replacement: "use std::io::ErrorKind;\n#[cfg(unix)]\nuse std::fmt::Error;\n".into(),
				rule: "RUST-STYLE-IMPORT-006",
			},
			Edit {
				start: 0,
				end: 0,
				replacement: "use std::io::ErrorKind;\n#[cfg(windows)]\nuse std::fmt::Error;\n"
					.into(),
				rule: "RUST-STYLE-IMPORT-008",
			},
		];
		let mut text = "fn sample() {}\n".to_owned();

		fixes::apply_edits(&mut text, edits).expect("Apply insertions.");

		assert_eq!(text.matches("use std::io::ErrorKind;").count(), 1, "{text}");
		assert!(text.contains("#[cfg(unix)]\nuse std::fmt::Error;"), "{text}");
		assert!(text.contains("#[cfg(windows)]\nuse std::fmt::Error;"), "{text}");
	}

	#[test]
	fn import006_and_import008_share_inserted_type_imports() {
		let original = "pub fn sample() -> std::io::ErrorKind {\n use std::io::ErrorKind;\n ErrorKind::NotFound\n}\n";
		let (rewritten, _, _, _) =
			style::apply_fix_passes(Path::new("shared_type_import.rs"), original, true)
				.expect("Apply fixes.");

		assert_eq!(rewritten.matches("use std::io::ErrorKind;").count(), 1, "{rewritten}");
	}

	#[test]
	fn parent_glob_expansion_preserves_conditional_bindings() {
		let original = r#"#[cfg(feature = "enabled")]
struct Conditional;
#[cfg(feature = "enabled")]
use std::io::ErrorKind;
struct Always;
#[cfg_attr(not(feature = "enabled"), cfg(any()))]
struct ViaAttr;
#[cfg_attr(feature = "enabled", cfg_attr(not(feature = "extra"), cfg(any())))]
struct Nested;
#[cfg(feature = "enabled")]
#[cfg(feature = "extra")]
struct Both;
#[cfg(feature = "enabled")]
struct Either;
#[cfg(not(feature = "enabled"))]
struct Either;
#[cfg(test)]
mod tests {
    #[cfg(test)]
    use super::*;
    #[test]
    fn unconditional() { let _ = Always; let _ = Either; }
    #[cfg(any(not(feature = "enabled"), feature = "extra"))]
    #[test]
    fn nested() { let _ = Nested; }
    #[cfg(all(feature = "enabled", feature = "extra"))]
    #[test]
    fn both() { let _ = Both; }
    #[cfg(feature = "enabled")]
    #[test]
    fn conditional() {
        let _ = Conditional;
        let _ = ViaAttr;
        let _ = ErrorKind::NotFound;
    }
}
"#;
		let now = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.expect("Timestamp.")
			.as_nanos();
		let root = env::temp_dir().join(format!("vstyle-glob-cfg-{}-{now}", process::id()));
		let path = root.join("lib.rs");
		let binary = root.join("fixture");

		fs::create_dir_all(&root).expect("Create fixture directory.");

		let (rewritten, _, _, _) =
			style::apply_fix_passes(&path, original, true).expect("Apply fixes.");

		assert!(!rewritten.contains("::*"), "{rewritten}");

		fs::write(&path, &rewritten).expect("Write rewritten fixture.");

		for (enabled, extra) in [(false, false), (false, true), (true, false), (true, true)] {
			let mut compiler = process::Command::new("rustc");

			compiler.args(["--edition=2024", "--test"]).arg(&path).arg("-o").arg(&binary);

			if enabled {
				compiler.args(["--cfg", "feature=\"enabled\""]);
			}
			if extra {
				compiler.args(["--cfg", "feature=\"extra\""]);
			}

			let compiled = compiler.output().expect("Compile rewritten fixture.");

			assert!(
				compiled.status.success(),
				"enabled={enabled}, extra={extra}\n{rewritten}\n{}",
				String::from_utf8_lossy(&compiled.stderr)
			);

			let executed = process::Command::new(&binary).output().expect("Run rewritten fixture.");

			assert!(executed.status.success(), "{}", String::from_utf8_lossy(&executed.stderr));

			let expected = if enabled && extra { 4 } else { 2 };

			assert!(
				String::from_utf8_lossy(&executed.stdout).contains(&format!("{expected} passed;"))
			);
		}

		fs::remove_dir_all(root).expect("Remove fixture.");
	}

	#[test]
	fn import009_preserves_test_module_macro_bindings() {
		let original = "use std::io::Error;\nfn outer(_: Error) {}\nfn other(_: std::fmt::Error) {}\n#[cfg(test)]\nmod tests {\n\tuse std::fmt::Error;\n\t#[test]\n\tfn inner() {\n\t\tassert_eq!(format!(\"{:?}\", Error::default()), \"Error\");\n\t}\n}\n";
		let now = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.expect("Timestamp.")
			.as_nanos();
		let root = env::temp_dir().join(format!("vstyle-macro-scope-{}-{now}", process::id()));
		let path = root.join("lib.rs");
		let binary = root.join("fixture");

		fs::create_dir_all(&root).expect("Create fixture directory.");

		let (rewritten, _, _, _) =
			style::apply_fix_passes(&path, original, true).expect("Apply fixes.");

		fs::write(&path, &rewritten).expect("Write rewritten fixture.");

		let compiled = process::Command::new("rustc")
			.args(["--edition=2024", "--test"])
			.arg(&path)
			.arg("-o")
			.arg(&binary)
			.output()
			.expect("Compile rewritten fixture.");

		assert!(
			compiled.status.success(),
			"{rewritten}\n{}",
			String::from_utf8_lossy(&compiled.stderr)
		);

		let executed = process::Command::new(&binary).output().expect("Run rewritten fixture.");

		fs::remove_dir_all(root).expect("Remove fixture.");

		assert!(executed.status.success(), "{}", String::from_utf8_lossy(&executed.stderr));
	}

	#[test]
	fn parent_glob_expansion_preserves_method_traits() {
		let original = r#"mod toolkit {
    pub mod prelude {
        pub trait Styled { fn size_full(self) -> Self; }
        impl Styled for u8 { fn size_full(self) -> Self { self + 1 } }
        pub trait ParentElement { fn child(self) -> Self; }
        impl ParentElement for u8 { fn child(self) -> Self { self + 2 } }
    }
}
use toolkit::prelude::{Styled, ParentElement as ChildTrait};
trait Local { fn local(self) -> Self; }
impl Local for u8 { fn local(self) -> Self { self + 3 } }
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn methods() { assert_eq!(1_u8.size_full().child().local(), 7); }
}
"#;
		let now = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.expect("Timestamp.")
			.as_nanos();
		let root = env::temp_dir().join(format!("vstyle-glob-traits-{}-{now}", process::id()));
		let path = root.join("lib.rs");
		let binary = root.join("fixture");

		fs::create_dir_all(&root).expect("Create fixture directory.");

		let (rewritten, _, _, _) =
			style::apply_fix_passes(&path, original, true).expect("Apply fixes.");

		assert!(!rewritten.contains("::*"), "{rewritten}");

		fs::write(&path, &rewritten).expect("Write rewritten fixture.");

		let compiled = process::Command::new("rustc")
			.args(["--edition=2024", "--test"])
			.arg(&path)
			.arg("-o")
			.arg(&binary)
			.output()
			.expect("Compile rewritten fixture.");

		assert!(
			compiled.status.success(),
			"{rewritten}\n{}",
			String::from_utf8_lossy(&compiled.stderr)
		);

		let executed = process::Command::new(&binary).output().expect("Run rewritten fixture.");

		assert!(executed.status.success(), "{}", String::from_utf8_lossy(&executed.stderr));
		assert!(String::from_utf8_lossy(&executed.stdout).contains("1 passed;"));

		fs::remove_dir_all(root).expect("Remove fixture.");
	}

	#[test]
	fn parent_glob_inventory_ignores_qualified_import_leaves() {
		let original = "struct Error;\n#[cfg(test)]\nmod tests {\n\tuse super::*;\n\t#[test]\n\tfn sample() {\n\t\tuse std::fmt::{Error as Source};\n\t\tlet _ = Source;\n\t}\n}\n";
		let parsed = ra_ap_syntax::SourceFile::parse(original, ra_ap_syntax::Edition::CURRENT);
		let glob = parsed
			.tree()
			.syntax()
			.descendants()
			.filter_map(Use::cast)
			.find(|item| item.syntax().text().to_string().contains("super::*"))
			.expect("Find parent glob.");
		let symbols =
			imports::exported_symbols_from_super_scope(&glob).expect("Read parent inventory.");

		assert!(symbols.is_empty(), "{symbols:?}");
	}

	#[test]
	fn parent_glob_expansion_retains_nested_use_dependencies() {
		let now = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.expect("Timestamp.")
			.as_nanos();
		let root = env::temp_dir().join(format!("vstyle-glob-dependency-{}-{now}", process::id()));
		let path = root.join("lib.rs");

		fs::create_dir_all(&root).expect("Create fixture directory.");

		for local_import in [
			"use ErrorKind as Source;",
			"use {ErrorKind as Source};",
			"use ErrorKind::{NotFound as Selected};",
		] {
			let value =
				if local_import.contains("Selected") { "Selected" } else { "Source::NotFound" };
			let original = format!(
				"use std::io::ErrorKind;\n#[cfg(test)]\nmod tests {{\n\tuse super::*;\n\t#[test]\n\tfn sample() {{\n\t\t{local_import}\n\t\tlet _ = {value};\n\t}}\n}}\n"
			);
			let (rewritten, _, _, _) =
				style::apply_fix_passes(&path, &original, true).expect("Apply fixes.");

			fs::write(&path, &rewritten).expect("Write rewritten fixture.");

			let output = process::Command::new("rustc")
				.args(["--edition=2024", "--test", "--emit=metadata"])
				.arg(&path)
				.arg("-o")
				.arg(root.join("fixture.rmeta"))
				.output()
				.expect("Compile rewritten fixture.");

			assert!(
				output.status.success(),
				"{rewritten}\n{}",
				String::from_utf8_lossy(&output.stderr)
			);
		}

		fs::remove_dir_all(root).expect("Remove fixture.");
	}

	#[test]
	fn import004_preserves_explicit_call_type_and_const_arguments() {
		let original = "use std::mem::{size_of, align_of};\nuse std::array::from_fn;\nfn main() { let size = size_of::<[u16; 3]>(); let alignment = align_of::<u16>(); let array = from_fn::<u8, 3, _>(|i| i as u8); assert_eq!(size, 6); assert_eq!(alignment, 2); assert_eq!(array, [0, 1, 2]); assert_eq!(size_of::<u32>(), 4); }\n";
		let now = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.expect("Timestamp.")
			.as_nanos();
		let root = env::temp_dir().join(format!("vstyle-call-generics-{}-{now}", process::id()));
		let path = root.join("main.rs");
		let binary = root.join("fixture");

		fs::create_dir_all(&root).expect("Create fixture directory.");

		let (rewritten, _, _, _) =
			style::apply_fix_passes(&path, original, true).expect("Apply fixes.");

		fs::write(&path, &rewritten).expect("Write rewritten fixture.");

		let compiled = process::Command::new("rustc")
			.arg("--edition=2024")
			.arg(&path)
			.arg("-o")
			.arg(&binary)
			.output()
			.expect("Compile rewritten fixture.");

		assert!(
			compiled.status.success(),
			"{rewritten}\n{}",
			String::from_utf8_lossy(&compiled.stderr)
		);

		let executed = process::Command::new(&binary).output().expect("Run rewritten fixture.");

		fs::remove_dir_all(root).expect("Remove fixture.");

		assert!(executed.status.success(), "{}", String::from_utf8_lossy(&executed.stderr));
	}

	#[test]
	fn import_hoisting_precedes_qualified_path_shortening() {
		let original = "fn sample(_: std::io::ErrorKind) { use std::io::{ErrorKind, SeekFrom}; let _ = ErrorKind::NotFound; let _ = SeekFrom::Start(0); }\n";
		let now = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.expect("Timestamp.")
			.as_nanos();
		let root = env::temp_dir().join(format!("vstyle-group-hoist-{}-{now}", process::id()));
		let path = root.join("lib.rs");

		fs::create_dir_all(&root).expect("Create fixture directory.");

		let (rewritten, _, _, _) =
			style::apply_fix_passes(&path, original, true).expect("Apply fixes.");

		fs::write(&path, &rewritten).expect("Write rewritten fixture.");

		let output = process::Command::new("rustc")
			.args(["--edition=2024", "--crate-type=lib", "--emit=metadata"])
			.arg(&path)
			.arg("-o")
			.arg(root.join("fixture.rmeta"))
			.output()
			.expect("Compile rewritten fixture.");

		fs::remove_dir_all(root).expect("Remove fixture.");

		assert!(
			output.status.success(),
			"{rewritten}\n{}",
			String::from_utf8_lossy(&output.stderr)
		);
	}

	#[test]
	fn import_removal_qualifies_dependent_nested_use_roots() {
		let now = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.expect("Timestamp.")
			.as_nanos();
		let root = env::temp_dir().join(format!("vstyle-nested-use-{}-{now}", process::id()));
		let path = root.join("lib.rs");

		fs::create_dir_all(&root).expect("Create fixture directory.");

		for original in [
			"use std::io::ErrorKind as Kind;\nfn sample() { use Kind as Source; let _ = Source::NotFound; }\n",
			"use std::io::ErrorKind;\nfn sample(value: ErrorKind) -> bool { use ErrorKind as Source; matches!(value, Source::NotFound) }\n#[cfg(unix)] fn qualified(_: std::io::ErrorKind) {}\n",
			"use std::io::ErrorKind as Kind;\nfn sample() { use {Kind as Source}; let _ = Source::NotFound; }\n",
			"use std::io::ErrorKind as Kind;\nfn sample() { use Kind::{NotFound}; let _ = NotFound; }\n",
			"use std::io::ErrorKind as Kind;\nfn outer(_: Kind) {}\nfn sample() { use std::option::Option as Kind; use Kind as Source; let _ = Source::Some(1); }\n",
		] {
			let (rewritten, _, _, _) =
				style::apply_fix_passes(&path, original, true).expect("Apply fixes.");

			fs::write(&path, &rewritten).expect("Write rewritten fixture.");

			let output = process::Command::new("rustc")
				.args(["--edition=2024", "--crate-type=lib", "--emit=metadata"])
				.arg(&path)
				.arg("-o")
				.arg(root.join("fixture.rmeta"))
				.output()
				.expect("Compile rewritten fixture.");

			assert!(
				output.status.success(),
				"{rewritten}\n{}",
				String::from_utf8_lossy(&output.stderr)
			);
		}

		fs::remove_dir_all(root).expect("Remove fixture.");
	}

	#[test]
	fn import003_preserves_aliases_used_by_bare_patterns() {
		let original = "use libc::ESRCH as S;\nfn sample(code: Option<i32>) -> bool { match code { Some(S) => true, _ => false } }\n";
		let ctx =
			shared::read_file_context_from_text(Path::new("constant_alias.rs"), original.into())
				.expect("Read context.")
				.expect("Have context.");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-003" && !v.fixable));
		assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-003"));
	}

	#[test]
	fn import009_preserves_imports_used_by_bare_patterns() {
		let original = "use libc::EPERM;\nuse libc::ESRCH;\nfn plain(code: Option<i32>) -> bool { match code { Some(ESRCH) => true, Some(EPERM) => false, _ => false } }\n#[cfg(unix)]\nfn guarded(code: Option<i32>) -> bool { match code { Some(libc::ESRCH) => true, Some(libc::EPERM) => false, _ => false } }\n";
		let ctx =
			shared::read_file_context_from_text(Path::new("constant_patterns.rs"), original.into())
				.expect("Read context.")
				.expect("Have context.");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009" && !v.fixable));
		assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-009"));
	}

	#[test]
	fn import003_preserves_function_local_alias_shadowing() {
		let original = r#"use std::io::ErrorKind as Phase;
pub fn outer() -> Phase { Phase::NotFound }
pub fn inner() -> Option<u8> {
    let value = Phase::Some(1);
    use std::option::Option as Phase;
    assert!(matches!(value, Phase::Some(_)));
    value
}
"#;
		let (rewritten, _, _, _) =
			style::apply_fix_passes(Path::new("shadowed_alias.rs"), original, true)
				.expect("Apply fixes.");

		assert!(!rewritten.contains("ErrorKind::Some"), "{rewritten}");
		assert!(rewritten.contains("Phase::Some(1)"), "{rewritten}");
		assert!(rewritten.contains("value, Phase::Some(_)"), "{rewritten}");
		assert!(!rewritten.contains("Phase::NotFound"), "{rewritten}");
	}

	#[test]
	fn import003_qualifies_tuple_constructor_patterns() {
		let original = r#"use std::result::Result as Receipt;
fn sample(value: Receipt<u8, u8>) -> u8 {
    if let Receipt::Ok(inner) = value { return inner; }
    let Receipt::Err(inner) = value else { return 0; };
    match value { Receipt::Ok(other) | Receipt::Err(other) => inner + other }
}
"#;
		let (rewritten, _, _, _) =
			style::apply_fix_passes(Path::new("tuple_patterns.rs"), original, true)
				.expect("Apply fixes.");

		assert!(!rewritten.contains("Receipt"), "{rewritten}");
		assert!(rewritten.contains("::Ok(inner)"), "{rewritten}");
		assert!(rewritten.contains("::Err(inner)"), "{rewritten}");
	}

	#[test]
	fn import003_rewrites_macro_alias_paths_without_touching_literals() {
		let original = r#"use crate::state::Status as S;
fn sample(value: crate::state::Status) {
    assert!(matches!(value, S::Ready), "S::Ready must remain literal");
}
"#;
		let (rewritten, _, _, _) =
			style::apply_fix_passes(Path::new("macro_alias.rs"), original, true)
				.expect("Apply fixes.");

		assert!(!rewritten.contains("value, S::Ready"), "{rewritten}");
		assert!(rewritten.contains("\"S::Ready must remain literal\""), "{rewritten}");
	}

	#[test]
	fn import006_preserves_conditional_expression_imports() {
		for attr in ["#[cfg(any())]", "#[cfg_attr(all(), cfg(any()))]", "#[cfg (any())]"] {
			let text =
				format!("fn sample() {{ {attr} {{ use missing::Feature; let _ = Feature; }} }}");
			let ctx = shared::read_file_context_from_text(Path::new("conditional_block.rs"), text)
				.expect("Read context.")
				.expect("Have context.");
			let (violations, edits) = style::collect_violations(&ctx, true);

			assert!(
				violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-006" && !v.fixable),
				"{attr}"
			);
			assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-006"), "{attr}");
		}
	}

	#[test]
	fn import006_alias_local_use_is_reported_non_fixable() {
		let text = r#"
fn sample() {
	use std::collections::BTreeMap as Map;

	let _ = Map::<u8, u8>::new();
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import006_alias_local_use.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-006"
				&& v.message == "Use items must appear only at file top level or module top level."
				&& !v.fixable
		}));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-006"));
	}

	#[test]
	fn import006_glob_local_use_is_reported_non_fixable() {
		let text = r#"
fn sample() {
	use crate::math::*;

	let _ = 1usize;
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import006_glob_local_use.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-006"
				&& v.message == "Use items must appear only at file top level or module top level."
				&& !v.fixable
		}));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-006"));
	}

	#[test]
	fn import006_leading_comment_local_use_is_reported_non_fixable() {
		let text = r#"
fn sample() {
	// Keep this local until cfg behavior is sorted out.
	use std::collections::BTreeMap;

	let _ = BTreeMap::<u8, u8>::new();
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import006_leading_comment_local_use.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-006"
				&& v.message == "Use items must appear only at file top level or module top level."
				&& !v.fixable
		}));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-006"));
	}

	#[test]
	fn import004_qualifies_function_calls_after_macro_labels() {
		let original = "use crate::geometry::point;\nfn sample() { let _ = point(1); wrapper!(origin: point(2)); }";
		let (rewritten, _, _, _) =
			style::apply_fix_passes(Path::new("macro_labels.rs"), original, true)
				.expect("Apply fixes.");

		assert!(rewritten.contains("origin: geometry::point(2)"), "{rewritten}");
		assert!(!rewritten.contains("geometry::geometry::"), "{rewritten}");
	}

	#[test]
	fn import004_reuses_module_bindings_planned_for_other_imports() {
		let original = r#"pub fn observe() {}
pub mod conversations {
    pub mod tests {
        pub fn connected() {}
        pub fn dispatched() {}
        pub fn archived() {}
        pub fn replied() {}
    }
}
#[cfg(test)]
mod tests {
    use super::observe;
    use crate::conversations::tests::{connected, dispatched};
    #[test]
    fn connects_session() {
        observe();
        connected();
        dispatched();
    }
    #[test]
    fn archives_session() {
        use crate::conversations::tests::{archived, replied};
        archived();
        replied();
    }
}
"#;
		let now = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.expect("Timestamp.")
			.as_nanos();
		let root = env::temp_dir().join(format!("vstyle-shared-plan-{}-{now}", process::id()));
		let path = root.join("src/lib.rs");

		fs::create_dir_all(root.join("src")).expect("Create fixture directory.");
		fs::write(
			root.join("Cargo.toml"),
			"[package]\nname='glob-plan'\nversion='0.0.0'\nedition='2024'\n",
		)
		.expect("Write manifest.");
		fs::write(&path, original).expect("Write fixture.");

		let (rewritten, _, _, _) =
			style::apply_fix_passes(&path, original, true).expect("Apply fixes.");

		assert!(rewritten.contains("tests::connected()"), "{rewritten}");

		fs::write(&path, &rewritten).expect("Write rewritten fixture.");

		let output = process::Command::new("rustc")
			.arg("--edition=2024")
			.arg("--test")
			.arg("--emit=metadata")
			.arg(&path)
			.arg("-o")
			.arg(root.join("fixture.rmeta"))
			.output()
			.expect("Compile rewritten fixture.");

		fs::remove_dir_all(root).expect("Remove fixture.");

		assert!(
			output.status.success(),
			"{rewritten}\n{}",
			String::from_utf8_lossy(&output.stderr)
		);
	}

	#[test]
	fn import007_coordinates_expansion_with_test_module_paths() {
		let original = "pub mod helper { pub fn run() {} }\n#[cfg(test)]\nmod tests {\n use super::*;\n #[test] fn calls_helper() { super::helper::run(); }\n}\n";
		let now = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.expect("Timestamp.")
			.as_nanos();
		let root = env::temp_dir().join(format!("vstyle-glob-plan-{}-{now}", process::id()));
		let path = root.join("src/lib.rs");

		fs::create_dir_all(root.join("src")).expect("Create fixture directory.");
		fs::write(
			root.join("Cargo.toml"),
			"[package]\nname='glob-plan'\nversion='0.0.0'\nedition='2024'\n",
		)
		.expect("Write manifest.");
		fs::write(&path, original).expect("Write fixture.");

		let (rewritten, _, _, _) =
			style::apply_fix_passes(&path, original, true).expect("Apply fixes.");

		assert!(!rewritten.contains("use super::*"), "{rewritten}");

		fs::write(&path, &rewritten).expect("Write rewritten fixture.");

		let output = process::Command::new("rustc")
			.arg("--edition=2024")
			.arg("--test")
			.arg("--emit=metadata")
			.arg(&path)
			.arg("-o")
			.arg(root.join("fixture.rmeta"))
			.output()
			.expect("Compile rewritten fixture.");

		fs::remove_dir_all(root).expect("Remove fixture.");

		assert!(
			output.status.success(),
			"{rewritten}\n{}",
			String::from_utf8_lossy(&output.stderr)
		);
	}

	#[test]
	fn import004_preserves_relative_parent_depth() {
		let original =
			"fn sample() { super::super::render(); wrapper!(super::super::super::render()); }";
		let (rewritten, _, _, _) =
			style::apply_fix_passes(Path::new("relative_parent.rs"), original, true)
				.expect("Apply fixes.");

		assert!(rewritten.contains("super::super::render()"), "{rewritten}");
		assert!(rewritten.contains("super::super::super::render()"), "{rewritten}");
		assert!(!rewritten.contains("use super"), "{rewritten}");
	}

	#[test]
	fn import004_checks_relative_module_binding_collisions() {
		let original =
			"use crate::conversations::tests;\nfn sample() { super::super::tests::profiles(); }";
		let (rewritten, _, _, _) =
			style::apply_fix_passes(Path::new("relative_collision.rs"), original, true)
				.expect("Apply fixes.");

		assert!(rewritten.contains("super::super::tests::profiles()"), "{rewritten}");
		assert!(!rewritten.contains("use super"), "{rewritten}");
	}

	#[cfg(unix)]
	#[test]
	fn import004_coordinates_direct_imports_and_shortened_paths() {
		let original = r#"#[cfg(test)]
mod tests {
    use std::os::unix::fs::{MetadataExt, symlink};
    #[test]
    fn sample() {
        let _ = std::fs::read("file");
        let _ = std::fs::metadata("file").map(|metadata| metadata.mode());
        let _ = symlink("a", "b");
    }
}
"#;
		let now = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.expect("Timestamp.")
			.as_nanos();
		let root = env::temp_dir().join(format!("vstyle-planned-imports-{}-{now}", process::id()));

		fs::create_dir_all(&root).expect("Create fixture directory.");

		let path = root.join("fixture.rs");
		let (rewritten, _, _, _) =
			style::apply_fix_passes(&path, original, true).expect("Apply fixes.");

		assert!(rewritten.contains("fs::symlink(\"a\", \"b\")"), "{rewritten}");

		fs::write(&path, &rewritten).expect("Write rewritten fixture.");

		let output = process::Command::new("rustc")
			.arg("--edition=2024")
			.arg("--test")
			.arg("--emit=metadata")
			.arg(&path)
			.arg("-o")
			.arg(root.join("fixture.rmeta"))
			.output()
			.expect("Compile rewritten fixture.");

		fs::remove_dir_all(root).expect("Remove fixture.");

		assert!(
			output.status.success(),
			"{rewritten}\n{}",
			String::from_utf8_lossy(&output.stderr)
		);
	}

	#[test]
	fn import006_does_not_hoist_colliding_pending_bindings() {
		let original = "fn first() {\n use std::fmt::Result;\n let _: Result = Ok(());\n}\nfn second() {\n use std::io::Result;\n let _: Result<()> = Ok(());\n}\n";
		let ctx = shared::read_file_context_from_text(
			Path::new("pending_imports.rs"),
			original.to_owned(),
		)
		.expect("Read context.")
		.expect("Have context.");
		let (_, edits) = style::collect_violations(&ctx, true);
		let edits = edits.into_iter().filter(|edit| edit.rule == "RUST-STYLE-IMPORT-006").collect();
		let mut rewritten = original.to_owned();

		fixes::apply_edits(&mut rewritten, edits).expect("Apply hoists.");

		assert!(rewritten.contains(" use std::io::Result;"), "{rewritten}");
		assert!(rewritten.starts_with("use std::fmt::Result;"), "{rewritten}");
	}

	#[test]
	fn import006_pub_use_conflict_is_reported_non_fixable() {
		let text = r#"
pub use crate::first::Thing;

fn sample() {
	use crate::second::Thing;

	let _ = 1usize;
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import006_pub_use_conflict.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-006"
				&& v.message == "Use items must appear only at file top level or module top level."
				&& !v.fixable
		}));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-006"));
	}

	#[test]
	fn import006_keeps_cfg_test_module_top_level_use_valid() {
		let text = r#"
#[cfg(test)]
mod tests {
	use std::collections::BTreeMap;

	#[test]
	fn sample() {
		let _ = BTreeMap::<u8, u8>::new();
	}
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import006_cfg_test_module_scope.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-006"));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-006"));
	}

	#[test]
	fn import_fixes_qualify_nested_macros_without_changing_literal_text() {
		let original = r#"
use crate::metrics::emit;
fn sample() {
    emit!(emit!("emit!()"), crate::metrics::emit!("qualified"));
    wrapper!(selector: emit!(), qualified: crate::metrics::emit!());
}
"#;
		let (rewritten, _, _, _) =
			style::apply_fix_passes(Path::new("nested_macro.rs"), original, true)
				.expect("Apply fixes.");

		assert!(rewritten.contains("metrics::emit!(metrics::emit!(\"emit!()\")"), "{rewritten}");
		assert!(!rewritten.contains("metrics::metrics::"), "{rewritten}");
		assert!(rewritten.contains("selector: metrics::emit!()"), "{rewritten}");
	}

	#[test]
	fn import_fixes_preserve_bindings_imported_by_child_modules() {
		for (file, child_import) in [
			("src/lib.rs", "super::emit"),
			("src/lib.rs", "super::*"),
			("src/lib.rs", "crate::emit"),
			("src/parent.rs", "crate::parent::emit"),
		] {
			let original = format!(
				"use crate::metrics::emit;\nfn sample() {{ emit!(); }}\n#[cfg(test)]\nmod tests {{\n    use {child_import};\n    fn child() {{ emit!(); }}\n}}\n"
			);
			let (rewritten, _, _, _) =
				style::apply_fix_passes(Path::new(file), &original, true).expect("Apply fixes.");

			assert!(rewritten.contains("use crate::metrics::emit;"), "{rewritten}");
		}
	}

	#[test]
	fn import_fixes_keep_glob_symbols_used_only_inside_macros() {
		let original = r#"
enum StoreError { Conflict }
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sample() {
        assert!(matches!(None::<StoreError>, Some(StoreError::Conflict)));
    }
}
"#;
		let (rewritten, _, _, _) =
			style::apply_fix_passes(Path::new("src/lib.rs"), original, true).expect("Apply fixes.");

		assert!(rewritten.contains("use crate::{StoreError};"), "{rewritten}");
	}

	#[test]
	fn import_rules_ignore_paths_inside_macro_string_literals() {
		let original = r##"fn sample() {
    assert_eq!("std::io::empty()", r#"std::io::empty()"#);
    assert_eq!("foo::Bar { x: 1 }", "foo::Bar { x: 1 }");
}"##;
		let ctx =
			shared::read_file_context_from_text(Path::new("macro_strings.rs"), original.to_owned())
				.expect("Read context.")
				.expect("Have context.");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(
			!violations.iter().any(|v| v.rule.starts_with("RUST-STYLE-IMPORT-")),
			"{violations:#?}"
		);
		assert!(
			!edits.iter().any(|edit| edit.rule.starts_with("RUST-STYLE-IMPORT-")),
			"{edits:#?}"
		);
	}

	#[test]
	fn import_edits_preserve_bindings_when_rules_overlap() {
		let original =
			"use std::io::Read;\n\nfn sample() {\n    let _ = std::io::empty().read(&mut []);\n}\n";
		let path = Path::new("import_overlap.rs");
		let ctx = shared::read_file_context_from_text(path, original.to_owned())
			.expect("Read context.")
			.expect("Have context.");
		let (_, edits) = style::collect_violations(&ctx, true);
		let mut rewritten = original.to_owned();

		fixes::apply_edits(&mut rewritten, edits).expect("Apply conflicting import rules.");

		assert!(
			rewritten.contains("std::io::empty()") || rewritten.contains("use std::io::{self,"),
			"{rewritten}"
		);

		let (finished, _, _, _) =
			style::apply_fix_passes(path, original, true).expect("Finish rewrites.");

		assert!(!finished.contains("std::io::empty()"), "{finished}");
		assert!(
			finished.contains("use std::io::{self,") || finished.contains("use std::io;"),
			"{finished}"
		);
	}

	#[test]
	fn import_fix_qualifies_unqualified_macro_calls() {
		let original = r#"
use crate::metrics::emit;

fn sample() {
	emit!("ok");
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import004_macro_fix.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-004" && v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-004"));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 2);
		assert!(!rewritten.contains("use crate::metrics::emit;"));
		assert!(rewritten.contains("use crate::metrics;"));
		assert!(rewritten.contains("metrics::emit!(\"ok\")"));
	}

	#[test]
	fn import_fix_rewrites_braced_use_tree_member() {
		let original = r#"
use super::shared::{Edit, line_from_offset, offset_from_line};

fn sample() {
	let _ = line_from_offset();
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import004_braced_fix.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-004" && v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-004"));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 2);
		assert!(rewritten.contains("shared::line_from_offset()"));
		assert!(rewritten.contains("use super::shared::{self, Edit, offset_from_line};"));
		assert!(
			!rewritten.contains("use super::shared::{Edit, line_from_offset, offset_from_line};")
		);
	}

	#[test]
	fn import004_fix_prefers_parent_module_keep_alive_style_for_eyre() {
		let original = r#"
use color_eyre::eyre::{Context as _, Result, eyre};

fn sample() -> Result<()> {
	Err(eyre!("boom"))
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import004_eyre_parent_module_fix.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-004" && v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-004"));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 2);
		assert!(
			rewritten.contains("use color_eyre::eyre::{self, Context as _, Result};"),
			"{rewritten}"
		);
		assert!(rewritten.contains("Err(eyre::eyre!(\"boom\"))"));
		assert!(!rewritten.contains("use color_eyre::eyre::{Context as _, Result, eyre};"));
	}

	#[test]
	fn import008_fix_imports_unambiguous_type_paths_and_keeps_group_order() {
		let original = r#"
use std::collections::HashSet;
use crate::local::Marker;

fn run<'e, E>(_exec: E)
where
	E: sqlx::Executor<'e>,
{
	let _ = HashSet::<usize>::new();
	let _ = Marker;
}
"#;
		let mut rewritten = original.to_owned();

		for _ in 0..4 {
			let ctx = shared::read_file_context_from_text(
				Path::new("import008_unambiguous.rs"),
				rewritten.clone(),
			)
			.expect("context")
			.expect("has ctx");
			let (_violations, edits) = style::collect_violations(&ctx, true);

			if edits.is_empty() {
				break;
			}

			let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		}

		assert!(rewritten.contains("use sqlx::Executor;"));
		assert!(rewritten.contains("E: Executor<'e>,"));

		let std_idx = rewritten.find("use std::collections::HashSet;").expect("std");
		let third_party_idx = rewritten.find("use sqlx::Executor;").expect("third-party");
		let self_idx = rewritten.find("use crate::local::Marker;").expect("self");

		assert!(std_idx < third_party_idx);
		assert!(third_party_idx < self_idx);
	}

	#[test]
	fn import008_shortens_same_name_module_macro_paths() {
		let original = r#"
use color_eyre::eyre::{Result};

fn build() -> Result<()> {
	Err(color_eyre::eyre::eyre!("boom"))
}
"#;
		let mut rewritten = original.to_owned();

		for _ in 0..4 {
			let ctx = shared::read_file_context_from_text(
				Path::new("import008_macro_module_shorten.rs"),
				rewritten.clone(),
			)
			.expect("context")
			.expect("has ctx");
			let (_violations, edits) = style::collect_violations(&ctx, true);

			if edits.is_empty() {
				break;
			}

			let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		}

		assert!(rewritten.contains("eyre::eyre!(\"boom\")"));
		assert!(!rewritten.contains("color_eyre::eyre::eyre!(\"boom\")"));

		let use_tree_start =
			rewritten.find("use color_eyre::eyre::{").expect("must keep color_eyre::eyre use tree");
		let use_tree_end = rewritten[use_tree_start..]
			.find("};")
			.expect("must keep color_eyre::eyre use tree end");
		let use_tree = &rewritten[use_tree_start..use_tree_start + use_tree_end + 2];

		assert!(use_tree.contains("self"));
		assert!(use_tree.contains("Result"));
		assert!(!rewritten.contains("use color_eyre::eyre;"));
	}

	#[test]
	fn import008_fix_keeps_module_import_for_same_name_module_macro() {
		let original = r#"
use color_eyre::eyre::{Result};

fn build() -> Result<()> {
	Err(color_eyre::eyre::eyre!("boom"))
}
"#;
		let mut rewritten = original.to_owned();

		for _ in 0..4 {
			let ctx = shared::read_file_context_from_text(
				Path::new("import008_macro_module_fix.rs"),
				rewritten.clone(),
			)
			.expect("context")
			.expect("has ctx");
			let (_violations, edits) = style::collect_violations(&ctx, true);

			if edits.is_empty() {
				break;
			}

			let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		}

		assert!(rewritten.contains("eyre::eyre!(\"boom\")"));
		assert!(!rewritten.contains("color_eyre::eyre::eyre!(\"boom\")"));

		let use_tree_start =
			rewritten.find("use color_eyre::eyre::{").expect("must keep color_eyre::eyre use tree");
		let use_tree_end = rewritten[use_tree_start..]
			.find("};")
			.expect("must keep color_eyre::eyre use tree end");
		let use_tree = &rewritten[use_tree_start..use_tree_start + use_tree_end + 2];

		assert!(use_tree.contains("self"));
		assert!(use_tree.contains("Result"));
		assert!(!rewritten.contains("use color_eyre::eyre;"));
	}

	#[test]
	fn import008_merges_same_name_module_macro_into_existing_braced_use_tree() {
		let original = r#"
use color_eyre::eyre::{Result};

fn build() -> Result<()> {
	Err(color_eyre::eyre::eyre!("boom"))
}
"#;
		let mut rewritten = original.to_owned();

		for _ in 0..4 {
			let ctx = shared::read_file_context_from_text(
				Path::new("import008_macro_module_merge.rs"),
				rewritten.clone(),
			)
			.expect("context")
			.expect("has ctx");
			let (_violations, edits) = style::collect_violations(&ctx, true);

			if edits.is_empty() {
				break;
			}

			let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		}

		assert!(rewritten.contains("eyre::eyre!(\"boom\")"));
		assert!(!rewritten.contains("color_eyre::eyre::eyre!(\"boom\")"));

		let use_tree_start =
			rewritten.find("use color_eyre::eyre::{").expect("must keep color_eyre::eyre use tree");
		let use_tree_end = rewritten[use_tree_start..]
			.find("};")
			.expect("must keep color_eyre::eyre use tree end");
		let use_tree = &rewritten[use_tree_start..use_tree_start + use_tree_end + 2];

		assert!(use_tree.contains("self"));
		assert!(use_tree.contains("Result"));
		assert!(!rewritten.contains("use color_eyre::eyre;"));
	}

	#[test]
	fn import008_skips_ambiguous_type_symbol_paths() {
		let text = r#"
fn left<'e, E>(_exec: E)
where
	E: foo::Executor<'e>,
{
}

fn right<'e, E>(_exec: E)
where
	E: bar::Executor<'e>,
{
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import008_ambiguous.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-008"));
		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009"));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-008"));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-009"));
	}

	#[test]
	fn import008_keeps_lowercase_ffi_type_function_collision_fully_qualified() {
		let original = r#"
fn install(signal: i32) {
	let mut action: libc::sigaction = unsafe { std::mem::zeroed() };

	unsafe {
		libc::sigaction(signal, &raw const action, std::ptr::null_mut());
	}
}
"#;
		let path = Path::new("import008_lowercase_ffi_type_function_collision.rs");
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(path, original, true).expect("apply fix passes");

		assert!(!rewritten.contains("use libc::sigaction;"), "{rewritten}");
		assert!(rewritten.contains("action: libc::sigaction"), "{rewritten}");
		assert!(rewritten.contains("libc::sigaction(signal"), "{rewritten}");

		let ctx = shared::read_file_context_from_text(path, rewritten.clone())
			.expect("context")
			.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|violation| {
			matches!(violation.rule, "RUST-STYLE-IMPORT-008" | "RUST-STYLE-IMPORT-009")
		}));
		assert!(!edits.iter().any(|edit| {
			matches!(edit.rule, "RUST-STYLE-IMPORT-008" | "RUST-STYLE-IMPORT-009")
		}));

		let (repeated, applied_count, _, _) =
			style::apply_fix_passes(path, &rewritten, true).expect("repeat fix passes");

		assert_eq!(applied_count, 0);
		assert_eq!(repeated, rewritten);
	}

	#[test]
	fn import008_merges_alias_child_import_into_existing_parent_module_use() {
		let original = r#"
use futures::channel::mpsc;

fn send(tx: mpsc::UnboundedSender<u8>) {
	let _ = tx;
}
"#;
		let mut rewritten = original.to_owned();

		for _ in 0..4 {
			let ctx = shared::read_file_context_from_text(
				Path::new("import008_merge_alias_child.rs"),
				rewritten.clone(),
			)
			.expect("context")
			.expect("has ctx");
			let (_violations, edits) = style::collect_violations(&ctx, true);

			if edits.is_empty() {
				break;
			}

			let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		}

		assert!(rewritten.contains("use futures::channel::mpsc::{self, UnboundedSender};"));
		assert!(rewritten.contains("fn send(tx: UnboundedSender<u8>)"));
		assert!(!rewritten.contains("use mpsc::UnboundedSender;"));
	}

	#[test]
	fn import008_merges_children_into_existing_nested_use_tree() {
		let original = r#"
use pubfi_extractor::{
	TextIndex,
	asset::{self, AssetRegistry},
	chain_object, mentions,
	product::{self, ProductRegistry},
};

fn demo(
	_i: TextIndex,
	_m: mentions::Mentions,
	a: asset::AssetMention,
	b: chain_object::ChainObjectMention,
	c: product::ProductMention,
) {
	let _ = (a, b, c);
}
"#;
		let mut rewritten = original.to_owned();

		for _ in 0..4 {
			let ctx = shared::read_file_context_from_text(
				Path::new("import008_merge_nested_use_tree.rs"),
				rewritten.clone(),
			)
			.expect("context")
			.expect("has ctx");
			let (_violations, edits) = style::collect_violations(&ctx, true);

			if edits.is_empty() {
				break;
			}

			let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		}

		let compact = rewritten.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();

		assert!(compact.contains("asset::{"));
		assert!(compact.contains("AssetRegistry"));
		assert!(compact.contains("AssetMention"));
		assert!(compact.contains("chain_object::{self,ChainObjectMention}"), "{rewritten}");
		assert!(compact.contains("product::{"));
		assert!(compact.contains("ProductRegistry"));
		assert!(compact.contains("ProductMention"));
		assert!(compact.contains("a:AssetMention"));
		assert!(compact.contains("b:ChainObjectMention"));
		assert!(compact.contains("c:ProductMention"));
		assert!(!compact.contains("useasset::AssetMention;"));
		assert!(!compact.contains("usechain_object::ChainObjectMention;"));
		assert!(!compact.contains("useproduct::ProductMention;"));
	}

	#[test]
	fn import008_recovers_short_child_imports_into_existing_parent_use_tree() {
		let original = r#"
use asset::AssetMention;
use chain_object::ChainObjectMention;
use product::ProductMention;

use pubfi_extractor::{
	TextIndex,
	asset::{self, AssetRegistry},
	chain_object, mentions,
	product::{self, ProductRegistry},
};

fn demo(
	_i: TextIndex,
	_m: mentions::Mentions,
	a: AssetMention,
	b: ChainObjectMention,
	c: ProductMention,
) {
	let _ = (a, b, c);
}
"#;
		let mut rewritten = original.to_owned();

		for _ in 0..4 {
			let ctx = shared::read_file_context_from_text(
				Path::new("import008_recover_short_child_use.rs"),
				rewritten.clone(),
			)
			.expect("context")
			.expect("has ctx");
			let (_violations, edits) = style::collect_violations(&ctx, true);

			if edits.is_empty() {
				break;
			}

			let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		}

		let compact = rewritten.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();

		assert!(compact.contains("asset::{"));
		assert!(compact.contains("AssetRegistry"));
		assert!(compact.contains("AssetMention"));
		assert!(compact.contains("chain_object::{self,ChainObjectMention}"), "{rewritten}");
		assert!(compact.contains("product::{"));
		assert!(compact.contains("ProductRegistry"));
		assert!(compact.contains("ProductMention"));
		assert!(!compact.contains("useasset::AssetMention;"));
		assert!(!compact.contains("usechain_object::ChainObjectMention;"));
		assert!(!compact.contains("useproduct::ProductMention;"));
	}

	#[test]
	fn import008_skips_cfg_test_module_paths() {
		let text = r#"
#[cfg(test)]
mod tests {
	fn sample<'e, E>(_exec: E)
	where
		E: sqlx::Executor<'e>,
	{
	}
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import008_cfg_test.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-008"));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-008"));
	}

	#[test]
	fn import008_prefers_imported_symbol_over_redundant_qualified_type_path() {
		let original = r#"
use shared::{Edit, Violation};

fn demo(v: Vec<shared::Violation>) -> Option<shared::Violation> {
	let _ = Edit { start: 0, end: 0, replacement: String::new(), rule: "R" };

	v.into_iter().next()
}
"#;
		let mut rewritten = original.to_owned();

		for _ in 0..4 {
			let ctx = shared::read_file_context_from_text(
				Path::new("import008_prefers_imported_symbol.rs"),
				rewritten.clone(),
			)
			.expect("context")
			.expect("has ctx");
			let (_violations, edits) = style::collect_violations(&ctx, true);

			if edits.is_empty() {
				break;
			}

			let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		}

		assert!(rewritten.contains("Vec<Violation>"));
		assert!(rewritten.contains("Option<Violation>"));
		assert!(!rewritten.contains("Vec<shared::Violation>"));
		assert!(!rewritten.contains("Option<shared::Violation>"));
	}

	#[test]
	fn import008_shortens_type_like_value_constructor_paths() {
		let original = r#"
fn sort(values: &mut [usize]) {
	values.sort_by_key(|value| std::cmp::Reverse(*value));
}
"#;
		let mut rewritten = original.to_owned();

		for _ in 0..4 {
			let ctx = shared::read_file_context_from_text(
				Path::new("import008_type_like_value_constructor.rs"),
				rewritten.clone(),
			)
			.expect("context")
			.expect("has ctx");
			let (_violations, edits) = style::collect_violations(&ctx, true);

			if edits.is_empty() {
				break;
			}

			let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		}

		assert!(rewritten.contains("use std::cmp::Reverse;"), "{rewritten}");
		assert!(rewritten.contains("Reverse(*value)"), "{rewritten}");
		assert!(!rewritten.contains("std::cmp::Reverse(*value)"), "{rewritten}");
	}

	#[test]
	fn import008_shortens_type_like_value_constants_without_primitive_constants() {
		let original = r#"
mod shared {
	pub const STYLE_RULE_IDS: [&str; 0] = [];
}

fn max_value() -> usize {
	usize::MAX
}

fn print_coverage() {
	for rule in shared::STYLE_RULE_IDS {
		println!("{rule}");
	}
}
"#;
		let mut rewritten = original.to_owned();

		for _ in 0..4 {
			let ctx = shared::read_file_context_from_text(
				Path::new("import008_type_like_value_constants.rs"),
				rewritten.clone(),
			)
			.expect("context")
			.expect("has ctx");
			let (_violations, edits) = style::collect_violations(&ctx, true);

			if edits.is_empty() {
				break;
			}

			let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		}

		assert!(rewritten.contains("use shared::STYLE_RULE_IDS;"), "{rewritten}");
		assert!(rewritten.contains("for rule in STYLE_RULE_IDS"), "{rewritten}");
		assert!(rewritten.contains("usize::MAX"), "{rewritten}");
		assert!(!rewritten.contains("use usize::MAX"), "{rewritten}");
		assert!(!rewritten.contains("for rule in shared::STYLE_RULE_IDS"), "{rewritten}");
	}

	#[test]
	fn import008_skips_non_importable_self_root_paths() {
		let text = r#"
trait Job {
	type Output;
	fn run(&self) -> Self::Output;
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import008_skip_self_root.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-008"));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-008"));
	}

	#[test]
	fn import008_skips_non_importable_generic_root_paths() {
		let text = r#"
use serde::Serializer;

pub fn serialize<S>(serializer: S) -> Result<S::Ok, S::Error>
where
	S: Serializer,
{
	let _ = serializer;
	todo!()
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import008_skip_generic_root.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-008"));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-008"));
	}

	#[test]
	fn import008_skips_std_result_alias_shortening() {
		let text = r#"
pub fn run() -> std::result::Result<(), String> {
	Ok(())
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import008_skip_std_result_alias.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-008"));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-008"));
	}

	#[test]
	fn import008_keeps_io_result_qualified_when_unqualified_result_is_used() {
		let text = r#"
fn business_result() -> Result<(), i32> {
	Ok(())
}

fn read_state() -> std::io::Result<String> {
	Ok(String::new())
}

fn apply_test_override() -> Result<(), i32> {
	Ok(())
}
"#;
		let path = Path::new("import008_io_result_prelude_conflict.rs");
		let ctx = shared::read_file_context_from_text(path, text.to_owned())
			.expect("context")
			.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|violation| violation.rule == "RUST-STYLE-IMPORT-008"));
		assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-008"));

		let (rewritten, _, _, _) =
			style::apply_fix_passes(path, text, true).expect("apply fix passes");

		assert!(!rewritten.contains("use std::io::Result;"), "Rewritten:\n{rewritten}");
		assert!(
			rewritten.contains("fn read_state() -> std::io::Result<String>"),
			"Rewritten:\n{rewritten}"
		);
		assert!(
			rewritten.contains("fn apply_test_override() -> Result<(), i32>"),
			"Rewritten:\n{rewritten}"
		);
	}

	#[test]
	fn import_rules_skip_error_rs_and_do_not_add_imports() {
		let text = r#"
pub enum Error {
	Io(sqlx::Error),
}
"#;
		let ctx = shared::read_file_context_from_text(Path::new("error.rs"), text.to_owned())
			.expect("context")
			.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-008"));
		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009"));
		assert!(!edits.iter().any(|e| {
			matches!(
				e.rule,
				"RUST-STYLE-IMPORT-001"
					| "RUST-STYLE-IMPORT-002"
					| "RUST-STYLE-IMPORT-003"
					| "RUST-STYLE-IMPORT-004"
					| "RUST-STYLE-IMPORT-008"
					| "RUST-STYLE-IMPORT-009"
			)
		}));
	}

	#[test]
	fn import008_shortens_qualified_derive_path_and_inserts_use() {
		let original = r#"
#[derive(Clone, Debug, sqlx::FromRow)]
struct Row;
"#;
		let mut rewritten = original.to_owned();

		for _ in 0..4 {
			let ctx = shared::read_file_context_from_text(
				Path::new("import008_derive_row.rs"),
				rewritten.clone(),
			)
			.expect("context")
			.expect("has ctx");
			let (_violations, edits) = style::collect_violations(&ctx, true);

			if edits.is_empty() {
				break;
			}

			let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		}

		assert!(rewritten.lines().any(|line| line.trim() == "use sqlx::FromRow;"));
		assert!(rewritten.contains("#[derive(Clone, Debug, FromRow)]"));
		assert!(!rewritten.contains("#[derive(Clone, Debug, sqlx::FromRow)]"));
	}

	#[test]
	fn import008_skips_ambiguous_derive_symbol_paths() {
		let text = r#"
#[derive(sqlx::FromRow)]
struct SqlRow;

#[derive(other::FromRow)]
struct OtherRow;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import008_derive_ambiguous.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);
		let import008_violations =
			violations.iter().filter(|v| v.rule == "RUST-STYLE-IMPORT-008").count();
		let import008_edits = edits.iter().filter(|e| e.rule == "RUST-STYLE-IMPORT-008").count();

		assert_eq!(import008_violations, 0);
		assert_eq!(import008_edits, 0);
	}

	#[test]
	fn import008_derive_does_not_touch_skip_serializing_if_string_literal() {
		let original = r#"
#[serde(skip_serializing_if = "core::ops::Not::not")]
#[derive(serde::Serialize)]
struct Record {
	#[allow(dead_code)]
	value: bool,
}
"#;
		let mut rewritten = original.to_owned();

		for _ in 0..4 {
			let ctx = shared::read_file_context_from_text(
				Path::new("import008_derive_skip_serializing.rs"),
				rewritten.clone(),
			)
			.expect("context")
			.expect("has ctx");
			let (_violations, edits) = style::collect_violations(&ctx, true);

			if edits.is_empty() {
				break;
			}

			let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		}

		let skip_serializing_if_lines: Vec<&str> = rewritten
			.lines()
			.filter(|line| line.contains("skip_serializing_if"))
			.map(str::trim)
			.collect();

		assert_eq!(
			skip_serializing_if_lines.len(),
			1,
			"expected exactly one skip_serializing_if attribute"
		);
		assert_eq!(
			skip_serializing_if_lines[0],
			r#"#[serde(skip_serializing_if = "core::ops::Not::not")]"#
		);
		assert!(!rewritten.contains("use core::ops::Not;"));
		assert!(rewritten.contains("#[derive(Serialize)]"));
	}

	#[test]
	fn import008_shortens_qualified_value_receiver_path_inside_macro_and_adds_import() {
		let original = r#"
use crate::{
	agent::tracker_tool_bridge::{DynamicToolCallResponse, DynamicToolHandler, DynamicToolSpec},
};

fn build_response() -> DynamicToolCallResponse {
	let _handler = None::<DynamicToolHandler>;
	let _spec = None::<DynamicToolSpec>;

	DynamicToolCallResponse {
		content_items: vec![
			crate::agent::tracker_tool_bridge::DynamicToolContentItem::InputText {
				text: String::from("boom"),
			},
		],
		success: false,
	}
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import008_value_receiver_nested_use.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(
			rewritten.lines().any(|line| line.trim()
				== "use crate::agent::tracker_tool_bridge::DynamicToolContentItem;"),
			"{rewritten}"
		);
		assert!(rewritten.contains("DynamicToolContentItem::InputText{"), "{rewritten}");
		assert!(
			!rewritten
				.contains("crate::agent::tracker_tool_bridge::DynamicToolContentItem::InputText")
		);
	}

	#[test]
	fn import008_ignores_large_macro_noise_without_record_receiver_paths() {
		let entries = (0..128)
			.map(|idx| {
				format!(
					"Noise {{ label: helper::module::name({idx}), count: helper::count({idx}) }}"
				)
			})
			.collect::<Vec<_>>()
			.join(", ");
		let original = format!(
			r#"
fn build_noise() {{
	let _ = vec![{entries}];
}}
"#
		);
		let ctx =
			shared::read_file_context_from_text(Path::new("import008_macro_noise.rs"), original)
				.expect("context")
				.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-008"));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-008"));
	}

	#[test]
	fn import009_autofixes_when_different_qualified_symbol_path_exists() {
		let original = r#"
use a::A;

fn sample(a: A, aa: b::A) {
	let _ = (a, aa);
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import009_mixed_usage.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009" && v.fixable));
		assert!(edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-009"));

		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(!rewritten.contains("use a::A;"));
		assert!(rewritten.contains("fn sample(a: a::A, aa: b::A)"));
	}

	#[test]
	fn import009_autofixes_when_qualified_and_imported_paths_differ() {
		let original = r#"
use qdrant_client::qdrant::Value;

struct Payload {
	raw: serde_json::Value,
}

fn build_value() -> Value {
	Value::from(1_i64)
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import009_different_qualified_path.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009" && v.fixable));
		assert!(edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-009"));

		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(!rewritten.contains("use qdrant_client::qdrant::Value;"));
		assert!(rewritten.contains("fn build_value() -> qdrant_client::qdrant::Value"));
		assert!(rewritten.contains("qdrant_client::qdrant::Value::from(1_i64)"));
	}

	#[test]
	fn import009_rewrites_unqualified_derive_symbol() {
		let original = r#"
use foo::Bar;
#[serde(skip_serializing_if = "core::ops::Not::not")]
#[derive(Bar)]
struct Row;

#[derive(foo :: Bar)]
struct Qualified;

fn run() {
	foo::Bar::make();
}
"#;
		let mut rewritten = original.to_owned();
		let mut applied_count = 0_usize;

		for _ in 0..4 {
			let ctx = shared::read_file_context_from_text(
				Path::new("import009_unqualified_derive_symbol.rs"),
				rewritten.clone(),
			)
			.expect("context")
			.expect("has ctx");
			let (_violations, edits) = style::collect_violations(&ctx, true);

			if edits.is_empty() {
				break;
			}

			let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

			applied_count += applied;
		}

		assert!(applied_count > 0);
		assert!(rewritten.contains("#[derive(foo::Bar)]"));
		assert!(!rewritten.contains("#[derive(Bar)]"));
		assert!(!rewritten.contains("use foo::Bar;"));
		assert!(!rewritten.contains("foo::foo::Bar"));
		assert!(
			rewritten.lines().any(|line| line.contains("derive") && line.contains("foo :: Bar"))
		);
		assert!(rewritten.contains("foo::Bar::make()"));

		let skip_serializing_if_lines: Vec<&str> = rewritten
			.lines()
			.filter(|line| line.contains("skip_serializing_if"))
			.map(str::trim)
			.collect();

		assert_eq!(skip_serializing_if_lines.len(), 1);
		assert_eq!(
			skip_serializing_if_lines[0],
			r#"#[serde(skip_serializing_if = "core::ops::Not::not")]"#
		);
	}

	#[test]
	fn import011_reorders_derive_groups_and_alphabetizes_entries() {
		let original = r#"
use serde::Serialize;
use crate::WorkspaceMacro;

#[derive(
	Serialize,
	Debug,
	WorkspaceMacro,
	Clone,
	Default,
)]
struct Row;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import011_derive_groups.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-011" && v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-011"));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied > 0);
		assert!(rewritten.contains("#[derive(Clone, Debug, Default, Serialize, WorkspaceMacro)]"));
	}

	#[test]
	fn import011_reorders_unqualified_imported_derives_by_display_text() {
		let original = r#"
use sqlx::FromRow;
use serde::Serialize;

#[derive(Serialize, Default, FromRow, Clone)]
struct Row;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import011_unqualified_derive.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-011" && v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-011"));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied > 0);
		assert!(rewritten.contains("#[derive(Clone, Default, FromRow, Serialize)]"));
	}

	#[test]
	fn import011_skips_derive_when_any_entry_is_unresolved() {
		let original = r#"
#[derive(Clone, Unknown, serde::Serialize)]
struct Row;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import011_unresolved_derive.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-011"));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-011"));
	}

	#[test]
	fn import011_converges_after_import008_shortens_derive_paths() {
		let original = r#"
#[derive(sqlx::FromRow, Debug, Clone)]
struct Row;
"#;
		let (rewritten, applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(Path::new("import011_after_import008.rs"), original, true)
				.expect("apply fix passes");

		assert!(applied_count > 0, "Rewritten:\n{rewritten}");
		assert!(rewritten.lines().any(|line| line.trim() == "use sqlx::FromRow;"));
		assert!(rewritten.contains("#[derive(Clone, Debug, FromRow)]"));
	}

	#[test]
	fn import011_converges_when_imported_derive_symbol_and_associated_call_stay_short() {
		let original = r#"
use foo::Bar;

#[derive(Bar, Clone)]
struct Row;

fn run() {
	foo::Bar::make();
}
"#;
		let (rewritten, applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(Path::new("import011_after_import009.rs"), original, true)
				.expect("apply fix passes");

		assert!(applied_count > 0, "Rewritten:\n{rewritten}");
		assert!(rewritten.contains("use foo::Bar;"), "Rewritten:\n{rewritten}");
		assert!(rewritten.contains("#[derive(Clone, Bar)]"), "Rewritten:\n{rewritten}");
		assert!(rewritten.contains("Bar::make();"), "Rewritten:\n{rewritten}");
	}

	#[test]
	fn import009_fix_rewrites_ambiguous_pubfi_ai_usage_with_grouped_import_kept() {
		let original = r#"
use pubfi_ai::Usage;
use pubfi_ai::{Cost, Inference};

fn normalize(
	input: Usage,
	rig_usage: rig::completion::Usage,
	qualified: pubfi_ai::Usage,
) -> Usage {
	let _ = (std::mem::size_of::<Cost>(), std::mem::size_of::<Inference>());
	let _ = rig_usage;
	let _ = qualified;
	let local: Usage = input;
	local
}
"#;
		let (rewritten, applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import009_pubfi_ai_usage_grouped_import.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(applied_count > 0, "Rewritten:\n{rewritten}");
		assert!(!rewritten.contains("use pubfi_ai::Usage;"), "Rewritten:\n{rewritten}");
		assert!(rewritten.contains("use pubfi_ai::{Cost, Inference};"), "Rewritten:\n{rewritten}");
		assert!(rewritten.contains("input: pubfi_ai::Usage"), "Rewritten:\n{rewritten}");
		assert!(rewritten.contains(") -> pubfi_ai::Usage"), "Rewritten:\n{rewritten}");
		assert!(
			rewritten.contains("let local: pubfi_ai::Usage = input;"),
			"Rewritten:\n{rewritten}"
		);
		assert!(rewritten.contains("rig_usage: rig::completion::Usage"), "Rewritten:\n{rewritten}");
		assert!(rewritten.contains("qualified: pubfi_ai::Usage"), "Rewritten:\n{rewritten}");
	}

	#[test]
	fn import009_fix_rewrites_pubfi_ai_usage_snippet_and_is_idempotent() {
		let original = r#"
use pubfi_ai::Usage;
use pubfi_ai::{Cost, Inference};

#[test]
fn usage_from_rig_skips_empty() {
	let usage = rig::completion::Usage { input_tokens: 0, output_tokens: 0, total_tokens: 0 };

	assert!(Usage::from_rig(usage).is_none());
}

#[test]
fn usage_from_rig_maps_tokens() {
	let usage = rig::completion::Usage { input_tokens: 12, output_tokens: 34, total_tokens: 46 };
	let mapped = pubfi_ai::Usage::from_rig(usage).expect("Expected usage to map.");

	assert_eq!(mapped, Usage { input_tokens: 12, output_tokens: 34, total_tokens: 46 });
}

#[test]
fn usage_merge_accumulates() {
	let mut usage = Some(pubfi_ai::Usage { input_tokens: 10, output_tokens: 5, total_tokens: 15 });

	pubfi_ai::Usage::merge(&mut usage, Some(pubfi_ai::Usage { input_tokens: 2, output_tokens: 3, total_tokens: 5 }));

	assert_eq!(usage, Some(Usage { input_tokens: 12, output_tokens: 8, total_tokens: 20 }));
}

#[test]
fn inference_record_usage_accumulates() {
	let mut usage = None;
	let mut inference = Inference::new(
		String::from("ok"),
		Some(pubfi_ai::Usage { input_tokens: 7, output_tokens: 9, total_tokens: 16 }),
	);

	inference.cost = Some(Cost::usd(0.00012));

	let mut cost = None;
	let output = inference.record_usage(&mut usage, &mut cost);

	assert_eq!(output, "ok");
	assert_eq!(usage, Some(Usage { input_tokens: 7, output_tokens: 9, total_tokens: 16 }));
	assert_eq!(cost, Some(Cost::usd(0.00012)));
}
"#;
		let (rewritten, applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import009_pubfi_ai_usage_snippet_idempotence.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(applied_count > 0, "Rewritten:\n{rewritten}");
		assert!(!rewritten.contains("use pubfi_ai::Usage;"), "Rewritten:\n{rewritten}");
		assert!(rewritten.contains("assert!(pubfi_ai::Usage::from_rig(usage).is_none());"));
		assert!(rewritten.contains(
			"assert_eq!(mapped, pubfi_ai::Usage { input_tokens: 12, output_tokens: 34, total_tokens: 46 });"
		));
		assert!(rewritten.contains(
			"assert_eq!(usage, Some(pubfi_ai::Usage { input_tokens: 12, output_tokens: 8, total_tokens: 20 }));"
		));
		assert!(rewritten.contains(
			"assert_eq!(usage, Some(pubfi_ai::Usage { input_tokens: 7, output_tokens: 9, total_tokens: 16 }));"
		));

		let (
			repeated,
			repeated_applied_count,
			_repeated_had_import_shortening_edits,
			_repeated_had_let_mut_reorder_edits,
		) = style::apply_fix_passes(
			Path::new("import009_pubfi_ai_usage_snippet_idempotence.rs"),
			&rewritten,
			true,
		)
		.expect("repeat apply fix passes");

		assert_eq!(repeated_applied_count, 0, "Repeated:\n{repeated}");
		assert_eq!(repeated, rewritten);
	}

	#[test]
	fn import009_autofixes_result_when_std_result_is_also_used() {
		let original = r#"
use color_eyre::Result;

fn forward_std_result(value: std::result::Result<(), ()>) -> std::result::Result<(), ()> {
	value
}

async fn acquire_queue() -> Result<()> {
	let _ = std::result::Result::<(), ()>::Ok(());
	Ok(())
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import009_result_std_result.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009" && v.fixable));
		assert!(edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-009"));

		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(!rewritten.contains("use color_eyre::Result;"));
		assert!(rewritten.contains("async fn acquire_queue() -> color_eyre::Result<()>"));
		assert!(rewritten.contains("fn forward_std_result(value: std::result::Result<(), ()>)"));
		assert!(rewritten.contains("std::result::Result::<(), ()>::Ok(())"));
	}

	#[test]
	fn import009_autofixes_crawler_like_std_tokio_reqwest_symbol_conflicts() {
		let original = r#"
use std::io::Error;
use std::result::Result;
use std::time::Instant;

fn dispatch() -> Result<(), Error> {
	let _tokio_now = tokio::time::Instant::now();
	let _reqwest_err = reqwest::Error::from(std::io::Error::other("boom"));
	let _reqwest_ok = reqwest::Result::<(), reqwest::Error>::Ok(());
	let _instant = Instant::now();
	let _error = Error::other("local");
	let _result = Result::<(), Error>::Ok(());
	Ok(())
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import009_crawler_std_tokio_reqwest_conflict.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009" && v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-009"));

		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import009_crawler_std_tokio_reqwest_conflict.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(!rewritten.contains("use std::io::Error;"));
		assert!(!rewritten.contains("use std::result::Result;"));
		assert!(!rewritten.contains("use std::time::Instant;"));
		assert!(rewritten.contains("fn dispatch() -> std::result::Result<(), std::io::Error>"));
		assert!(rewritten.contains("let _instant = std::time::Instant::now();"));
		assert!(rewritten.contains("let _error = std::io::Error::other(\"local\");"));
		assert!(
			rewritten.contains("let _result = std::result::Result::<(), std::io::Error>::Ok(());")
		);
		assert!(rewritten.contains("tokio::time::Instant::now()"));
		assert!(rewritten.contains("reqwest::Error::from(std::io::Error::other(\"boom\"))"));
		assert!(rewritten.contains("reqwest::Result::<(), reqwest::Error>::Ok(())"));
	}

	#[test]
	fn import009_fix_applies_for_pubfi_crawler_dispatcher_braced_segments() {
		let original = r#"
use std::{
	collections::{HashSet, VecDeque, hash_map::Entry},
	error::Error,
	fmt::{Display, Formatter},
	time::Instant,
};
use color_eyre::{Report, eyre};
use color_eyre::Result;

#[derive(Debug)]
struct DispatchError;

impl Display for DispatchError {
	fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
		write!(f, "{f}")
	}
}

impl Error for DispatchError {}

fn execute(deadline: Instant) -> Result<(), Report> {
	let _ = HashSet::<String>::new();
	let _ = VecDeque::<String>::new();
	let _ = std::mem::size_of::<Entry<String, String>>();
	let _ = eyre!("boom");
	let _tokio_deadline = tokio::time::Instant::from_std(deadline);
	let _now = Instant::now();
	let _reqwest_error = reqwest::Error::from(std::io::Error::other("boom"));
	let _std_result = std::result::Result::<(), reqwest::Error>::Ok(());
	Ok(())
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import009_pubfi_crawler_dispatcher.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		for symbol in ["Error", "Instant", "Result"] {
			assert!(violations.iter().any(|v| {
				v.rule == "RUST-STYLE-IMPORT-009"
					&& v.fixable
					&& v.message.contains(&format!("`{symbol}`"))
			}));
		}

		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-009"));

		let (rewritten, applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import009_pubfi_crawler_dispatcher.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(applied_count > 0);
		assert!(!rewritten.contains("error::Error,"));
		assert!(!rewritten.contains("time::Instant,"));
		assert!(rewritten.contains("impl std::error::Error for DispatchError {}"));
		assert!(rewritten.contains("let _now = std::time::Instant::now();"));
	}

	#[test]
	fn import009_fix_applies_for_pubfi_gateway_service_referral_symbols() {
		let original = r#"
use color_eyre::Result;
use tonic::{Request, Response, Status};

use crate::{
	grpc::{
		GetReferralCodeRequest, GetReferralRelationByInviteeRequest, UpsertReferralCodeRequest,
		UpsertReferralRelationRequest,
	},
};
use crate::grpc::ReferralCode;
use crate::grpc::ReferralRelation;

async fn get_referral_code(
	request: Request<GetReferralCodeRequest>,
) -> Result<Response<ReferralCode>, Status> {
	let UpsertReferralCodeRequest { id, referral_code, referrer } = UpsertReferralCodeRequest {
		id: String::new(),
		referral_code: String::new(),
		referrer: String::new(),
	};
	let _record = crate::types::ReferralCode { id, referral_code, referrer, created_at: now() };

	let GetReferralCodeRequest { id: _ } = request.into_inner();

	Ok(Response::new(ReferralCode {
		id: String::new(),
		referral_code: String::new(),
		referrer: String::new(),
		created_at: None,
	}))
}

async fn get_referral_relation(
	request: Request<GetReferralRelationByInviteeRequest>,
) -> Result<Response<ReferralRelation>, Status> {
	let UpsertReferralRelationRequest { id, referral_code, invitee } = UpsertReferralRelationRequest {
		id: String::new(),
		referral_code: String::new(),
		invitee: String::new(),
	};
	let _relation = crate::types::ReferralRelation { id, referral_code, invitee, created_at: now() };

	let GetReferralRelationByInviteeRequest { invitee: _ } = request.into_inner();

	Ok(Response::new(ReferralRelation {
		id: String::new(),
		referral_code: String::new(),
		invitee: String::new(),
		created_at: None,
	}))
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import009_pubfi_gateway_service.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		for symbol in ["ReferralCode", "ReferralRelation"] {
			assert!(violations.iter().any(|v| {
				v.rule == "RUST-STYLE-IMPORT-009"
					&& v.fixable
					&& v.message.contains(&format!("`{symbol}`"))
			}));
		}

		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-009"));

		let (rewritten, applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import009_pubfi_gateway_service.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(applied_count > 0);
		assert!(rewritten.contains("Response::new(crate::grpc::ReferralCode {"));
		assert!(rewritten.contains("Response::new(crate::grpc::ReferralRelation {"));
	}

	#[test]
	fn import009_fix_applies_for_pubfi_gateway_service_large_use_block_shape() {
		let original = r#"
use color_eyre::Result;
use tonic::{Request, Response, Status};

use crate::{auth, context::Context, db::{mail_code, referral, user_data}, error::SqlxStatusExt as _, grpc::{DeleteUserDataRequest, GetReferralCodeRequest, GetReferralRelationByInviteeRequest, GetReferralRelationsByCodeRequest, GetReferralRelationsByCodeResponse, GetUserDataRequest, SendMailCodeRequest, UpdateUserDataRequest, UpdateUserDataResponse, UpsertReferralCodeRequest, UpsertReferralRelationRequest, UserData, VerifyMailCodeRequest}, mail, rate_limit::{RateLimitAction, RateLimitKeyType, RateLimitWindow}, types::{UserDataUpdate}};
use crate::grpc::ReferralCode;
use crate::grpc::ReferralRelation;

async fn get_referral_code(
	request: Request<GetReferralCodeRequest>,
) -> Result<Response<ReferralCode>, Status> {
	let UpsertReferralCodeRequest { id, referral_code, referrer } = UpsertReferralCodeRequest {
		id: String::new(),
		referral_code: String::new(),
		referrer: String::new(),
	};
	let _record = crate::types::ReferralCode { id, referral_code, referrer, created_at: now() };

	let GetReferralCodeRequest { id: _ } = request.into_inner();

	Ok(Response::new(ReferralCode {
		id: String::new(),
		referral_code: String::new(),
		referrer: String::new(),
		created_at: None,
	}))
}

async fn get_referral_relation(
	request: Request<GetReferralRelationByInviteeRequest>,
) -> Result<Response<ReferralRelation>, Status> {
	let UpsertReferralRelationRequest { id, referral_code, invitee } = UpsertReferralRelationRequest {
		id: String::new(),
		referral_code: String::new(),
		invitee: String::new(),
	};
	let _relation = crate::types::ReferralRelation { id, referral_code, invitee, created_at: now() };

	let GetReferralRelationByInviteeRequest { invitee: _ } = request.into_inner();

	Ok(Response::new(ReferralRelation {
		id: String::new(),
		referral_code: String::new(),
		invitee: String::new(),
		created_at: None,
	}))
}
"#;
		let (rewritten, applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import009_pubfi_gateway_service_large_use_shape.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(applied_count > 0, "Rewritten:\n{rewritten}");
		assert!(!rewritten.contains("use crate::grpc::ReferralCode;"), "Rewritten:\n{rewritten}");
		assert!(
			!rewritten.contains("use crate::grpc::ReferralRelation;"),
			"Rewritten:\n{rewritten}"
		);
		assert!(rewritten.contains("Result<Response<crate::grpc::ReferralCode>, Status>"));
		assert!(rewritten.contains("Result<Response<crate::grpc::ReferralRelation>, Status>"));
		assert!(rewritten.contains("Response::new(crate::grpc::ReferralCode {"));
		assert!(rewritten.contains("Response::new(crate::grpc::ReferralRelation {"));
	}

	#[test]
	fn import009_fix_applies_when_result_line_also_breaks_import_group_order() {
		let original = r#"
use std::time::Duration;
use reqwest::Response;

use crate::http::dispatcher::RequestSpec;
use color_eyre::Result;

fn execute(_spec: RequestSpec) -> Result<Response> {
	let _std_result = std::result::Result::<(), reqwest::Error>::Ok(());
	let _duration = Duration::from_millis(1);
	todo!()
}
"#;
		let (rewritten, applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import009_result_with_import001_conflict.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(applied_count > 0, "Rewritten:\n{rewritten}");
		assert!(!rewritten.contains("use color_eyre::Result;"), "Rewritten:\n{rewritten}");
		assert!(
			rewritten.contains("fn execute(_spec: RequestSpec) -> color_eyre::Result<Response>")
		);
	}

	#[test]
	fn import009_fix_applies_for_pubfi_crawler_result_import_after_workspace_group() {
		let original = r#"
use std::{collections::{HashSet, VecDeque, hash_map::Entry}, fmt::{Display, Formatter}, sync::atomic::{AtomicUsize, Ordering}, task::{Context, Poll}};
use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc, time::Duration};

use derive_setters::Setters;
use reqwest::{Method, Request, Response, Url, header::HeaderName};
use tokio::sync::{
	AcquireError, OwnedSemaphorePermit,
	mpsc::{self, Receiver},
	oneshot,
};
use tower::{
	Service,
	util::{BoxCloneService, ServiceExt as _},
};
use tracing::Instrument;
use color_eyre::{Report, eyre};
use reqwest::Client;
use sqlx::types::Uuid;
use tokio::sync::{Mutex, Semaphore};

use crate::http::{
	control::{AdaptiveState, CircuitBreaker, Feedback, HostControl, QpsLimiter},
	params::{AdaptiveParams, DispatcherParams},
};
use pubfi_obs::{RuntimeEvent, RuntimeLog};
use pubfi_util::{Backoff, RetryError};

use color_eyre::Result;

pub type GatewayService = BoxCloneService<RequestSpec, Response, Report>;

#[derive(Debug)]
enum DispatchError {
	QueueFull,
}
impl Display for DispatchError {
	fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
		write!(f, "{f}")
	}
}
impl std::error::Error for DispatchError {}

fn poll_ready(_: &mut Context) -> Poll<std::result::Result<(), Report>> {
	let _ = std::result::Result::<(), Report>::Ok(());
	Poll::Ready(Ok(()))
}

async fn acquire_queue() -> Result<OwnedSemaphorePermit> {
	let _ = (
		AdaptiveState::default(),
		CircuitBreaker::new(0),
		Feedback::default(),
		HostControl::default(),
		QpsLimiter::default(),
		AdaptiveParams::default(),
		DispatcherParams::default(),
		Backoff::default(),
		RetryError::default(),
	);
	let _ = (
		HashSet::<String>::new(),
		VecDeque::<String>::new(),
		std::mem::size_of::<Entry<String, String>>(),
		AtomicUsize::new(0),
		Ordering::Relaxed,
		Arc::new(AtomicUsize::new(1)),
		Duration::from_millis(1),
		Method::GET,
		Request::new(Method::GET, Url::parse("https://example.com").expect("url")),
		Response::new(reqwest::Body::default()),
		HeaderName::from_static("x-test"),
		AcquireError::NoPermits,
		Receiver::<()>::default(),
		oneshot::channel::<()>().0,
		Client::new(),
		Uuid::nil(),
		Mutex::new(()),
		Semaphore::new(1),
		RuntimeEvent::Shutdown,
		RuntimeLog::default(),
		eyre!("boom"),
	);
	todo!()
}
"#;
		let (rewritten, applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import009_pubfi_crawler_result_after_workspace_group.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(applied_count > 0, "Rewritten:\n{rewritten}");
		assert!(!rewritten.contains("use color_eyre::Result;"), "Rewritten:\n{rewritten}");
		assert!(
			rewritten
				.contains("async fn acquire_queue() -> color_eyre::Result<OwnedSemaphorePermit>")
		);
	}

	#[test]
	fn import009_removes_redundant_import_when_only_qualified_same_path_usage_remains() {
		let original = r#"
use crate::{
	structured_fields::{StructuredFields, upsert_structured_fields_tx},
};

fn run_ops() {
	let _ = StructuredFields::default();
	crate::structured_fields::upsert_structured_fields_tx();
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import009_remove_redundant_same_path_import.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(!rewritten.contains("upsert_structured_fields_tx,"));
		assert!(!rewritten.contains("use crate::structured_fields::upsert_structured_fields_tx"));
		assert!(
			rewritten.contains("use crate::{structured_fields::{self, StructuredFields}};"),
			"{rewritten}"
		);
		assert!(rewritten.contains("structured_fields::upsert_structured_fields_tx();"));
		assert!(!rewritten.contains("crate::structured_fields::upsert_structured_fields_tx();"));
	}

	#[test]
	fn import004_multi_pass_removes_multiple_free_function_imports_without_leftover_unused_imports()
	{
		let original = r#"
use crate::{
	structured_fields::{StructuredFields, upsert_structured_fields_tx, validate_structured_fields},
};

fn run_ops() {
	let _ = StructuredFields::default();
	upsert_structured_fields_tx();
	validate_structured_fields();
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import004_remove_multiple_free_functions.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(
			rewritten.contains("use crate::{structured_fields::{self, StructuredFields}};"),
			"{rewritten}"
		);
		assert!(rewritten.contains("structured_fields::upsert_structured_fields_tx();"));
		assert!(rewritten.contains("structured_fields::validate_structured_fields();"));
		assert!(!rewritten.contains("use crate::structured_fields::upsert_structured_fields_tx"));
		assert!(!rewritten.contains("use crate::structured_fields::validate_structured_fields"));
		assert!(!rewritten.contains("upsert_structured_fields_tx,"));
		assert!(!rewritten.contains("validate_structured_fields,"));
	}

	#[test]
	fn import004_rewrites_remaining_free_function_import_when_parent_module_is_already_imported() {
		let original = r#"
use crate::{
	structured_fields::{self, StructuredFields, validate_structured_fields},
};

fn run_ops() {
	let _ = StructuredFields::default();
	structured_fields::upsert_structured_fields_tx();
	validate_structured_fields();
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import004_remaining_free_function_with_existing_parent_module.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(
			rewritten.contains("use crate::{structured_fields::{self, StructuredFields}};"),
			"{rewritten}"
		);
		assert!(rewritten.contains("structured_fields::upsert_structured_fields_tx();"));
		assert!(rewritten.contains("structured_fields::validate_structured_fields();"));
		assert!(!rewritten.contains("crate::structured_fields::validate_structured_fields();"));
		assert!(!rewritten.contains("validate_structured_fields,"));
	}

	#[test]
	fn import_rules_report_cfg_test_module_import_violations() {
		let text = r#"#[cfg(test)]
mod tests {
	use crate::structured_fields::{StructuredFields, upsert_structured_fields_tx};
	use std::collections::BTreeMap;

	#[test]
	fn run_ops() {
		let _ = StructuredFields::default();
		let _ = BTreeMap::<u8, u8>::new();
		upsert_structured_fields_tx();
	}
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import_cfg_test_module_violations.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-004"
				&& v.line == 3
				&& v.fixable
				&& v.message
					== "Do not import free functions or macros into scope; prefer qualified module paths."
		}));
		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-001"
				&& v.line == 4
				&& v.fixable
				&& v.message == "Import groups must be ordered: std, third-party, self/workspace."
		}));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-004"));
		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-008"));
		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009"));
	}

	#[test]
	fn import_rules_fix_cfg_test_module_imports() {
		let original = r#"#[cfg(test)]
mod tests {
	use crate::structured_fields::{StructuredFields, upsert_structured_fields_tx};
	use std::collections::BTreeMap;

	#[test]
	fn run_ops() {
		let _ = StructuredFields::default();
		let _ = BTreeMap::<u8, u8>::new();
		upsert_structured_fields_tx();
	}
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(Path::new("import_cfg_test_module_fix.rs"), original, true)
				.expect("apply fix passes");
		let std_use_idx =
			rewritten.find("use std::collections::BTreeMap;").expect("std import must remain");
		let crate_use_idx =
			rewritten.find("use crate::structured_fields").expect("crate import must remain");

		assert!(std_use_idx < crate_use_idx);
		assert!(
			rewritten.contains("use std::collections::BTreeMap;\n\n\tuse crate::structured_fields")
		);
		assert!(
			rewritten.contains("\n\tuse crate::structured_fields::{self, StructuredFields};\n"),
			"{rewritten}"
		);
		assert!(rewritten.contains("\n\t\tstructured_fields::upsert_structured_fields_tx();\n"));
		assert!(!rewritten.contains(
			"use crate::structured_fields::{StructuredFields, upsert_structured_fields_tx};"
		));
		assert!(!rewritten.contains("use crate::structured_fields::upsert_structured_fields_tx;"));

		let rewritten_ctx = shared::read_file_context_from_text(
			Path::new("import_cfg_test_module_fix.rs"),
			rewritten.clone(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&rewritten_ctx, true);

		assert!(!violations.iter().any(|v| {
			matches!(
				v.rule,
				"RUST-STYLE-IMPORT-001" | "RUST-STYLE-IMPORT-002" | "RUST-STYLE-IMPORT-004"
			)
		}));
		assert!(!edits.iter().any(|e| {
			matches!(
				e.rule,
				"RUST-STYLE-IMPORT-001" | "RUST-STYLE-IMPORT-002" | "RUST-STYLE-IMPORT-004"
			)
		}));
	}

	#[test]
	fn import_rules_fix_cfg_test_module_function_calls_inside_macro_tokens() {
		let original = r#"#[cfg(test)]
mod tests {
	use crate::routes::{effective_token_id, inject_request_id_into_json_body};
	use uuid::Uuid;

	#[test]
	fn check_routes() {
		let request_id =
			Uuid::parse_str("123e4567-e89b-12d3-a456-426614174000").expect("valid uuid");
		let body = serde_json::json!({"note_id":"abc","status":"ok"}).to_string();
		let response_body = inject_request_id_into_json_body(body.as_bytes(), &request_id)
			.expect("Expected request_id field to be injected.");
		assert!(inject_request_id_into_json_body(body.as_bytes(), &request_id).is_none());
		assert_eq!(effective_token_id("off", &axum::http::HeaderMap::new()), None);
		let _ = response_body;
	}
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import_cfg_test_module_macro_token_function_fix.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert_eq!(
			rewritten
				.matches("routes::inject_request_id_into_json_body(body.as_bytes(), &request_id)")
				.count(),
			2,
			"Rewritten:\n{rewritten}"
		);
		assert!(rewritten.contains("use crate::routes::{self};"), "{rewritten}");
		assert!(
			rewritten
				.contains("routes::effective_token_id(\"off\", &axum::http::HeaderMap::new())")
		);
		assert!(!rewritten.contains(
			"use crate::routes::{effective_token_id, inject_request_id_into_json_body};"
		));

		let rewritten_ctx = shared::read_file_context_from_text(
			Path::new("import_cfg_test_module_macro_token_function_fix.rs"),
			rewritten.clone(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&rewritten_ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-004"));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-004"));
	}

	#[test]
	fn import009_fix_rewrites_non_importable_generic_root_use() {
		let original = r#"
use serde::Serializer;
use S::Error;
use S::Ok;

pub fn serialize<S>(serializer: S) -> Result<Ok, Error>
where
	S: Serializer,
{
	let _ = serializer;
	todo!()
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import009_non_importable_generic_root.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		eprintln!(
			"use_items={:?}",
			ctx.top_items
				.iter()
				.filter(|item| item.kind == shared::TopKind::Use)
				.map(|item| (item.raw.clone(), item.use_path.clone()))
				.collect::<Vec<_>>()
		);
		eprintln!(
			"violations={:?}",
			violations.iter().map(|v| (v.rule, v.message.clone(), v.fixable)).collect::<Vec<_>>()
		);
		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009" && v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-009"));

		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(!rewritten.contains("use S::Error;"));
		assert!(!rewritten.contains("use S::Ok;"));
		assert!(rewritten.contains("-> Result<S::Ok, S::Error>"));
	}

	#[test]
	fn import009_fix_rewrites_non_importable_self_root_use() {
		let original = r#"
use Self::Error;

trait Task {
	type Error;
	fn run(&self) -> Result<(), Error>;
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import009_non_importable_self_root.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009" && v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-009"));

		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(!rewritten.contains("use Self::Error;"));
		assert!(rewritten.contains("-> Result<(), Self::Error>"));
	}

	#[test]
	fn import009_fix_rewrites_std_fmt_result_import_to_qualified_non_generic_uses() {
		let original = r#"
use std::fmt::Result;

fn format_output() -> Result {
	Ok(())
}

fn parse_value() -> Result<u8, &'static str> {
	Ok(1)
}
"#;
		let mut rewritten = original.to_owned();

		for _ in 0..4 {
			let ctx = shared::read_file_context_from_text(
				Path::new("import009_std_fmt_result.rs"),
				rewritten.clone(),
			)
			.expect("context")
			.expect("has ctx");
			let (violations, edits) = style::collect_violations(&ctx, true);

			if edits.is_empty() {
				assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009"));

				break;
			}

			let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		}

		assert!(!rewritten.contains("use std::fmt::Result;"));
		assert!(rewritten.contains("fn format_output() -> std::fmt::Result"));
		assert!(rewritten.contains("fn parse_value() -> Result<u8, &'static str>"));
	}

	#[test]
	fn import009_fix_removes_std_fmt_result_import_when_generic_result_is_used() {
		let original = r#"
use std::fmt::Result;

fn parse_value() -> Result<u8, &'static str> {
	Ok(1)
}
"#;
		let mut rewritten = original.to_owned();

		for _ in 0..4 {
			let ctx = shared::read_file_context_from_text(
				Path::new("import009_std_fmt_result_generic_only.rs"),
				rewritten.clone(),
			)
			.expect("context")
			.expect("has ctx");
			let (_violations, edits) = style::collect_violations(&ctx, true);

			if edits.is_empty() {
				break;
			}

			let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		}

		assert!(!rewritten.contains("use std::fmt::Result;"));
		assert!(rewritten.contains("fn parse_value() -> Result<u8, &'static str>"));
	}

	#[test]
	fn import009_fix_is_not_undone_by_import008_for_result_symbols() {
		let original = r#"
use color_eyre::Result;

fn display() -> std::fmt::Result {
	Ok(())
}

fn run() -> Result<()> {
	Ok(())
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import009_import008_result_cycle.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(!rewritten.contains("use color_eyre::Result;"));
		assert!(rewritten.contains("fn display() -> std::fmt::Result"));
		assert!(rewritten.contains("fn run() -> color_eyre::Result<()>"));

		let (
			repeated,
			_repeated_applied_count,
			_repeated_had_import_shortening_edits,
			_repeated_had_let_mut_reorder_edits,
		) = style::apply_fix_passes(
			Path::new("import009_import008_result_cycle.rs"),
			&rewritten,
			true,
		)
		.expect("repeat apply fix passes");

		assert_eq!(repeated.trim_start_matches('\n'), rewritten.trim_start_matches('\n'));
	}

	#[test]
	fn import009_autofix_applies_for_unqualified_type_with_qualified_same_path_value_constructor() {
		let original = r#"
use serde_json::Value;

fn build_payload() -> Value {
	serde_json::Value::String("ok".to_string())
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import009_unqualified_type_qualified_value_constructor.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);
		let import009_edits = edits
			.into_iter()
			.filter(|edit| edit.rule == "RUST-STYLE-IMPORT-009")
			.collect::<Vec<_>>();
		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, import009_edits).expect("apply edits");

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009" && v.fixable));
		assert!(applied > 0);
		assert!(!rewritten.contains("use serde_json::Value;"));
		assert!(rewritten.contains("fn build_payload() -> serde_json::Value"));
		assert!(rewritten.contains("serde_json::Value::String(\"ok\".to_string())"));
	}

	#[test]
	fn import009_autofix_applies_for_unqualified_error_type_with_qualified_same_path_variant_usage()
	{
		let original = r#"
use pubfi_search::Error;

struct SearchFailure {
	source: Error,
}

fn normalize_error(input: Error) -> Error {
	let _ = input;
	pubfi_search::Error::NotFound
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import009_unqualified_error_type_qualified_variant.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);
		let import009_edits = edits
			.into_iter()
			.filter(|edit| edit.rule == "RUST-STYLE-IMPORT-009")
			.collect::<Vec<_>>();
		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, import009_edits).expect("apply edits");

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009" && v.fixable));
		assert!(applied > 0);
		assert!(!rewritten.contains("use pubfi_search::Error;"));
		assert!(rewritten.contains("source: pubfi_search::Error"));
		assert!(
			rewritten
				.contains("fn normalize_error(input: pubfi_search::Error) -> pubfi_search::Error")
		);
		assert!(rewritten.contains("pubfi_search::Error::NotFound"));
	}

	#[test]
	fn import008_shortens_grouped_pubfi_search_error_receiver_when_import_exists() {
		let original = r#"
use pubfi_search::{
	client::SearchClient,
	query::SearchQuery,
};
use pubfi_search::Error;

struct SearchFailure {
	source: Error,
}

fn normalize_error(input: Error, query: SearchQuery, client: SearchClient) -> Error {
	let _ = input;
	let _ = query;
	let _ = client;
	pubfi_search::Error::NotFound
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import009_import008_pubfi_search_error_cycle.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(rewritten.contains("use pubfi_search::Error;"), "{rewritten}");
		assert!(rewritten.contains("source: Error"), "Rewritten:\n{rewritten}");
		assert!(
			rewritten.contains(
				"fn normalize_error(input: Error, query: SearchQuery, client: SearchClient) -> Error"
			),
			"Rewritten:\n{rewritten}"
		);
		assert!(rewritten.contains("Error::NotFound"), "Rewritten:\n{rewritten}");

		let (
			repeated,
			repeated_applied_count,
			_repeated_had_import_shortening_edits,
			_repeated_had_let_mut_reorder_edits,
		) = style::apply_fix_passes(
			Path::new("import009_import008_pubfi_search_error_cycle.rs"),
			&rewritten,
			true,
		)
		.expect("repeat apply fix passes");

		assert_eq!(repeated_applied_count, 0);
		assert_eq!(repeated.trim_start_matches('\n'), rewritten.trim_start_matches('\n'));
	}

	#[test]
	fn import009_does_not_autofix_when_symbol_has_standalone_value_uses() {
		let original = r#"
use pubfi_search::query;

fn validate_percolator_query(query: &serde_json::Value) {
	let _ = sqlx::query("SELECT 1");
	let _ = query.get("bool");
	let _dsl = query::build_percolate_dsl_from_constraints;
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import009_standalone_value_uses.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009"));
		assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-009"));
	}

	#[test]
	fn import009_ignores_lowercase_module_symbol_with_qualified_module_sibling() {
		let original = r#"
use crate::service::feed::feeds;

fn publish() {
	let _ = feeds::get_owned_feed;
	let _ = store::feed::feeds::update_by_id;
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import009_lowercase_module_symbol.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009"));
		assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-009"));
	}

	#[test]
	fn import009_fix_rewrites_qualified_associated_fn_receiver_symbol() {
		let original = r#"
use crate::cli::PercolateFilterMode;

fn normalize(filter_mode: PercolateFilterMode) {
	let _ = pubfi_query::PercolateFilterMode::from(filter_mode);
}
"#;
		let expected = r#"
use crate::cli;

fn normalize(filter_mode: crate::cli::PercolateFilterMode) {
	let _ = pubfi_query::PercolateFilterMode::from(filter_mode);
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import009_associated_fn_receiver_symbol.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert_eq!(rewritten.trim_start_matches('\n'), expected.trim_start_matches('\n'));
	}

	#[test]
	fn import009_fix_rewrites_dyn_trait_type_when_associated_receiver_path_conflicts() {
		let original = r#"
use age::Recipient;

fn encrypt(recipients: &[String]) {
	let recipients = recipients.iter().map(|s| age::x25519::Recipient::from_str(s).unwrap());
	let _ = recipients.map(|r| r as &dyn Recipient).count();
}
"#;
		let expected = r#"
fn encrypt(recipients: &[String]) {
	let recipients = recipients.iter().map(|s| age::x25519::Recipient::from_str(s).unwrap());
	let _ = recipients.map(|r| r as &dyn age::Recipient).count();
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import009_age_recipient_conflict.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert_eq!(rewritten.trim_start_matches('\n'), expected.trim_start_matches('\n'));
	}

	#[test]
	fn import009_cycle_guard_blocks_unqualified_value_with_qualified_type_same_path() {
		let original = r#"
use serde_json::Value;

fn build_payload() -> serde_json::Value {
	Value::String("ok".to_string())
}
"#;
		let expected = r#"
use serde_json::Value;

fn build_payload() -> Value {
	Value::String("ok".to_string())
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(Path::new("import009_import008_value_cycle.rs"), original, true)
				.expect("apply fix passes");

		assert_eq!(rewritten.trim_start_matches('\n'), expected.trim_start_matches('\n'));

		let (
			repeated,
			repeated_applied_count,
			_repeated_had_import_shortening_edits,
			_repeated_had_let_mut_reorder_edits,
		) = style::apply_fix_passes(
			Path::new("import009_import008_value_cycle.rs"),
			&rewritten,
			true,
		)
		.expect("repeat apply fix passes");

		assert_eq!(repeated_applied_count, 0);
		assert_eq!(repeated.trim_start_matches('\n'), rewritten.trim_start_matches('\n'));
	}

	#[test]
	fn import008_shortens_serde_value_receivers_when_import_exists() {
		let original = r#"
use std::collections::HashMap;

use serde_json::Value;

fn build_payload() -> Value {
	serde_json::Value::String("ok".to_string())
}

fn capture(mut map: HashMap<String, Value>) {
	map.insert("kind".to_string(), serde_json::Value::String("x".to_string()));
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(
				Path::new("import009_import008_serde_value_cycle.rs"),
				original,
				true,
			)
			.expect("apply fix passes");

		assert!(rewritten.contains("use serde_json::Value;"), "Rewritten:\n{rewritten}");
		assert!(rewritten.contains("fn build_payload() -> Value"), "Rewritten:\n{rewritten}");
		assert!(
			rewritten.contains("fn capture(mut map: HashMap<String, Value>)"),
			"Rewritten:\n{rewritten}"
		);
		assert!(rewritten.contains("Value::String(\"x\".to_string())"), "Rewritten:\n{rewritten}");

		let (
			repeated,
			repeated_applied_count,
			_repeated_had_import_shortening_edits,
			_repeated_had_let_mut_reorder_edits,
		) = style::apply_fix_passes(
			Path::new("import009_import008_serde_value_cycle.rs"),
			&rewritten,
			true,
		)
		.expect("repeat apply fix passes");

		assert_eq!(repeated_applied_count, 0);
		assert_eq!(repeated.trim_start_matches('\n'), rewritten.trim_start_matches('\n'));
	}

	#[test]
	fn import009_fix_rewrites_struct_literal_value_path_when_symbol_is_ambiguous() {
		let original = r#"
use crate::types::ReferralCode;

fn upsert(input: crate::grpc::ReferralCode) -> crate::types::ReferralCode {
	let _ = input;
	let record = ReferralCode { id: "x".to_string() };
	record
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import009_struct_literal_value_path.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009" && v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-009"));

		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(!rewritten.contains("use crate::types::ReferralCode;"));
		assert!(
			rewritten
				.contains("let record = crate::types::ReferralCode { id: \"x\".to_string() };")
		);
	}

	#[test]
	fn import009_fix_rewrites_struct_literal_value_path_from_braced_use_with_alias_sibling() {
		let original = r#"
use crate::{error::SqlxStatusExt as _, grpc::{GetReferralCodeRequest}, types::{ReferralCode}};

fn upsert(input: crate::grpc::ReferralCode) -> crate::types::ReferralCode {
	let _ = input;
	let _ = GetReferralCodeRequest { id: String::new() };
	let record = ReferralCode { id: "x".to_string() };
	record
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import009_struct_literal_braced_alias_sibling.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009" && v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-009"));

		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(rewritten.contains("SqlxStatusExt as _"));
		assert!(!rewritten.contains("types::{ReferralCode}"));
		assert!(
			rewritten
				.contains("let record = crate::types::ReferralCode { id: \"x\".to_string() };")
		);
	}

	#[test]
	fn import009_fix_rewrites_gateway_style_struct_literal_from_large_braced_use_tree() {
		let original = r#"
use crate::{auth, context::Context, db::{mail_code, referral, user_data}, error::SqlxStatusExt as _, grpc::{DeleteUserDataRequest, GetReferralCodeRequest, GetReferralRelationByInviteeRequest, GetReferralRelationsByCodeRequest, GetReferralRelationsByCodeResponse, GetUserDataRequest, SendMailCodeRequest, UpdateUserDataRequest, UpdateUserDataResponse, UpsertReferralCodeRequest, UpsertReferralRelationRequest, UserData, VerifyMailCodeRequest}, mail, rate_limit::{RateLimitAction, RateLimitKeyType, RateLimitWindow}, types::{ReferralCode, ReferralRelation, UserDataUpdate}};

fn upsert_referral_code(request: crate::grpc::ReferralCode) -> crate::grpc::ReferralCode {
	let _ = (
		auth::client_app_from_request,
		mail_code::delete_by_id,
		referral::insert_code,
		user_data::get_or_insert,
		mail::generate_code,
		RateLimitAction::Send,
		RateLimitKeyType::Mail,
		RateLimitWindow { window_seconds: 1, limit: 1 },
		DeleteUserDataRequest {},
		GetReferralCodeRequest {},
		GetReferralRelationByInviteeRequest {},
		GetReferralRelationsByCodeRequest {},
		GetReferralRelationsByCodeResponse { relations: Vec::new() },
		GetUserDataRequest {},
		SendMailCodeRequest {},
		UpdateUserDataRequest {},
		UpdateUserDataResponse {},
		UpsertReferralCodeRequest {},
		UpsertReferralRelationRequest {},
		UserData {},
		VerifyMailCodeRequest {},
	);
	let _ = (
		ReferralRelation { id: String::new(), referral_code: String::new(), invitee: String::new(), created_at: now() },
		UserDataUpdate { user_id: String::new(), user_data: None },
	);
	let record = ReferralCode { id: String::new(), referral_code: String::new(), referrer: String::new(), created_at: now() };
	let _ = Context::new();
	let _ = request;
	crate::grpc::ReferralCode { id: record.id, referral_code: record.referral_code, referrer: record.referrer, created_at: None }
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import009_gateway_style_large_use.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-009" && v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-009"));

		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(!rewritten.contains("types::{ReferralCode,"));
		assert!(rewritten.contains("let record = crate::types::ReferralCode {"));
	}

	#[test]
	fn import009_fix_applies_for_gateway_symbols_with_separate_imports_and_conflicting_paths() {
		let original = r#"
use crate::types::ReferralCode;
use crate::types::ReferralRelation;

fn upsert_referral_code(
	request: crate::grpc::ReferralCode,
	request_relation: crate::grpc::ReferralRelation,
) -> crate::grpc::ReferralCode {
	let _ = request_relation;
	let _relation = ReferralRelation {
		id: String::new(),
		referral_code: String::new(),
		invitee: String::new(),
		created_at: now(),
	};
	let record = ReferralCode {
		id: String::new(),
		referral_code: String::new(),
		referrer: String::new(),
		created_at: now(),
	};
	let _type_conflict_code: crate::grpc::ReferralCode = request;
	let _type_conflict_relation: crate::grpc::ReferralRelation = request_relation;
	crate::grpc::ReferralCode {
		id: record.id,
		referral_code: record.referral_code,
		referrer: record.referrer,
		created_at: None,
	}
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import009_gateway_separate_imports_conflicting_paths.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-009" && v.fixable && v.message.contains("`ReferralCode`")
		}));
		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-009"
				&& v.fixable
				&& v.message.contains("`ReferralRelation`")
		}));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-IMPORT-009"));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied > 0);
		assert!(!rewritten.contains("use crate::types::ReferralCode;"));
		assert!(!rewritten.contains("use crate::types::ReferralRelation;"));
		assert!(rewritten.contains("let _relation = crate::types::ReferralRelation {"));
		assert!(rewritten.contains("let record = crate::types::ReferralCode {"));
	}

	#[test]
	fn import009_fix_applies_when_gateway_imports_are_eof_and_same_line() {
		let original = r#"fn upsert_referral_code(
	request: crate::grpc::ReferralCode,
	request_relation: crate::grpc::ReferralRelation,
) -> crate::grpc::ReferralCode {
	let _relation = ReferralRelation {
		id: String::new(),
		referral_code: String::new(),
		invitee: String::new(),
		created_at: now(),
	};
	let record = ReferralCode {
		id: String::new(),
		referral_code: String::new(),
		referrer: String::new(),
		created_at: now(),
	};
	let _type_conflict_code: crate::grpc::ReferralCode = request;
	let _type_conflict_relation: crate::grpc::ReferralRelation = request_relation;
	crate::grpc::ReferralCode {
		id: record.id,
		referral_code: record.referral_code,
		referrer: record.referrer,
		created_at: None,
	}
}
use crate::types::ReferralCode; use crate::types::ReferralRelation;"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("import009_gateway_separate_imports_eof_same_line.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-009" && v.fixable && v.message.contains("`ReferralCode`")
		}));
		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-IMPORT-009"
				&& v.fixable
				&& v.message.contains("`ReferralRelation`")
		}));

		let import009_edits = edits
			.into_iter()
			.filter(|edit| edit.rule == "RUST-STYLE-IMPORT-009")
			.collect::<Vec<_>>();

		assert!(!import009_edits.is_empty());

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, import009_edits).expect("apply edits");

		assert!(applied > 0);
		assert!(!rewritten.contains("use crate::types::ReferralCode;"));
		assert!(!rewritten.contains("use crate::types::ReferralRelation;"));
		assert!(rewritten.contains("let _relation = crate::types::ReferralRelation {"));
		assert!(rewritten.contains("let record = crate::types::ReferralCode {"));
	}

	#[test]
	fn mod005_relocation_preserves_one_impl_when_another_edit_overlaps() {
		let original = "struct Sample;\n\nenum Other { Item }\n\nimpl Sample {\n    fn value() -> u32 { 1 }\n}\n";
		let ctx =
			shared::read_file_context_from_text(Path::new("move_overlap.rs"), original.to_owned())
				.expect("Read context.")
				.expect("Have context.");
		let (_, edits) = style::collect_violations(&ctx, true);
		let start = original.find("impl Sample").expect("Find impl.");
		let mut edits =
			edits.into_iter().filter(|edit| edit.rule == "RUST-STYLE-MOD-005").collect::<Vec<_>>();

		edits.push(shared::Edit {
			start,
			end: start + 1,
			replacement: "i".to_owned(),
			rule: "RUST-STYLE-IMPORT-001",
		});

		let mut rewritten = original.to_owned();

		fixes::apply_edits(&mut rewritten, edits).expect("Apply overlapping edits.");

		assert_eq!(rewritten.matches("impl Sample").count(), 1, "{rewritten}");
	}

	#[test]
	fn mod005_fix_moves_impl_block_adjacent_to_type() {
		let original = r#"
struct Sample;

enum Other {
	Item,
}

impl Sample {
	fn new() -> Self {
		Self
	}
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod005_move_impl.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-005" && v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-MOD-005"));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);

		let struct_idx = rewritten.find("struct Sample;").expect("struct");
		let impl_idx = rewritten.find("impl Sample").expect("impl");
		let enum_idx = rewritten.find("enum Other").expect("enum");

		assert!(struct_idx < impl_idx);
		assert!(impl_idx < enum_idx);
	}

	#[test]
	fn mod005_fix_moves_impl_block_after_type_when_impl_precedes_type() {
		let original = r#"
impl Sample {
	fn new() -> Self {
		Self
	}
}

struct Sample;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod005_impl_before_type.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-005" && v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-MOD-005"));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains("struct Sample;\nimpl Sample {"));
	}

	#[test]
	fn mod005_fix_does_not_accumulate_blank_lines_around_relocated_impls() {
		let original = r#"
use std::sync::Arc;

struct RoundGate {
	in_progress: bool,
}

struct RoundGateGuard {
	gate: Arc<RoundGate>,
}

impl RoundGate {
	fn new() -> Self {
		Self { in_progress: false }
	}
}

impl RoundGateGuard {
	fn new(gate: Arc<RoundGate>) -> Self {
		Self { gate }
	}
}

impl Drop for RoundGateGuard {
	fn drop(&mut self) {}
}

/// Bootstrap and run all crawlers.
pub async fn run() {}
"#;
		let mut rewritten = original.to_owned();

		for _ in 0..MAX_FIX_PASSES {
			let Some(ctx) = shared::read_file_context_from_text(
				Path::new("mod005_blank_lines.rs"),
				rewritten.clone(),
			)
			.expect("context") else {
				break;
			};
			let (_violations, edits) = style::collect_violations(&ctx, true);

			if edits.is_empty() {
				break;
			}

			let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

			if applied == 0 {
				break;
			}
		}

		assert!(
			rewritten.contains("struct RoundGate {\n\tin_progress: bool,\n}\nimpl RoundGate {")
		);
		assert!(rewritten.contains(
			"struct RoundGateGuard {\n\tgate: Arc<RoundGate>,\n}\nimpl RoundGateGuard {"
		));
		assert!(rewritten.contains("}\n\n/// Bootstrap and run all crawlers."));
		assert!(!rewritten.contains("}\n\n\n/// Bootstrap and run all crawlers."));
	}

	#[test]
	fn space003_fix_removes_blank_lines_within_same_statement_type() {
		let original = r#"
fn sample() {
	let a = 1;

	let b = 2;
}
"#;
		let ctx =
			shared::read_file_context_from_text(Path::new("space_same.rs"), original.to_owned())
				.expect("context")
				.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-003"
			&& v.message == "Do not insert blank lines within the same statement type."
			&& v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains("let a = 1;\n\tlet b = 2;"));
	}

	#[test]
	fn space003_fix_inserts_single_blank_line_between_different_statement_types() {
		let original = r#"
fn sample() {
	let a = 1;
	if a > 0 {
		let _ = a;
	}
}
"#;
		let ctx =
			shared::read_file_context_from_text(Path::new("space_diff.rs"), original.to_owned())
				.expect("context")
				.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-003"
			&& v.message == "Insert exactly one blank line between different statement types."
			&& v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains("let a = 1;\n\n\tif a > 0 {"));
	}

	#[test]
	fn spacing_fixes_preserve_metadata_and_literals() {
		for (source, anchor) in [
			(
				"fn sample() {\n let value = \"雪\";\n // Keep the check.\n #[cfg(test)]\n assert_eq!(value, \"雪\");\n}\n",
				" // Keep the check.",
			),
			(
				"fn sample() -> i32 {\n let value = 1;\n /* Keep this block\n  * and its layout. */\n return value;\n}\n",
				" /* Keep this block",
			),
			(
				"fn sample() -> i32 {\n let value = 1;\n // Keep the tail.\n #[cfg(all())]\n value\n}\n",
				" // Keep the tail.",
			),
		] {
			let ctx = shared::read_file_context_from_text(
				Path::new("metadata_spacing.rs"),
				source.to_owned(),
			)
			.expect("Read fixture.")
			.expect("Get fixture context.");
			let (_, edits) = style::collect_violations(&ctx, true);
			let edits = edits
				.into_iter()
				.filter(|edit| matches!(edit.rule, "RUST-STYLE-SPACE-003" | "RUST-STYLE-SPACE-004"))
				.collect();
			let mut rewritten = source.to_owned();

			assert!(fixes::apply_edits(&mut rewritten, edits).expect("Apply spacing edits.") > 0);
			assert_eq!(rewritten, source.replace(anchor, &format!("\n{anchor}")));

			let ctx =
				shared::read_file_context_from_text(Path::new("metadata_spacing.rs"), rewritten)
					.expect("Read fixed fixture.")
					.expect("Get fixed context.");
			let (violations, _) = style::collect_violations(&ctx, true);

			assert!(
				!violations
					.iter()
					.any(|v| matches!(v.rule, "RUST-STYLE-SPACE-003" | "RUST-STYLE-SPACE-004")),
				"{violations:?}"
			);
		}
	}

	#[test]
	fn space003_match_guard_preserves_real_statement_checks() {
		let text = r#"
fn inspect(value: Item) -> bool {
	match value {
		Item::Ready { value } | Item::Pending { value }
			if value > 0 => {
			let ready = value > 1;
			if ready {
				return true;
			}

			false
		},
		_ => false,
	}
}
"#;
		let ctx = shared::read_file_context_from_text(Path::new("match_guard.rs"), text.into())
			.expect("context")
			.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);
		let guard_line = text.lines().position(|line| line.contains("if value > 0")).unwrap() + 1;
		let body_line = text.lines().position(|line| line.contains("if ready")).unwrap() + 1;

		assert!(
			!violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-003" && v.line == guard_line)
		);
		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-003" && v.line == body_line));
		assert!(edits.iter().any(|edit| edit.rule == "RUST-STYLE-SPACE-003"));
	}

	#[test]
	fn space003_does_not_split_if_condition_from_body_brace_after_block_expression() {
		let text = r#"
fn lane_reference(
	metadata_file_type_is_symlink: bool,
	metadata_is_regular_file: bool,
	metadata_hard_link_count: u64,
	metadata_owner_uid: u32,
) -> bool {
	if metadata_file_type_is_symlink
		|| !metadata_is_regular_file
		|| metadata_hard_link_count != 1
		|| metadata_owner_uid != unsafe { libc::geteuid() }
	{
		return false;
	}

	true
}
"#;
		let path = Path::new("space_if_condition_block_expression.rs");
		let ctx = shared::read_file_context_from_text(path, text.to_owned())
			.expect("context")
			.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|violation| {
			violation.rule == "RUST-STYLE-SPACE-003"
				&& violation.message
					== "Insert exactly one blank line between different statement types."
		}));
		assert!(!edits.iter().any(|edit| edit.rule == "RUST-STYLE-SPACE-003"));

		let (rewritten, applied_count, _, _) =
			style::apply_fix_passes(path, text, true).expect("apply fix passes");

		assert_eq!(applied_count, 0);
		assert_eq!(rewritten, text);
	}

	#[test]
	fn space003_does_not_split_method_chain_after_struct_literal() {
		let text = r#"
fn sample() {
	let retry = Backoff {
		start_ms: args.retry_backoff_ms,
		cap_ms: args.retry_backoff_ms.saturating_mul(20).max(args.retry_backoff_ms),
		factor: 2,
		jitter_ratio: 0.2,
	}
	.retry_policy(args.retry_max);
}
"#;
		let ctx = shared::read_file_context_from_text(Path::new("space_chain.rs"), text.to_owned())
			.expect("context")
			.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-003"
			&& v.message == "Insert exactly one blank line between different statement types."));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-SPACE-003"));
	}

	#[test]
	fn space003_fix_preserves_attributes_and_removes_only_blank_lines() {
		let original = r#"
fn sample() {
	#[derive(Debug)]
	struct A;


	#[derive(Debug)]
	struct B;
}
"#;
		let ctx =
			shared::read_file_context_from_text(Path::new("space_attr.rs"), original.to_owned())
				.expect("context")
				.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-SPACE-003"
				&& v.message == "Insert exactly one blank line between local item declarations."
				&& v.fixable
		}));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains("struct A;\n\n\t#[derive(Debug)]\n\tstruct B;"));
	}

	#[test]
	fn space003_does_not_remove_blank_between_local_items() {
		let text = r#"
fn schema() {
	static SCHEMA: usize = 1;

	fn build_schema() -> usize {
		SCHEMA
	}
}
"#;
		let ctx =
			shared::read_file_context_from_text(Path::new("space_items_keep.rs"), text.to_owned())
				.expect("context")
				.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-003"
			&& v.message == "Do not insert blank lines within the same statement type."));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-SPACE-003"));
	}

	#[test]
	fn space003_fix_inserts_blank_between_local_items() {
		let original = r#"
fn schema() {
	static SCHEMA: usize = 1;
	fn build_schema() -> usize {
		SCHEMA
	}
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("space_items_insert.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-003"
			&& v.message == "Insert exactly one blank line between local item declarations."
			&& v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains("static SCHEMA: usize = 1;\n\n\tfn build_schema()"));
	}

	#[test]
	fn space003_const_group_has_no_blank_lines() {
		let original = r#"
fn topic_limits() {
	const TOPIC_MAX: usize = 24;

	const TOPIC_RATIO_NUM: usize = 2;

	const TOPIC_RATIO_DEN: usize = 3;
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("space_const_group.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-003"
			&& v.message == "Do not insert blank lines within constant declaration groups."
			&& v.fixable));
		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-003"
			&& v.message == "Insert exactly one blank line between local item declarations."));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains(
			"const TOPIC_MAX: usize = 24;\n\tconst TOPIC_RATIO_NUM: usize = 2;\n\tconst TOPIC_RATIO_DEN: usize = 3;"
		));
	}

	#[test]
	fn space003_treats_assert_macros_as_same_group() {
		let original = r#"
fn sample() {
	assert_eq!(1, 1);

	assert!((0.0..=1.0).contains(&0.5), "ok");
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("space_assert_macros.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-003"
			&& v.message == "Do not insert blank lines within the same statement type."
			&& v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(
			rewritten.contains("assert_eq!(1, 1);\n\tassert!((0.0..=1.0).contains(&0.5), \"ok\");")
		);
	}

	#[test]
	fn space003_does_not_split_match_pattern_alternation() {
		let text = r#"
enum Pred {
	Tag { strength: i32 },
	Entity { strength: i32 },
}

fn pred_strength(pred: &Pred) -> i32 {
	match pred {
		Pred::Tag { strength }
		| Pred::Entity { strength } => *strength,
	}
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("space_match_or_keep.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-003"
			&& v.message == "Insert exactly one blank line between different statement types."));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-SPACE-003"));
	}

	#[test]
	fn space003_fix_removes_blank_inside_match_pattern_alternation() {
		let original = r#"
enum Pred {
	Tag { strength: i32 },
	Entity { strength: i32 },
}

fn pred_strength(pred: &Pred) -> i32 {
	match pred {
		Pred::Tag { strength }

		| Pred::Entity { strength } => *strength,
	}
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("space_match_or_fix.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-003"
			&& v.message == "Do not insert blank lines inside a match pattern alternation."
			&& v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(
			rewritten
				.contains("Pred::Tag { strength }\n\t\t| Pred::Entity { strength } => *strength,")
		);
	}

	#[test]
	fn space004_fix_inserts_single_blank_before_return() {
		let original = r#"
fn sample(flag: bool) -> i32 {
	let value = 1;
	return if flag { value } else { 0 };
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("space004_return.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-SPACE-004"
				&& v.message == "Insert exactly one blank line before each return statement."
				&& v.fixable
		}));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains("let value = 1;\n\n\treturn if flag { value } else { 0 };"));
	}

	#[test]
	fn mod005_fix_removes_blank_lines_between_type_and_impl() {
		let original = r#"
struct RuntimeEvent {
	id: usize,
}

impl RuntimeEvent {
	fn id(&self) -> usize {
		self.id
	}
}
"#;
		let ctx = shared::read_file_context_from_text(Path::new("mod005.rs"), original.to_owned())
			.expect("context")
			.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-005" && v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains("}\nimpl RuntimeEvent {"));
	}

	#[test]
	fn mod002_fix_reorders_pub_items_before_non_pub_items() {
		let original = r#"
fn internal() -> usize {
	1
}

pub fn external() -> usize {
	2
}
"#;
		let ctx = shared::read_file_context_from_text(Path::new("mod002.rs"), original.to_owned())
			.expect("context")
			.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-002" && v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains("pub fn external() -> usize"));
		assert!(
			rewritten.find("pub fn external").unwrap_or_default()
				< rewritten.find("fn internal").unwrap_or_default()
		);
	}

	#[test]
	fn mod003_fix_reorders_non_async_before_async_with_same_visibility() {
		let original = r#"
pub async fn pull() -> usize {
	1
}

pub fn plan() -> usize {
	2
}
"#;
		let ctx = shared::read_file_context_from_text(Path::new("mod003.rs"), original.to_owned())
			.expect("context")
			.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-003" && v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(
			rewritten.find("pub fn plan").unwrap_or_default()
				< rewritten.find("pub async fn pull").unwrap_or_default()
		);
	}

	#[test]
	fn mod001_fix_reorders_cross_category_top_level_items() {
		let original = r#"
fn execute() -> usize {
	1
}

const LIMIT: usize = 3;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod001_cross_kind.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-001" && v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(
			rewritten.find("const LIMIT").unwrap_or_default()
				< rewritten.find("fn execute").unwrap_or_default()
		);
	}

	#[test]
	fn mod001_fix_reorders_use_before_macro_calls() {
		let original = r#"
macro_rules! define_placeholder {
	() => {};
}

define_placeholder! {}

pub use crate::api::Code;

define_placeholder! {}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod001_macro_call_reorder.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-001" && v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-MOD-001"));

		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");
		let use_pos = rewritten.find("pub use crate::api::Code;").expect("has use");
		let first_macro_call_pos =
			rewritten.find("define_placeholder! {}").expect("has macro call");

		assert!(use_pos < first_macro_call_pos);
	}

	#[test]
	fn mod001_fix_reorders_use_before_macro_calls_inside_inline_module() {
		let original = r#"
pub mod api_code {
	#[cfg(feature = "pubfi")]
	mod pubfi {
		def_api_codes! {
			ERR_A = -1,
			ERR_B = -2,
		}
	}

	def_api_codes! {
		OUTSIDE_A = -3,
	}

	#[cfg(feature = "pubfi")] pub use self::pubfi::*;

	#[cfg(feature = "pubfi")]
	def_api_codes! {
		OK = 0,
	}
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(Path::new("mod001_inline_module_macro_use.rs"), original, true)
				.expect("apply fix passes");
		let compact = rewritten.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
		let use_pos =
			compact.find("pubuseself::pubfi::{ERR_A,ERR_B};").expect("has rewritten pub use");
		let first_macro_pos = compact.find("OUTSIDE_A=-3").expect("has first macro call");
		let second_macro_pos = compact.find("OK=0").expect("has second macro call");

		assert!(use_pos < first_macro_pos);
		assert!(first_macro_pos < second_macro_pos);
		assert!(rewritten.contains("}\n\n\t#[cfg(feature = \"pubfi\")] pub use self::pubfi::{"));
		assert!(rewritten.contains("pub use self::pubfi::{ERR_A, ERR_B};\n\n\tdef_api_codes! {"));
	}

	#[test]
	fn mod001_fix_hoists_macro_rules_before_prior_nested_macro_call() {
		let original = r#"
pub mod api_code {
	#[cfg(feature = "pubfi")]
	pub mod pubfi {
		def_api_codes! {
			ERR_A = -1;
		}
	}

	pub use self::pubfi::{ERR_A};

	macro_rules! def_api_codes {
		($($name:ident = $code:expr;)*) => {
			$(pub const $name: i16 = $code;)*
		};
	}
}
"#;
		let (rewritten, _applied_count, _had_import_shortening_edits, _had_let_mut_reorder_edits) =
			style::apply_fix_passes(Path::new("mod001_hoist_macro_rules.rs"), original, true)
				.expect("apply fix passes");
		let macro_pos = rewritten.find("macro_rules! def_api_codes").expect("has macro rules");
		let mod_pos = rewritten.find("pub mod pubfi").expect("has pubfi module");

		assert!(macro_pos < mod_pos, "{rewritten}");
	}

	#[test]
	fn mod001_mod002_fix_reorders_use_blocks_split_by_mod_items() {
		let original = r#"use std::sync::LazyLock;

pub mod backoff;
#[cfg(feature = "crypto")] pub mod crypto;
pub mod retry;
#[cfg(feature = "serde")] pub mod serde_helpers;

#[cfg(feature = "crypto")] pub use crate::crypto::blake2b256;
pub use crate::{
	backoff::Backoff,
	retry::{RetryError, RetryPolicy},
};
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod001_mod002_split_use_blocks.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(
			violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-001" && v.fixable),
			"{violations:#?}"
		);
		assert!(
			violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-002" && v.fixable),
			"{violations:#?}"
		);
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-MOD-001"), "{edits:#?}");

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains(
			"pub mod backoff;\n#[cfg(feature = \"crypto\")] pub mod crypto;\npub mod retry;\n#[cfg(feature = \"serde\")] pub mod serde_helpers;"
		));
	}

	#[test]
	fn mod_group_spacing_fix_keeps_pub_mod_group_compact() {
		let original = r#"
pub mod backoff;

#[cfg(feature = "crypto")] pub mod crypto;

pub mod retry;

#[cfg(feature = "serde")] pub mod serde_helpers;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod_group_compact_spacing.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-SPACE-003"
				&& v.message == "Do not insert blank lines within module declaration groups."
				&& v.fixable
		}));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains(
			"pub mod backoff;\n#[cfg(feature = \"crypto\")] pub mod crypto;\npub mod retry;\n#[cfg(feature = \"serde\")] pub mod serde_helpers;"
		));
	}

	#[test]
	fn mod002_fix_inserts_blank_line_between_pub_and_pub_crate_mod_batches() {
		let original = r#"
pub mod backoff;
pub(crate) mod crypto;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod002_visibility_batch_spacing_mod.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-MOD-002"
				&& v.message
					== "Insert exactly one blank line between visibility batches within the same item kind."
				&& v.fixable
		}));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains("pub mod backoff;\n\npub(crate) mod crypto;"));
	}

	#[test]
	fn mod002_keeps_existing_blank_line_between_pub_and_pub_crate_mod_batches() {
		let original = r#"
pub mod backoff;

pub(crate) mod crypto;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod002_visibility_batch_spacing_mod_keep.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| {
			v.rule == "RUST-STYLE-SPACE-003"
				&& v.message == "Do not insert blank lines within module declaration groups."
		}));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-SPACE-003"));
	}

	#[test]
	fn mod002_fix_reorders_pub_items_across_interleaved_segments() {
		let original = r#"
fn internal_a() -> usize {
	1
}

const LIMIT: usize = 3;

pub fn external() -> usize {
	2
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod002_interleaved.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-002" && v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(
			rewritten.find("pub fn external").unwrap_or_default()
				< rewritten.find("fn internal_a").unwrap_or_default()
		);
	}

	#[test]
	fn mod001_fix_keeps_adjacent_top_level_const_group_compact() {
		let original = r#"
const CONTENT_CHAR_LIMIT: usize = 500;

fn helper() -> usize {
	1
}

const INSIGHTS_PER_FEED_LIMIT: usize = 3;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod001_const_compact.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-001" && v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains(
			"const CONTENT_CHAR_LIMIT: usize = 500;\nconst INSIGHTS_PER_FEED_LIMIT: usize = 3;\n\nfn helper() -> usize {"
		));
	}

	#[test]
	fn mod001_fix_keeps_blank_line_between_const_and_static_groups() {
		let original = r#"
static RE_HEADING: usize = 1;
const SANITIZE_EXPANSION_LIMIT: f32 = 1.3;
const SANITIZE_SHRINK_LIMIT: f32 = 0.6;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod001_const_static_grouping.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-001" && v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains(
			"const SANITIZE_EXPANSION_LIMIT: f32 = 1.3;\nconst SANITIZE_SHRINK_LIMIT: f32 = 0.6;\n\nstatic RE_HEADING: usize = 1;"
		));
	}

	#[test]
	fn mod002_fix_separates_pub_and_non_pub_const_groups() {
		let original = r#"
const INTERNAL_LIMIT: usize = 3;

pub const PUBLIC_LIMIT: usize = 5;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod002_const_compact.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-002" && v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(
			rewritten
				.contains("pub const PUBLIC_LIMIT: usize = 5;\n\nconst INTERNAL_LIMIT: usize = 3;")
		);
	}

	#[test]
	fn mod002_fix_inserts_blank_line_between_pub_and_pub_crate_const_batches_without_reorder() {
		let original = r#"
pub const PUBLIC_LIMIT: usize = 5;
pub(crate) const CRATE_LIMIT: usize = 3;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod002_visibility_batch_spacing_const.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-MOD-002"
				&& v.message
					== "Insert exactly one blank line between visibility batches within the same item kind."
				&& v.fixable
		}));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains(
			"pub const PUBLIC_LIMIT: usize = 5;\n\npub(crate) const CRATE_LIMIT: usize = 3;"
		));
	}

	#[test]
	fn mod002_keeps_existing_blank_line_between_pub_and_pub_crate_const_batches() {
		let original = r#"
pub const PUBLIC_LIMIT: usize = 5;

pub(crate) const CRATE_LIMIT: usize = 3;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod002_visibility_batch_spacing_const_keep.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| {
			v.rule == "RUST-STYLE-SPACE-003"
				&& v.message == "Do not insert blank lines within constant declaration groups."
		}));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-SPACE-003"));
	}

	#[test]
	fn mod004_reports_outer_doc_comment_on_inline_module() {
		let text = r#"
/// API helpers.
mod api {
	fn run() {}
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod004_outer_doc_inline_module.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-MOD-004"
				&& v.message
					== "Do not write module docs as outer comments on `mod`; place module docs inside the module with `//!`."
				&& !v.fixable
		}));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-MOD-004"));
	}

	#[test]
	fn mod004_reports_outer_doc_comment_on_external_module_declaration() {
		let text = r#"
/// API helpers.
mod api;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod004_outer_doc_external_module.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-004" && !v.fixable));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-MOD-004"));
	}

	#[test]
	fn mod004_allows_inner_doc_comment_inside_module_body() {
		let text = r#"
mod api {
	//! API helpers.
	fn run() {}
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod004_inner_doc_module.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-004"));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-MOD-004"));
	}

	#[test]
	fn mod004_allows_doc_control_attribute_on_module() {
		let text = r#"
#[doc(hidden)]
mod api {
	fn run() {}
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("mod004_doc_hidden_module.rs"),
			text.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-MOD-004"));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-MOD-004"));
	}

	#[test]
	fn space003_fix_removes_blank_lines_between_top_level_const_items() {
		let original = r#"
const CONTENT_CHAR_LIMIT: usize = 500;

const INSIGHTS_PER_FEED_LIMIT: usize = 3;

const PROD_PUBLIC_WEB_BASE_URL: &str = "https://pubfi.ai";
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("space003_top_const_group.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-SPACE-003"
				&& v.message == "Do not insert blank lines within constant declaration groups."
				&& v.fixable
		}));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains(
			"const CONTENT_CHAR_LIMIT: usize = 500;\nconst INSIGHTS_PER_FEED_LIMIT: usize = 3;\nconst PROD_PUBLIC_WEB_BASE_URL: &str = \"https://pubfi.ai\";"
		));
	}

	#[test]
	fn space003_fix_removes_extra_blank_lines_between_top_level_items() {
		let original = r#"
struct RoundGateGuard;

impl Drop for RoundGateGuard {
	fn drop(&mut self) {}
}



/// Bootstrap and run all crawlers.
pub async fn run() {}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("space003_top_level_extra_blank.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| {
			v.rule == "RUST-STYLE-SPACE-003"
				&& v.message == "Do not insert extra blank lines between top-level items."
				&& v.fixable
		}));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains("}\n\n/// Bootstrap and run all crawlers."));
		assert!(!rewritten.contains("}\n\n\n/// Bootstrap and run all crawlers."));
	}

	#[test]
	fn space_rules_ignore_statements_inside_raw_string_literals() {
		let text = r##"
fn sample() {
	let prompt = r#"
	#[cfg(test)]
	mod tests {
		use super::*;
		fn fake_case() {}
	}
	"#;

	println!("{prompt}");
}
"##;
		let ctx =
			shared::read_file_context_from_text(Path::new("space_raw_string.rs"), text.to_owned())
				.expect("context")
				.expect("has ctx");
		let (violations, _edits) = style::collect_violations(&ctx, true);

		assert!(
			!violations
				.iter()
				.any(|v| v.rule == "RUST-STYLE-SPACE-003" || v.rule == "RUST-STYLE-SPACE-004")
		);
	}

	#[test]
	fn space004_keeps_binary_expression_continuations() {
		let text = r#"
fn safe_tail(flag: bool) -> bool {
    flag
        && unsafe { check() }
        && other()
}
fn real_gap(flag: bool) -> bool {
    record();
    flag
        && unsafe { check() }
        && other()
}
"#;
		let ctx = shared::read_file_context_from_text(Path::new("binary_tail.rs"), text.to_owned())
			.expect("context")
			.expect("has ctx");
		let (violations, _) = style::collect_violations(&ctx, true);
		let tail_lines = violations
			.iter()
			.filter(|v| v.rule == "RUST-STYLE-SPACE-004")
			.map(|v| v.line)
			.collect::<Vec<_>>();

		assert_eq!(tail_lines, vec![9]);
	}

	#[test]
	fn space004_fix_remains_autofixable_with_char_literal_conditions() {
		let original = r#"
fn classify(ch: char) -> usize {
	if ch == '\'' {
		let value = 1;
		return value;
	}

	0
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("space_char_literal.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-SPACE-004" && v.fixable));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains("let value = 1;\n\n\t\treturn value;"));
	}

	#[test]
	fn type001_flags_only_meaningless_aliases() {
		let original = r#"
type A = B;
type JsonValue = JsonValue;
type A<T> = B<T>;
type A<'a, T> = B<'a, T>;
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("types_rule_hits.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, _edits) = style::collect_violations(&ctx, true);
		let matches =
			violations.iter().filter(|v| v.rule == "RUST-STYLE-TYPE-001").collect::<Vec<_>>();

		assert_eq!(matches.len(), 4);
		assert!(matches.iter().all(|v| v.fixable));
	}

	#[test]
	fn type001_skips_specialized_or_non_path_aliases() {
		let original = r#"
		type Bytes = Vec<u8>;
		type MyResult<T> = Result<T, MyError>;
	type MyResultWithDefault<T, E = Error> = std::result::Result<T, E>;
	type Span = (usize, usize);
	"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("types_rule_skips.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, _edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-TYPE-001"));
	}

	#[test]
	fn type001_skips_associated_types_in_impl_blocks() {
		let original = r#"
trait Service {
	type Response;
	type Error;
}

struct Wrapper;

impl Service for Wrapper {
	type Response = String;
	type Error = std::io::Error;
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("types_rule_impl_associated_types.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, _edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-TYPE-001"));
	}

	#[test]
	fn type001_fix_removes_primitive_public_alias_and_rewrites_uses() {
		let original = r#"
pub type Code = i16;

	pub struct ApiEnvelope {
		pub code: Code,
	}
"#;
		let path = Path::new("types_rule_primitive_alias_fix.rs");
		let ctx = shared::read_file_context_from_text(path, original.to_owned())
			.expect("context")
			.expect("has ctx");
		let type_fixes = types::collect_type_alias_rename_fixes(&ctx);

		assert_eq!(type_fixes.len(), 1);

		let plan = &type_fixes[0];
		let mut renames = BTreeMap::new();
		let mut edits = plan.definition_edits.clone();

		renames.insert(plan.alias.clone(), plan.target.clone());

		let skip_ranges = edits.iter().map(|edit| (edit.start, edit.end)).collect::<Vec<_>>();
		let usage_edits = types::build_type_alias_usage_rename_edits(&ctx, &renames, &skip_ranges);

		edits.extend(usage_edits);

		let mut rewritten = original.to_owned();
		let _applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(!rewritten.contains("type Code"));
		assert!(rewritten.contains("pub code: i16,"));
	}

	#[test]
	fn type001_private_alias_is_autofixable() {
		let original = r#"
type Hidden = i32;

fn produce() -> Hidden {
	Hidden
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("types_rule_private_alias.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, _edits) = style::collect_violations(&ctx, true);

		assert!(violations.iter().any(|v| v.rule == "RUST-STYLE-TYPE-001" && v.fixable));

		let type_fixes = types::collect_type_alias_rename_fixes(&ctx);

		assert_eq!(type_fixes.len(), 1);

		let plan = &type_fixes[0];

		assert_eq!(plan.alias, "Hidden");
		assert_eq!(plan.target, "i32");
		assert!(plan.definition_edits.iter().any(|edit| edit.rule == "RUST-STYLE-TYPE-001"));

		let mut renames = BTreeMap::new();

		renames.insert(plan.alias.clone(), plan.target.clone());

		let skip_ranges =
			plan.definition_edits.iter().map(|edit| (edit.start, edit.end)).collect::<Vec<_>>();
		let usage_edits = types::build_type_alias_usage_rename_edits(&ctx, &renames, &skip_ranges);

		assert!(usage_edits.iter().any(|edit| edit.replacement == "i32"));
	}

	#[test]
	fn generics002_fixes_typed_collect_turbofish() {
		let original = r#"
fn sample() {
	let values = vec![0_u8, 1];
	let iter = values.into_iter();
	let v: Vec<u8> = iter.collect::<Vec<u8>>();
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("generics002_collect.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);
		let matches =
			violations.iter().filter(|v| v.rule == "RUST-STYLE-GENERICS-002").collect::<Vec<_>>();

		assert_eq!(matches.len(), 1);
		assert!(matches.iter().all(|v| v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-GENERICS-002"));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert_eq!(
			rewritten,
			r#"
fn sample() {
	let values = vec![0_u8, 1];
	let iter = values.into_iter();
	let v: Vec<u8> = iter.collect();
}
"#
		);
	}

	#[test]
	fn generics002_fixes_typed_constructor_turbofish() {
		let original = r#"
use std::collections::HashMap;

fn sample() {
	let m: HashMap<u8, u8> = HashMap::<u8, u8>::new();
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("generics002_constructor.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);
		let matches =
			violations.iter().filter(|v| v.rule == "RUST-STYLE-GENERICS-002").collect::<Vec<_>>();

		assert_eq!(matches.len(), 1);
		assert!(matches.iter().all(|v| v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-GENERICS-002"));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert_eq!(
			rewritten,
			r#"
use std::collections::HashMap;

fn sample() {
	let m: HashMap<u8, u8> = HashMap::new();
}
"#
		);
	}

	#[test]
	fn generics002_skips_untyped_collect_turbofish() {
		let original = r#"
fn sample(iter: impl Iterator<Item = Vec<u8>>) {
	let v = iter.collect::<Vec<u8>>();
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("generics002_untyped_skip.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, _edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-GENERICS-002"));
	}

	#[test]
	fn generics002_skips_inferred_collect_wildcard() {
		let original = r#"
fn sample(iter: impl Iterator<Item = u8>) {
	let v: Vec<u8> = iter.collect::<Vec<_>>();
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("generics002_wildcard_skip.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, _edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-GENERICS-002"));
	}

	#[test]
	fn generics003_fixes_canonical_constructor_turbofish() {
		let original = r#"
fn sample() {
	let _ = <Vec<u8>>::new();
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("generics003_vec_constructor.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);
		let matches =
			violations.iter().filter(|v| v.rule == "RUST-STYLE-GENERICS-003").collect::<Vec<_>>();

		assert_eq!(matches.len(), 1);
		assert!(matches.iter().all(|v| v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-GENERICS-003"));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert_eq!(
			rewritten,
			r#"
fn sample() {
	let _ = Vec::<u8>::new();
}
"#
		);
	}

	#[test]
	fn generics003_fixes_canonical_collection_turbofish() {
		let original = r#"
fn sample() {
	let _ = <std::collections::HashMap<u8, u8>>::new();
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("generics003_hashmap_constructor.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);
		let matches =
			violations.iter().filter(|v| v.rule == "RUST-STYLE-GENERICS-003").collect::<Vec<_>>();

		assert_eq!(matches.len(), 1);
		assert!(matches.iter().all(|v| v.fixable));
		assert!(edits.iter().any(|e| e.rule == "RUST-STYLE-GENERICS-003"));

		let mut rewritten = original.to_owned();
		let applied = fixes::apply_edits(&mut rewritten, edits).expect("apply edits");

		assert!(applied >= 1);
		assert!(rewritten.contains("let _ = std::collections::HashMap::<u8, u8>::new();"));
	}

	#[test]
	fn generics003_skips_disambiguated_trait_path() {
		let original = r#"
trait Trait {
	type Assoc;
}

fn sample() {
	let _ = <T as Trait>::Assoc;
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("generics003_disambiguated_skip.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-GENERICS-003"));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-GENERICS-003"));
	}

	#[test]
	fn generics003_skips_non_path_type_anchor() {
		let original = r#"
fn sample() {
	let _ = <(u8, u8)>::something;
}
"#;
		let ctx = shared::read_file_context_from_text(
			Path::new("generics003_non_path_anchor_skip.rs"),
			original.to_owned(),
		)
		.expect("context")
		.expect("has ctx");
		let (violations, edits) = style::collect_violations(&ctx, true);

		assert!(!violations.iter().any(|v| v.rule == "RUST-STYLE-GENERICS-003"));
		assert!(!edits.iter().any(|e| e.rule == "RUST-STYLE-GENERICS-003"));
	}

	#[test]
	fn should_stop_tune_round_stops_when_no_fixes_applied() {
		let (should_stop, non_decreasing_rounds) =
			style::should_stop_tune_round(0, 10, 12, 1, true);

		assert!(should_stop);
		assert_eq!(non_decreasing_rounds, 1);
	}

	#[test]
	fn should_stop_tune_round_stops_when_no_fixable_violations() {
		let (should_stop, non_decreasing_rounds) = style::should_stop_tune_round(2, 0, 12, 1, true);

		assert!(should_stop);
		assert_eq!(non_decreasing_rounds, 1);
	}

	#[test]
	fn should_skip_tune_rounds_when_initial_scan_has_no_fixable_violations() {
		assert!(super::should_skip_tune_rounds(0));
	}

	#[test]
	fn should_not_skip_tune_rounds_when_fixable_violations_remain() {
		assert!(!super::should_skip_tune_rounds(1));
	}

	#[test]
	fn should_stop_tune_round_resets_streak_when_fixable_count_decreases() {
		let (should_stop, non_decreasing_rounds) = style::should_stop_tune_round(2, 8, 12, 2, true);

		assert!(!should_stop);
		assert_eq!(non_decreasing_rounds, 0);
	}

	#[test]
	fn should_stop_tune_round_continues_on_first_non_decreasing_round() {
		let (should_stop, non_decreasing_rounds) =
			style::should_stop_tune_round(2, 12, 12, 0, true);

		assert!(!should_stop);
		assert_eq!(non_decreasing_rounds, 1);
	}

	#[test]
	fn should_stop_tune_round_stops_on_second_consecutive_non_decreasing_round() {
		let (should_stop, non_decreasing_rounds) =
			style::should_stop_tune_round(2, 12, 12, 1, true);

		assert!(should_stop);
		assert_eq!(non_decreasing_rounds, 2);
	}

	#[test]
	fn should_stop_tune_round_stops_when_follow_up_round_is_not_needed() {
		let (should_stop, non_decreasing_rounds) =
			style::should_stop_tune_round(2, 8, 12, 0, false);

		assert!(should_stop);
		assert_eq!(non_decreasing_rounds, 0);
	}

	#[test]
	fn resolve_fix_round_scopes_workspace_splits_to_package_scopes() {
		let cargo_options = shared::CargoOptions {
			workspace: true,
			..shared::CargoOptions::new(shared::StyleLanguage::Rust)
		};
		let workspace_files =
			shared::resolve_files(&cargo_options).expect("resolve workspace files");
		let scopes = super::resolve_fix_round_scopes(&cargo_options).expect("resolve fix scopes");
		let scoped_files = scopes
			.iter()
			.flat_map(|(files, _options)| files.iter().cloned())
			.collect::<BTreeSet<_>>();
		let workspace_files = workspace_files.into_iter().collect::<BTreeSet<_>>();

		assert!(!scopes.is_empty());
		assert!(scopes.iter().all(|(files, options)| !files.is_empty() && !options.workspace));
		assert!(scopes.iter().all(|(_, options)| !options.packages.is_empty()));
		assert_eq!(scoped_files, workspace_files);
	}

	#[test]
	fn should_parallelize_fix_scopes_when_multiple_scopes_are_disjoint() {
		let scopes = vec![
			(
				vec![PathBuf::from("a/src/lib.rs"), PathBuf::from("a/src/mod.rs")],
				shared::CargoOptions {
					packages: vec!["a".to_owned()],
					..shared::CargoOptions::new(shared::StyleLanguage::Rust)
				},
			),
			(
				vec![PathBuf::from("b/src/lib.rs")],
				shared::CargoOptions {
					packages: vec!["b".to_owned()],
					..shared::CargoOptions::new(shared::StyleLanguage::Rust)
				},
			),
		];

		assert!(super::should_parallelize_fix_scopes(&scopes));
	}

	#[test]
	fn should_not_parallelize_fix_scopes_when_files_overlap() {
		let scopes = vec![
			(
				vec![PathBuf::from("shared/src/lib.rs")],
				shared::CargoOptions {
					packages: vec!["a".to_owned()],
					..shared::CargoOptions::new(shared::StyleLanguage::Rust)
				},
			),
			(
				vec![PathBuf::from("shared/src/lib.rs"), PathBuf::from("b/src/lib.rs")],
				shared::CargoOptions {
					packages: vec!["b".to_owned()],
					..shared::CargoOptions::new(shared::StyleLanguage::Rust)
				},
			),
		];

		assert!(!super::should_parallelize_fix_scopes(&scopes));
	}

	#[test]
	fn net_file_change_detection_matches_snapshot_delta() {
		let nanos = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.expect("current timestamp")
			.as_nanos();
		let path =
			env::temp_dir().join(format!("vstyle-net-change-{}-{}.rs", process::id(), nanos));

		fs::write(&path, "fn sample() {}\n").expect("seed temp file");

		let snapshots = super::collect_file_snapshots(slice::from_ref(&path));

		assert!(!super::has_net_file_changes(&snapshots));

		fs::write(&path, "fn changed() {}\n").expect("mutate temp file");

		assert!(super::has_net_file_changes(&snapshots));

		let _ = fs::remove_file(&path);
	}

	#[test]
	fn refresh_check_state_for_changed_files_updates_fixable_totals() {
		let nanos = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.expect("current timestamp")
			.as_nanos();
		let violating_path =
			env::temp_dir().join(format!("vstyle-check-state-a-{}-{}.rs", process::id(), nanos));
		let clean_path =
			env::temp_dir().join(format!("vstyle-check-state-b-{}-{}.rs", process::id(), nanos));
		let files = vec![violating_path.clone(), clean_path.clone()];

		fs::write(&violating_path, "fn sample() {\n\tlet value = 10f32;\n}\n")
			.expect("seed violating file");
		fs::write(&clean_path, "fn sample() {\n\tlet value = 10_f32;\n}\n")
			.expect("seed clean file");

		let counts_by_file = super::collect_file_violation_counts(&files).expect("initial counts");
		let violation_count =
			counts_by_file.values().map(|counts| counts.violation_count).sum::<usize>();
		let unfixable_count =
			counts_by_file.values().map(|counts| counts.unfixable_count).sum::<usize>();
		let mut check_state =
			super::CheckState { counts_by_file, violation_count, unfixable_count };

		assert_eq!(check_state.fixable_count(), 1);

		fs::write(&violating_path, "fn sample() {\n\tlet value = 10_f32;\n}\n")
			.expect("fix violating file");
		super::refresh_check_state_for_files(slice::from_ref(&violating_path), &mut check_state)
			.expect("refresh changed file counts");

		assert_eq!(check_state.fixable_count(), 0);
		assert_eq!(check_state.violation_count, 0);
		assert_eq!(
			check_state
				.counts_by_file
				.get(&violating_path)
				.expect("updated violating file counts")
				.violation_count,
			0
		);

		let _ = fs::remove_file(&violating_path);
		let _ = fs::remove_file(&clean_path);
	}

	#[test]
	fn run_fix_round_skips_semantic_for_no_change_scope() {
		let nanos = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.expect("current timestamp")
			.as_nanos();
		let path =
			env::temp_dir().join(format!("vstyle-noop-round-{}-{}.rs", process::id(), nanos));

		fs::write(&path, "fn already_clean() {}\n").expect("seed temp file");
		semantic::reset_cache_stats();

		let summary = super::run_fix_round(
			slice::from_ref(&path),
			&shared::CargoOptions::new(shared::StyleLanguage::Rust),
			false,
			false,
		)
		.expect("run no-op fix round");
		let stats = semantic::cache_stats();

		assert_eq!(summary.applied_count, 0);
		assert!(!summary.requires_follow_up_round);
		assert_eq!(stats.misses, 0);

		let _ = fs::remove_file(&path);
	}

	#[test]
	fn shortening_preserves_receiver_generics_and_nested_type_edits() {
		let source = r"pub fn value() {
    let value = std::mem::MaybeUninit::<std::ffi::c_int>::uninit();
    let values = std::iter::Iterator::collect::<std::vec::Vec<std::ffi::c_int>>(0..3);
    let _ = (value, values);
}";
		let root = env::temp_dir().join(format!("vstyle-receiver-generics-{}", process::id()));
		let path = root.join("lib.rs");

		fs::create_dir_all(&root).expect("Create fixture directory.");

		let (rewritten, applied, _, _) =
			style::apply_fix_passes(&path, source, true).expect("Apply fixes.");

		assert!(applied > 0);
		assert!(rewritten.contains("MaybeUninit::<c_int>::uninit()"), "{rewritten}");
		assert!(rewritten.contains("Iterator::collect::<Vec<c_int>>"), "{rewritten}");

		let (repeated, _, _, _) =
			style::apply_fix_passes(&path, &rewritten, true).expect("Repeat fixes.");

		assert_eq!(rewritten, repeated);

		fs::write(&path, &rewritten).expect("Write rewritten fixture.");

		let output = process::Command::new("rustc")
			.args(["--edition=2024", "--crate-type=lib", "--emit=metadata"])
			.arg(&path)
			.arg("-o")
			.arg(root.join("fixture.rmeta"))
			.output()
			.expect("Compile rewritten fixture.");

		fs::remove_dir_all(root).expect("Remove fixture.");

		assert!(
			output.status.success(),
			"{rewritten}\n{}",
			String::from_utf8_lossy(&output.stderr)
		);
	}

	#[test]
	fn module_sort_keeps_attached_impls_with_types() {
		let body = r"#[derive(Clone)]
struct Private;
impl Private { fn value() {} }
impl Default for Private { fn default() -> Self { Self } }

/// Public fixture type.
pub struct Public;
impl Public { pub fn value() {} }

pub enum Choice { One }
impl Choice { pub fn value() -> Self { Self::One } }
";

		for original in [body.to_owned(), format!("mod nested {{\n{body}}}\n")] {
			let path = Path::new("attached_impls.rs");
			let ctx = shared::read_file_context_from_text(path, original.clone())
				.expect("Parse input.")
				.expect("Input context.");
			let mut violations = Vec::new();
			let mut edits = Vec::new();

			module::check_module_order(&ctx, &mut violations, &mut edits, true);

			let mut sorted = original.clone();

			fixes::apply_edits(&mut sorted, edits).expect("Sort module items.");

			assert!(sorted.contains("pub struct Public;\nimpl Public"), "{sorted}");

			let (rewritten, _, _, _) =
				style::apply_fix_passes(path, &original, true).expect("Apply fixes.");
			let ctx = shared::read_file_context_from_text(path, rewritten.clone())
				.expect("Parse fixture.")
				.expect("Fixture context.");
			let (violations, _) = style::collect_violations(&ctx, false);

			assert!(
				!violations
					.iter()
					.any(|v| matches!(v.rule, "RUST-STYLE-MOD-002" | "RUST-STYLE-MOD-005")),
				"{rewritten}\n{violations:?}"
			);
			assert!(rewritten.contains("#[derive(Clone)]\nstruct Private;"), "{rewritten}");
			assert!(
				rewritten.contains("/// Public fixture type.\npub struct Public;"),
				"{rewritten}"
			);

			let (repeated, _, _, _) =
				style::apply_fix_passes(path, &rewritten, true).expect("Repeat fixes.");

			assert_eq!(rewritten, repeated);
		}
	}

	#[test]
	fn root_crate_qualification_does_not_import_the_crate_again() {
		let original = "use fixture::First;\nuse fixture::Second;\npub fn first() -> First { fixture::First {} }\npub fn second() -> Second { fixture::Second {} }\n";
		let root = env::temp_dir().join(format!("vstyle-root-import-{}", process::id()));
		let path = root.join("consumer.rs");
		let provider = root.join("provider.rs");
		let library = root.join("libfixture.rlib");
		let ctx = shared::read_file_context_from_text(&path, original.to_owned())
			.expect("Parse input.")
			.expect("Input context.");
		let (_, edits) = style::collect_violations(&ctx, true);
		let edits = edits.into_iter().filter(|edit| edit.rule == "RUST-STYLE-IMPORT-009").collect();
		let mut rewritten = original.to_owned();

		fixes::apply_edits(&mut rewritten, edits).expect("Qualify imports.");

		assert!(!rewritten.contains("use fixture"), "{rewritten}");
		assert!(rewritten.contains("-> fixture::First"), "{rewritten}");
		assert!(rewritten.contains("-> fixture::Second"), "{rewritten}");

		fs::create_dir_all(&root).expect("Create fixture directory.");
		fs::write(&provider, "pub struct First {}\npub struct Second {}\n")
			.expect("Write provider.");
		fs::write(&path, &rewritten).expect("Write consumer.");

		let compiled = process::Command::new("rustc")
			.args(["--edition=2024", "--crate-name=fixture", "--crate-type=rlib"])
			.arg(&provider)
			.arg("-o")
			.arg(&library)
			.output()
			.expect("Compile provider.");

		assert!(compiled.status.success(), "{}", String::from_utf8_lossy(&compiled.stderr));

		let compiled = process::Command::new("rustc")
			.args(["--edition=2024", "--crate-type=lib", "--emit=metadata", "--extern"])
			.arg(format!("fixture={}", library.display()))
			.arg(&path)
			.arg("-o")
			.arg(root.join("consumer.rmeta"))
			.output()
			.expect("Compile consumer.");

		fs::remove_dir_all(root).expect("Remove fixture.");

		assert!(
			compiled.status.success(),
			"{rewritten}\n{}",
			String::from_utf8_lossy(&compiled.stderr)
		);
	}

	#[test]
	fn import_shortening_merges_last_use_without_overlapping_new_imports() {
		let root = env::temp_dir().join(format!("vstyle-last-use-{}", process::id()));
		let path = root.join("last_use.rs");

		fs::create_dir_all(&root).expect("Create fixture directory.");

		for separator in ["\n", " "] {
			let original = format!(
				"use std::collections;{separator}pub struct Fixture {{ pub map: std::collections::BTreeMap<String, String>, pub path: std::path::PathBuf }}\n"
			);
			let (rewritten, applied, _, _) =
				style::apply_fix_passes(&path, &original, true).expect("Apply fixes.");

			assert!(applied > 0, "{rewritten}");
			assert!(rewritten.contains("pub map: BTreeMap<String, String>"), "{rewritten}");
			assert!(rewritten.contains("pub path: PathBuf"), "{rewritten}");

			let ctx = shared::read_file_context_from_text(&path, rewritten.clone())
				.expect("Parse output.")
				.expect("Output context.");
			let (violations, _) = style::collect_violations(&ctx, false);

			assert!(
				!violations.iter().any(|v| v.rule == "RUST-STYLE-IMPORT-008"),
				"{violations:?}"
			);

			fs::write(&path, &rewritten).expect("Write rewritten fixture.");

			let compiled = process::Command::new("rustc")
				.args(["--edition=2024", "--crate-type=lib", "--emit=metadata"])
				.arg(&path)
				.arg("-o")
				.arg(root.join("fixture.rmeta"))
				.output()
				.expect("Compile rewritten fixture.");

			assert!(
				compiled.status.success(),
				"{rewritten}\n{}",
				String::from_utf8_lossy(&compiled.stderr)
			);
		}

		fs::remove_dir_all(root).expect("Remove fixture.");
	}
	#[test]
	fn import_shortening_keeps_shared_guarded_type_paths_consistent() {
		let root = env::temp_dir().join(format!("vstyle-guarded-type-{}", process::id()));
		let path = root.join("guarded.rs");
		let original = "pub fn ordinary(value: std::path::PathBuf) -> usize { value.as_os_str().len() }\n#[cfg(test)]\npub fn guarded(value: std::path::PathBuf) -> usize { value.as_os_str().len() }\n#[cfg(any())]\npub fn unavailable(_: unavailable_dependency::Missing) {}\n";
		let (rewritten, _, _, _) =
			style::apply_fix_passes(&path, original, true).expect("Apply fixes.");
		let ctx = shared::read_file_context_from_text(&path, rewritten.clone())
			.expect("Parse output.")
			.expect("Output context.");
		let (violations, _) = style::collect_violations(&ctx, false);

		assert!(rewritten.contains("ordinary(value: PathBuf)"), "{rewritten}");
		assert!(rewritten.contains("guarded(value: PathBuf)"), "{rewritten}");
		assert!(rewritten.contains("unavailable_dependency::Missing"), "{rewritten}");
		assert!(!rewritten.contains("use unavailable_dependency"), "{rewritten}");
		assert!(
			!violations
				.iter()
				.any(|v| matches!(v.rule, "RUST-STYLE-IMPORT-008" | "RUST-STYLE-IMPORT-009")),
			"{rewritten}\n{violations:?}"
		);

		let (repeated, _, _, _) =
			style::apply_fix_passes(&path, &rewritten, true).expect("Repeat fixes.");

		assert_eq!(rewritten, repeated);

		fs::create_dir_all(&root).expect("Create fixture directory.");
		fs::write(&path, &rewritten).expect("Write fixture.");

		for configuration in [None, Some("test")] {
			let mut command = process::Command::new("rustc");

			command.args(["--edition=2024", "--crate-type=lib", "--emit=metadata"]);

			if let Some(configuration) = configuration {
				command.args(["--cfg", configuration]);
			}

			let compiled = command
				.arg(&path)
				.arg("-o")
				.arg(root.join("fixture.rmeta"))
				.output()
				.expect("Compile fixture.");

			assert!(
				compiled.status.success(),
				"{rewritten}\n{}",
				String::from_utf8_lossy(&compiled.stderr)
			);
		}

		fs::remove_dir_all(root).expect("Remove fixture.");
	}
	#[test]
	fn import_group_cleanup_preserves_module_bindings_used_by_children() {
		let root = env::temp_dir().join(format!("vstyle-child-module-binding-{}", process::id()));
		let src = root.join("src");
		let parent = src.join("parent.rs");

		fs::create_dir_all(&src).expect("Create source directory.");
		fs::write(
			root.join("Cargo.toml"),
			"[package]\nname = \"child-binding\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
		)
		.expect("Write manifest.");
		fs::write(src.join("lib.rs"), "pub mod theme { pub struct Marker; }\npub mod parent;\n")
			.expect("Write root.");

		for imports in ["use crate::{theme::{self, Marker}};", "use crate::{theme, theme::Marker};"]
		{
			for child in [
				"use super::theme; pub fn read() -> usize { std::mem::size_of::<theme::Marker>() }",
				"use super::*; pub fn read() -> usize { std::mem::size_of::<theme::Marker>() }",
				"use crate::parent::theme; pub fn read() -> usize { std::mem::size_of::<theme::Marker>() }",
				"use super::theme::Marker; pub fn read() -> usize { std::mem::size_of::<Marker>() }",
				"use crate::parent::{theme::Marker}; pub fn read() -> usize { std::mem::size_of::<Marker>() }",
			] {
				let original = format!(
					"{imports}\n#[path = \"consumer.rs\"] mod child;\npub fn marker() -> Marker {{ Marker }}\npub fn read() -> usize {{ child::read() }}\n"
				);

				fs::write(&parent, &original).expect("Write parent.");
				fs::write(src.join("consumer.rs"), child).expect("Write child.");

				let (rewritten, _, _, _) =
					style::apply_fix_passes(&parent, &original, true).expect("Apply fixes.");

				fs::write(&parent, &rewritten).expect("Write rewritten parent.");

				let compiled = process::Command::new("rustc")
					.args(["--edition=2024", "--crate-type=lib", "--emit=metadata"])
					.arg(src.join("lib.rs"))
					.arg("-o")
					.arg(root.join("fixture.rmeta"))
					.output()
					.expect("Compile fixture.");

				assert!(
					compiled.status.success(),
					"{rewritten}\n{}",
					String::from_utf8_lossy(&compiled.stderr)
				);

				let (repeated, _, _, _) =
					style::apply_fix_passes(&parent, &rewritten, true).expect("Repeat fixes.");

				assert_eq!(rewritten, repeated);
			}
		}

		fs::remove_dir_all(root).expect("Remove fixture.");
	}
	#[test]
	fn import_normalization_and_group_order_converge() {
		let original = "use crate::{theme::{self, Color}};\n\nuse std::path::PathBuf;\n\npub fn take(_: Color, _: PathBuf) {}\n";
		let path = Path::new("src/consumer.rs");
		let (rewritten, count, _, _) =
			style::apply_fix_passes(path, original, true).expect("Apply fixes.");
		let ctx = shared::read_file_context_from_text(path, rewritten.clone())
			.expect("Parse.")
			.expect("Context.");
		let (violations, _) = style::collect_violations(&ctx, false);

		assert!(count > 0, "{rewritten}");
		assert!(!rewritten.contains("self, Color"), "{rewritten}");
		assert!(
			rewritten.find("use std::").expect("Standard import")
				< rewritten.find("use crate::").expect("Local import")
		);
		assert!(
			!violations
				.iter()
				.any(|v| matches!(v.rule, "RUST-STYLE-IMPORT-001" | "RUST-STYLE-IMPORT-002")),
			"{violations:?}"
		);

		let (repeated, _, _, _) =
			style::apply_fix_passes(path, &rewritten, true).expect("Repeat fixes.");

		assert_eq!(rewritten, repeated);
	}

	#[test]
	fn spacing_fix_preserves_cfg_literals_and_unicode() {
		let original = "fn main() {\n    println!(\"界\");\n\n    #[cfg(target_os = \"macos\")]\n    println!(r#\"literal\n\n界\"#);\n}\n";
		let path = Path::new("spacing_cfg.rs");
		let (rewritten, count, _, _) =
			style::apply_fix_passes(path, original, true).expect("Apply fixes.");
		let expected = original.replace(";\n\n    #[cfg", ";\n    #[cfg");

		assert!(count > 0);
		assert_eq!(rewritten, expected);

		let (repeated, _, _, _) =
			style::apply_fix_passes(path, &rewritten, true).expect("Repeat fixes.");

		assert_eq!(rewritten, repeated);
	}
	#[test]
	fn module_layout_preserves_import_group_separators() {
		let original = "struct Thing;\n\n#[cfg(test)]\nmod tests {\n    use std::fmt::{Debug, Display};\n    use std::path::Path;\n\n    use crate::Thing;\n\n    fn accepts<T: Debug + Display>(_: &Path, _: &Thing) {}\n}\n";
		let path = Path::new("src/lib.rs");
		let ctx = shared::read_file_context_from_text(path, original.into())
			.expect("Parse.")
			.expect("Context.");
		let mut violations = Vec::new();
		let mut edits = Vec::new();

		module::check_module_order(&ctx, &mut violations, &mut edits, true);

		assert!(edits.is_empty(), "{edits:?}");

		let (rewritten, _, _, _) =
			style::apply_fix_passes(path, original, true).expect("Apply fixes.");
		let (repeated, count, _, _) =
			style::apply_fix_passes(path, &rewritten, true).expect("Repeat fixes.");

		assert_eq!(rewritten, repeated);
		assert_eq!(count, 0, "Import grouping and module layout must not oscillate.");
	}
}
