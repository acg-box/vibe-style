mod aliases;
mod analysis;
mod derives;
mod groups;
mod local;
mod paths;
mod planning;
mod qualification;
mod references;
mod scopes;
mod shortening;

pub(crate) use scopes::{declared_file_is_test_only, exported_symbols_from_super_scope};

use std::collections::{HashMap, HashSet};

use ra_ap_syntax::ast::{NameRef, Path};

use crate::style::shared::{Edit, FileContext, TopItem, Violation};

type Import009Plan<'a> = (
	bool,
	Vec<(usize, usize, String)>,
	Vec<(usize, usize, String)>,
	Vec<(&'a TopItem, String, Option<String>)>,
);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Import008CandidateKind {
	TypePath,
	ValuePath,
	ValueReceiver,
	MacroModule,
	Derive,
}

#[derive(Clone)]
enum BracedImportSegment {
	Simple(String),
	Nested { head: String, children: Vec<String> },
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum PathQualificationRequirement {
	Qualified,
	Unqualified,
}

#[derive(Clone, Debug)]
struct Import008Candidate {
	line: usize,
	start: usize,
	end: usize,
	kind: Import008CandidateKind,
	symbol: String,
	import_path: String,
	replacement: String,
}

#[derive(Clone, Debug)]
struct Import004QualifiedFunctionCandidate {
	line: usize,
	start: usize,
	end: usize,
	full_path: String,
	parent_module_path: String,
	module_symbol: String,
	leaf_text: String,
}

#[derive(Default)]
struct ImportedSymbolMaps {
	symbol_paths: HashMap<String, HashSet<String>>,
	symbol_lines: HashMap<String, HashSet<usize>>,
	full_paths_by_symbol: HashMap<String, HashSet<String>>,
}

struct UseItemAnalysis<'a> {
	item: &'a TopItem,
	path: String,
	imported_symbols: Vec<String>,
	imported_symbol_set: HashSet<String>,
	imported_self_full_paths: HashSet<String>,
	imported_full_paths: Vec<String>,
}

struct ImportAnalysis<'a> {
	use_runs: Vec<Vec<&'a TopItem>>,
	use_item_analyses: Vec<UseItemAnalysis<'a>>,
	use_item_analysis_by_key: HashMap<(usize, usize), usize>,
	local_module_roots: HashSet<String>,
	local_defined_symbols: HashSet<String>,
	imported_symbol_maps: ImportedSymbolMaps,
	qualified_type_paths_by_symbol: HashMap<String, HashSet<String>>,
	qualified_value_paths_by_symbol: HashMap<String, HashSet<String>>,
}
impl<'a> ImportAnalysis<'a> {
	fn use_item_analysis(&self, item: &TopItem) -> Option<&UseItemAnalysis<'a>> {
		let idx = *self.use_item_analysis_by_key.get(&paths::use_item_lock_key(item))?;

		self.use_item_analyses.get(idx)
	}
}

#[derive(Clone, Debug)]
struct Import008UseRecoveryCandidate {
	line: usize,
	start_line: usize,
	end_line: usize,
	symbol: String,
	import_path: String,
}

#[derive(Default)]
struct MixedUseGroup {
	indices: Vec<usize>,
	has_self: bool,
	children: Vec<String>,
}

struct UseEntry<'a> {
	item: &'a TopItem,
	origin: usize,
	order: usize,
	block: String,
}

#[derive(Default)]
struct TraitKeepAliveNormalizationState {
	affected_symbols: HashSet<String>,
	changed: bool,
	seen_trait_keys: HashSet<String>,
}

struct Import006TargetScope {
	ctx: FileContext,
	offset: usize,
	key: usize,
}

struct PlannedImportPath {
	path: String,
	conditions: Vec<String>,
}

struct Import004ModuleAccessPlan {
	access_path: String,
	keep_parent_module_import: bool,
}

