use std::{
	collections::{BTreeMap, BTreeSet},
	fs, mem,
	path::{Path, PathBuf},
	time::{Duration, Instant},
};

use color_eyre::{Result, eyre};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};

use crate::style::{
	fixes, rules, semantic,
	shared::{self, CargoOptions, RunSummary, Violation},
	swift, types, workspace,
};

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
	let_mut_reorder_files: &'a BTreeSet<PathBuf>,
	type_alias_rename_files: &'a BTreeSet<PathBuf>,
}

#[derive(Debug)]
struct FileFixApplication {
	changed_files: Vec<PathBuf>,
	total_applied: usize,
	import_fallbacks: BTreeMap<PathBuf, ImportFallbackPlan>,
	changed_originals: BTreeMap<PathBuf, (PathBuf, String)>,
	let_mut_reorder_files: BTreeSet<PathBuf>,
	type_alias_rename_files: BTreeSet<PathBuf>,
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

pub(crate) fn run_fix(
	cargo_options: &CargoOptions,
	verbose: bool,
	progress: bool,
) -> Result<RunSummary> {
	let files = workspace::resolve_files(cargo_options)?;
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

	Ok(checked)
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
	let files = workspace::resolve_files(cargo_options)?;
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
	let files = workspace::resolve_files(cargo_options)?;

	if !cargo_options.workspace || !cargo_options.packages.is_empty() {
		return Ok(vec![(files, cargo_options.clone())]);
	}

	let Some(files_by_package) = workspace::package_file_map_for_files(&files)? else {
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
	progress: bool,
) -> Result<BTreeSet<PathBuf>> {
	if progress {
		eprintln!("vstyle tune: [{scope_label}] validating the pre-edit semantic baseline.");
	}

	let (stdout, error_files) =
		semantic::collect_compiler_error_files_with_output(files, cargo_options, progress)?;

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
			progress,
		)?;
	let post_edit_succeeded = semantic::semantic_check_succeeded(&post_edit_stdout) == Some(true);
	let semantic_phase_start_snapshot = collect_file_snapshots(&file_fixes.changed_files);
	let semantic_applied = semantic::apply_semantic_fixes(
		&file_fixes.changed_files,
		cargo_options,
		Some(post_edit_stdout),
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
				progress,
			)?;

		if semantic::semantic_check_succeeded(&final_stdout) != Some(true) {
			if recover_without_binding_rewrites(file_fixes, cargo_options, progress)? {
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
	let type_alias_plan = types::TypeAliasRenamePlan::collect(files)?;
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
		let_mut_reorder_files: BTreeSet::new(),
		type_alias_rename_files: BTreeSet::new(),
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
			.insert(shared::normalize_path(&outcome.path), (outcome.path.clone(), original_text));
	}
	if let Some(source_text) = outcome.import_fallback_source_text {
		let normalized = shared::normalize_path(&outcome.path);

		application
			.import_fallbacks
			.insert(normalized, ImportFallbackPlan { path: outcome.path.clone(), source_text });
	}

	if outcome.had_let_mut_reorder_edits {
		application.let_mut_reorder_files.insert(shared::normalize_path(&outcome.path));
	}
	if outcome.had_type_alias_rename_edits {
		application.type_alias_rename_files.insert(shared::normalize_path(&outcome.path));
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
	let_mut_reorder_files: &mut BTreeSet<PathBuf>,
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

		let normalized = shared::normalize_path(path);

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
			let_mut_reorder_files.insert(normalized);
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
		let (violations, _edits) = rules::collect_violations(&ctx, false);

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
			ctx.progress,
		)?;

		&computed_post_error_files
	};
	let new_errors =
		post_error_files.difference(ctx.baseline_error_files).cloned().collect::<Vec<_>>();
	let mut handled = BTreeSet::<PathBuf>::new();

	if !ctx.type_alias_rename_files.is_empty()
		&& new_errors.iter().any(|path| ctx.type_alias_rename_files.contains(path))
	{
		for normalized in ctx.type_alias_rename_files.iter() {
			if let Some((path, original_text)) = ctx.changed_originals.get(normalized) {
				fs::write(path, original_text)?;

				handled.insert(normalized.clone());

				eprintln!(
					"Skipped RUST-STYLE-TYPE-001 rename in {} due to failed semantic validation.",
					path.display()
				);
			}
		}
	}

	for normalized in post_error_files.difference(ctx.baseline_error_files) {
		if handled.contains(normalized) {
			continue;
		}

		if let Some((path, original_text)) = ctx.changed_originals.get(normalized)
			&& ctx.let_mut_reorder_files.contains(normalized)
		{
			fs::write(path, original_text)?;

			handled.insert(normalized.clone());

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

			handled.insert(normalized.clone());

			if ctx.verbose {
				eprintln!(
					"Applied import-shortening fallback in {} due to failed semantic validation.",
					import_fallback.path.display()
				);
			}
		}
	}
	for normalized in post_error_files.difference(ctx.baseline_error_files) {
		if handled.contains(normalized) {
			continue;
		}

		if let Some((path, original_text)) = ctx.changed_originals.get(normalized) {
			fs::write(path, original_text)?;

			if ctx.verbose {
				eprintln!("Reverted {} due to failed semantic validation.", path.display());
			}
			if ctx.let_mut_reorder_files.contains(normalized) {
				eprintln!(
					"Skipped RUST-STYLE-LET-001 reorder in {} due to failed semantic validation.",
					path.display()
				);
			}
		}
	}

	Ok(())
}

fn collect_fix_outcomes(
	files: &[PathBuf],
	type_alias_plan: &types::TypeAliasRenamePlan,
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
					type_alias_plan.apply(file, &original_text)?;
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
		&& let Some(packages) = workspace::package_names_for_files(changed_files)?
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
		let (_violations, mut edits) = rules::collect_filtered_violations(
			&ctx,
			true,
			with_import_shortening,
			with_binding_rewrites,
		);
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
				let violations = if workspace::is_swift_file(file) {
					swift::collect_violations_from_file(file)?
				} else if let Some(ctx) = shared::read_file_context(file)? {
					let (found, _edits) = rules::collect_violations(&ctx, false);

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

#[cfg(test)]
fn violation_signature(violation: &Violation) -> (usize, &'static str, &str, bool) {
	(violation.line, violation.rule, violation.message.as_str(), violation.fixable)
}

#[cfg(test)]
#[path = "tests/aliases.rs"]
mod aliases_tests;
#[cfg(test)]
#[path = "tests/derives.rs"]
mod derives_tests;
#[cfg(test)]
#[path = "tests/groups.rs"]
mod groups_tests;
#[cfg(test)]
#[path = "tests/interactions.rs"]
mod interactions_tests;
#[cfg(test)]
#[path = "tests/layout.rs"]
mod layout_tests;
#[cfg(test)]
#[path = "tests/qualification.rs"]
mod qualification_tests;
#[cfg(test)]
#[path = "tests/rules.rs"]
mod rules_tests;
#[cfg(test)]
#[path = "tests/scopes.rs"]
mod scopes_tests;
#[cfg(test)]
#[path = "tests/shortening.rs"]
mod shortening_tests;
#[cfg(test)]
#[path = "tests/spacing.rs"]
mod spacing_tests;
#[cfg(test)]
#[path = "tests/workflow.rs"]
mod workflow_tests;
