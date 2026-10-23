use std::{
	collections::BTreeSet,
	env, fs,
	path::PathBuf,
	process, slice,
	time::{SystemTime, UNIX_EPOCH},
};

use crate::style::{
	engine::{self, CheckState},
	shared::{self, CargoOptions, StyleLanguage},
	types::TypeAliasRenamePlan,
	workspace,
};

#[test]
fn collect_fix_outcomes_stores_lazy_import_fallback_source_text() {
	let nanos = SystemTime::now().duration_since(UNIX_EPOCH).expect("current timestamp").as_nanos();
	let path = env::temp_dir().join(format!("vstyle-lazy-fallback-{}-{}.rs", process::id(), nanos));
	let original = "use std::fmt::Result;\n\nfn parse() -> std::fmt::Result<()> {\n\tOk(())\n}\n";

	fs::write(&path, original).expect("seed temp file");

	let outcomes = super::collect_fix_outcomes(
		slice::from_ref(&path),
		&TypeAliasRenamePlan::default(),
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
fn should_stop_tune_round_stops_when_no_fixes_applied() {
	let (should_stop, non_decreasing_rounds) = engine::should_stop_tune_round(0, 10, 12, 1, true);

	assert!(should_stop);
	assert_eq!(non_decreasing_rounds, 1);
}

#[test]
fn should_stop_tune_round_stops_when_no_fixable_violations() {
	let (should_stop, non_decreasing_rounds) = engine::should_stop_tune_round(2, 0, 12, 1, true);

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
	let (should_stop, non_decreasing_rounds) = engine::should_stop_tune_round(2, 8, 12, 2, true);

	assert!(!should_stop);
	assert_eq!(non_decreasing_rounds, 0);
}

#[test]
fn should_stop_tune_round_continues_on_first_non_decreasing_round() {
	let (should_stop, non_decreasing_rounds) = engine::should_stop_tune_round(2, 12, 12, 0, true);

	assert!(!should_stop);
	assert_eq!(non_decreasing_rounds, 1);
}

#[test]
fn should_stop_tune_round_stops_on_second_consecutive_non_decreasing_round() {
	let (should_stop, non_decreasing_rounds) = engine::should_stop_tune_round(2, 12, 12, 1, true);

	assert!(should_stop);
	assert_eq!(non_decreasing_rounds, 2);
}

#[test]
fn should_stop_tune_round_stops_when_follow_up_round_is_not_needed() {
	let (should_stop, non_decreasing_rounds) = engine::should_stop_tune_round(2, 8, 12, 0, false);

	assert!(should_stop);
	assert_eq!(non_decreasing_rounds, 0);
}

#[test]
fn resolve_fix_round_scopes_workspace_splits_to_package_scopes() {
	let cargo_options = CargoOptions { workspace: true, ..CargoOptions::new(StyleLanguage::Rust) };
	let workspace_files =
		workspace::resolve_files(&cargo_options).expect("resolve workspace files");
	let scopes = super::resolve_fix_round_scopes(&cargo_options).expect("resolve fix scopes");
	let scoped_files =
		scopes.iter().flat_map(|(files, _options)| files.iter().cloned()).collect::<BTreeSet<_>>();
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
	let nanos = SystemTime::now().duration_since(UNIX_EPOCH).expect("current timestamp").as_nanos();
	let path = env::temp_dir().join(format!("vstyle-net-change-{}-{}.rs", process::id(), nanos));

	fs::write(&path, "fn sample() {}\n").expect("seed temp file");

	let snapshots = super::collect_file_snapshots(slice::from_ref(&path));

	assert!(!super::has_net_file_changes(&snapshots));

	fs::write(&path, "fn changed() {}\n").expect("mutate temp file");

	assert!(super::has_net_file_changes(&snapshots));

	let _ = fs::remove_file(&path);
}

#[test]
fn refresh_check_state_for_changed_files_updates_fixable_totals() {
	let nanos = SystemTime::now().duration_since(UNIX_EPOCH).expect("current timestamp").as_nanos();
	let violating_path =
		env::temp_dir().join(format!("vstyle-check-state-a-{}-{}.rs", process::id(), nanos));
	let clean_path =
		env::temp_dir().join(format!("vstyle-check-state-b-{}-{}.rs", process::id(), nanos));
	let files = vec![violating_path.clone(), clean_path.clone()];

	fs::write(&violating_path, "fn sample() {\n\tlet value = 10f32;\n}\n")
		.expect("seed violating file");
	fs::write(&clean_path, "fn sample() {\n\tlet value = 10_f32;\n}\n").expect("seed clean file");

	let counts_by_file = super::collect_file_violation_counts(&files).expect("initial counts");
	let violation_count =
		counts_by_file.values().map(|counts| counts.violation_count).sum::<usize>();
	let unfixable_count =
		counts_by_file.values().map(|counts| counts.unfixable_count).sum::<usize>();
	let mut check_state = CheckState { counts_by_file, violation_count, unfixable_count };

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
	let nanos = SystemTime::now().duration_since(UNIX_EPOCH).expect("current timestamp").as_nanos();
	let path = env::temp_dir().join(format!("vstyle-noop-round-{}-{}.rs", process::id(), nanos));

	fs::write(&path, "fn already_clean() {}\n").expect("seed temp file");

	let summary = super::run_fix_round(
		slice::from_ref(&path),
		&CargoOptions::new(StyleLanguage::Rust),
		false,
		false,
	)
	.expect("run no-op fix round");

	assert_eq!(summary.applied_count, 0);
	assert!(!summary.requires_follow_up_round);

	let _ = fs::remove_file(&path);
}