struct ValuePathCandidate {
	path: Path,
	name_ref: NameRef,
	is_record_context: bool,
}

struct TypePathCandidate {
	path: Path,
	name_ref: NameRef,
	suffix: String,
}

pub(crate) fn check_import_rules(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	with_import_shortening: bool,
) {
	let use_runs = groups::collect_non_pub_use_runs(ctx);

	groups::apply_pub_use_group_rules(ctx, violations, edits, emit_edits);

	let use_items = use_runs.iter().flat_map(|run| run.iter().copied()).collect::<Vec<_>>();
	let import010_fixed_lines =
		scopes::apply_import010_no_super_use_rule(ctx, violations, edits, emit_edits, &use_items);
	let import007_fixed_lines =
		scopes::apply_import007_no_glob_use_rule(ctx, violations, edits, emit_edits, &use_items);

	if ctx.path.file_name().is_some_and(|name| name == "error.rs") {
		return;
	}

	let analysis = analysis::build_import_analysis(ctx, use_runs, use_items);
	let mut use_item_skip_lines = HashSet::new();

	use_item_skip_lines.extend(import010_fixed_lines.iter().copied());
	use_item_skip_lines.extend(import007_fixed_lines.iter().copied());

	let import004_fixed_lines = planning::apply_use_item_rules(
		ctx,
		violations,
		edits,
		emit_edits,
		&analysis.use_item_analyses,
		&use_item_skip_lines,
		with_import_shortening,
	);

	qualification::push_import004_ambiguous_symbol_violations(
		ctx,
		violations,
		&analysis.imported_symbol_maps,
	);
	aliases::apply_import012_crate_keep_alive_rule(
		ctx,
		violations,
		&analysis.use_item_analyses,
		&analysis.local_module_roots,
	);

	let import004_qualified_path_fixed_lines =
		qualification::apply_import004_qualified_function_path_rule(
			ctx,
			violations,
			edits,
			emit_edits,
			&analysis.use_runs,
			&analysis.local_module_roots,
			&analysis.imported_symbol_maps.full_paths_by_symbol,
			true,
		);
	let import009_fixed_lines = qualification::apply_import009_rules(
		ctx,
		violations,
		edits,
		emit_edits,
		&analysis,
		&use_item_skip_lines,
	);
	let import008_group_skip_lines =
		shortening::apply_import008_rules(ctx, violations, edits, emit_edits, &analysis);
	let mut import_group_skip_lines = HashSet::new();

	import_group_skip_lines.extend(import010_fixed_lines);
	import_group_skip_lines.extend(import007_fixed_lines);
	import_group_skip_lines.extend(import004_fixed_lines);
	import_group_skip_lines.extend(import004_qualified_path_fixed_lines);
	import_group_skip_lines.extend(import009_fixed_lines);
	import_group_skip_lines.extend(import008_group_skip_lines);

	derives::apply_import011_derive_order_rule(
		ctx,
		violations,
		edits,
		emit_edits,
		&analysis.local_module_roots,
		&analysis.local_defined_symbols,
		&analysis.imported_symbol_maps.full_paths_by_symbol,
	);

	let (planned_import_group_edits, fixable_import_group_lines) =
		groups::build_import_group_fix_plans(ctx, &analysis, &import_group_skip_lines);

	if emit_edits {
		edits.extend(planned_import_group_edits);
	}

	groups::push_import_group_order_spacing_violations(
		ctx,
		violations,
		&analysis.use_runs,
		&fixable_import_group_lines,
		&analysis.local_module_roots,
	);
	local::apply_import006_local_use_scope_rule(ctx, violations, edits, emit_edits);
	local::check_cfg_test_module_import_rules(ctx, violations, edits, emit_edits);

	// Hoisted groups can contain bindings that shortening would insert separately.
	// Re-plan shortening against the completed hoist on the next pass.
	if edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-006") {
		edits.retain(|edit| edit.rule != "RUST-STYLE-IMPORT-008");
	}
}
