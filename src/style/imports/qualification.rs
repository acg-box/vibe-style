use std::{
	borrow::Cow,
	collections::{BTreeSet, HashMap, HashSet},
};

use ra_ap_syntax::{
	AstNode as _, SyntaxNode,
	ast::{CallExpr, HasName as _, Item, MacroCall, Module, Path, PathExpr, Use},
};
use regex::Regex;

use crate::style::{
	imports::{
		Import004ModuleAccessPlan, Import004QualifiedFunctionCandidate, Import009Plan,
		ImportAnalysis, ImportedSymbolMaps, PlannedImportPath, UseItemAnalysis, groups, paths,
		references, scopes,
	},
	shared::{self, Edit, FileContext, TopItem, TopKind, Violation},
	syntax,
};

pub(super) fn apply_import009_std_fmt_result_rule(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	item: &TopItem,
	path: &str,
) -> bool {
	if !paths::imported_full_paths_from_use_path(path).iter().any(|full| full == "std::fmt::Result")
	{
		return false;
	}

	let nongeneric_rewrites =
		references::unqualified_nongeneric_type_path_rewrites(ctx, "Result", "std::fmt::Result");
	let has_generic_result_uses = references::has_unqualified_generic_type_path_use(ctx, "Result");
	let Some((_qualified_symbol_path, rewritten_use_path)) =
		paths::import004_fix_plan(path, "Result")
	else {
		return false;
	};

	if nongeneric_rewrites.is_empty() && !has_generic_result_uses {
		return false;
	}

	shared::push_violation(
		violations,
		ctx,
		item.line,
		"RUST-STYLE-IMPORT-009",
		"Do not import `std::fmt::Result`; use `std::fmt::Result` at call sites and keep generic `Result<T, E>` unshadowed.",
		true,
	);

	if !emit_edits {
		return true;
	}

	for (start, end, replacement) in nongeneric_rewrites {
		edits.push(Edit { start, end, replacement, rule: "RUST-STYLE-IMPORT-009" });
	}

	let Some(edit) = paths::build_use_item_rewrite_edit(
		ctx,
		item,
		rewritten_use_path.as_deref(),
		"RUST-STYLE-IMPORT-009",
	) else {
		return false;
	};

	edits.push(edit);

	true
}

pub(super) fn apply_import009_non_importable_root_use_rule(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	item: &TopItem,
	path: &str,
) -> bool {
	let compact_path = path.replace(' ', "");
	let Some(root) = compact_path.split("::").next() else {
		return false;
	};

	if !paths::is_non_importable_use_root(root) {
		return false;
	}

	let mut rewrites = Vec::new();

	for full_path in paths::imported_full_paths_from_use_path(path) {
		let Some(symbol) = paths::symbol_from_full_import_path(&full_path) else {
			continue;
		};

		rewrites.extend(references::unqualified_type_path_rewrites(ctx, &symbol, &full_path));
		rewrites.extend(references::unqualified_value_path_rewrites(ctx, &symbol, &full_path));
	}

	shared::push_violation(
		violations,
		ctx,
		item.line,
		"RUST-STYLE-IMPORT-009",
		"Do not import symbols from non-importable roots (`Self` or generic parameters); use qualified paths.",
		true,
	);

	if !emit_edits {
		return true;
	}

	for (start, end, replacement) in rewrites {
		edits.push(Edit { start, end, replacement, rule: "RUST-STYLE-IMPORT-009" });
	}

	let Some(edit) = paths::build_use_item_rewrite_edit(ctx, item, None, "RUST-STYLE-IMPORT-009")
	else {
		return false;
	};

	edits.push(edit);

	true
}

pub(super) fn apply_import004_free_fn_macro_rule(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	item: &TopItem,
	path: &str,
	planned_import_paths: &mut Vec<PlannedImportPath>,
) -> bool {
	if !path.contains("::") {
		return false;
	}

	let mut fixed = false;

	for symbol in paths::imported_symbols_from_use_path(path) {
		if symbol.is_empty() || !symbol.chars().next().is_some_and(char::is_lowercase) {
			continue;
		}

		let local_fn_defined = is_local_fn_defined(ctx, &symbol);
		let local_macro_defined = is_local_macro_defined(ctx, &symbol);
		let mut fn_ranges = references::unqualified_function_call_ranges(ctx, &symbol);
		let mut macro_ranges = references::unqualified_macro_call_ranges(ctx, &symbol);

		retain_parent_call_ranges(ctx, &mut fn_ranges, &mut macro_ranges);

		let needs_fn_fix = !fn_ranges.is_empty() && !local_fn_defined;
		let needs_macro_fix = !macro_ranges.is_empty()
			&& !local_macro_defined
			&& !symbol_imported_from_std_like_root(ctx, path, &symbol);

		if !(needs_fn_fix || needs_macro_fix) {
			continue;
		}

		let preserve_binding = scopes::symbol_referenced_by_child_module(ctx, &symbol);
		let mut fixable = false;
		let mut qualified_symbol_path = String::new();
		let mut use_item_edit = None;

		if (!fn_ranges.is_empty() || !macro_ranges.is_empty())
			&& let Some((qualified_path, rewritten_use_path)) = import004_free_fn_fix_plan(
				ctx,
				item,
				path,
				&symbol,
				planned_import_paths,
				preserve_binding,
			) {
			qualified_symbol_path = qualified_path;
			fixable = true;

			if emit_edits && rewritten_use_path.as_deref() != Some(path) {
				use_item_edit = paths::build_use_item_rewrite_edit(
					ctx,
					item,
					rewritten_use_path.as_deref(),
					"RUST-STYLE-IMPORT-004",
				);

				if use_item_edit.is_none() {
					fixable = false;
				} else {
					planned_import_paths.extend(rewritten_use_path.map(|path| PlannedImportPath {
						path,
						conditions: import_conditions(item),
					}));
				}
			}
		}

		shared::push_violation(
			violations,
			ctx,
			item.line,
			"RUST-STYLE-IMPORT-004",
			"Do not import free functions or macros into scope; prefer qualified module paths.",
			fixable,
		);

		if !emit_edits || !fixable {
			continue;
		}

		for (start, end) in fn_ranges.iter().copied() {
			edits.push(Edit {
				start,
				end,
				replacement: qualified_symbol_path.clone(),
				rule: "RUST-STYLE-IMPORT-004",
			});
		}
		for (start, end) in macro_ranges.iter().copied() {
			edits.push(Edit {
				start,
				end,
				replacement: qualified_symbol_path.clone(),
				rule: "RUST-STYLE-IMPORT-004",
			});
		}

		if let Some(edit) = use_item_edit {
			edits.push(edit);
		}

		fixed = true;

		break;
	}

	fixed
}

#[allow(clippy::too_many_arguments)]
pub(super) fn apply_import004_qualified_function_path_rule(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	use_runs: &[Vec<&TopItem>],
	local_module_roots: &HashSet<String>,
	imported_full_paths_by_symbol: &HashMap<String, HashSet<String>>,
	skip_cfg_test_module_paths: bool,
) -> HashSet<usize> {
	// Direct-import rewrites can introduce module bindings. Reconsider path
	// shortening on the next pass, after those bindings are part of the context.
	let emit_edits = emit_edits && !edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-004");
	let candidates =
		collect_import004_qualified_function_candidates(ctx, skip_cfg_test_module_paths);

	if candidates.is_empty() {
		return HashSet::new();
	}

	let mut touched_lines = HashSet::new();
	let mut pending_import_paths = BTreeSet::new();
	let mut planned_access_paths = HashMap::<String, String>::new();

	for candidate in candidates {
		let Some(module_access_plan) = import004_preferred_module_access_plan(
			ctx,
			None,
			None,
			&candidate.parent_module_path,
			&candidate.module_symbol,
		) else {
			continue;
		};
		let shortened_path = format!("{}::{}", module_access_plan.access_path, candidate.leaf_text);

		if paths::compact_path_for_match(&shortened_path)
			== paths::compact_path_for_match(&candidate.full_path)
		{
			continue;
		}

		if let Some(existing_parent) = planned_access_paths.get(&module_access_plan.access_path)
			&& paths::compact_path_for_match(existing_parent)
				!= paths::compact_path_for_match(&candidate.parent_module_path)
		{
			continue;
		}

		shared::push_violation(
			violations,
			ctx,
			candidate.line,
			"RUST-STYLE-IMPORT-004",
			"Prefer parent-module qualified free-function paths when unambiguous.",
			true,
		);

		if !emit_edits {
			continue;
		}

		edits.push(Edit {
			start: candidate.start,
			end: candidate.end,
			replacement: shortened_path,
			rule: "RUST-STYLE-IMPORT-004",
		});
		touched_lines.insert(candidate.line);

		if module_access_plan.keep_parent_module_import {
			pending_import_paths.insert(candidate.parent_module_path.clone());
			planned_access_paths
				.entry(module_access_plan.access_path)
				.or_insert(candidate.parent_module_path);
		}
	}

	if !emit_edits {
		return touched_lines;
	}

	touched_lines.extend(groups::apply_pending_module_import_path_edits(
		ctx,
		edits,
		use_runs,
		local_module_roots,
		&pending_import_paths,
		imported_full_paths_by_symbol,
		"RUST-STYLE-IMPORT-004",
	));

	touched_lines
}

pub(super) fn use_path_needs_import004_fix(ctx: &FileContext, path: &str) -> bool {
	for symbol in paths::imported_symbols_from_use_path(path) {
		if symbol.is_empty() || !symbol.chars().next().is_some_and(char::is_lowercase) {
			continue;
		}

		let local_fn_defined = is_local_fn_defined(ctx, &symbol);
		let local_macro_defined = is_local_macro_defined(ctx, &symbol);
		let needs_fn_fix = !references::unqualified_function_call_ranges(ctx, &symbol).is_empty()
			&& !local_fn_defined;
		let needs_macro_fix = !references::unqualified_macro_call_ranges(ctx, &symbol).is_empty()
			&& !local_macro_defined
			&& !symbol_imported_from_std_like_root(ctx, path, &symbol);

		if needs_fn_fix || needs_macro_fix {
			return true;
		}
	}

	false
}

pub(super) fn push_import004_ambiguous_symbol_violations(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	maps: &ImportedSymbolMaps,
) {
	for (symbol, paths) in &maps.symbol_paths {
		if paths.len() <= 1 {
			continue;
		}

		for line in maps.symbol_lines.get(symbol).into_iter().flat_map(|lines| lines.iter()) {
			shared::push_violation(
				violations,
				ctx,
				*line,
				"RUST-STYLE-IMPORT-004",
				&format!(
					"Ambiguous imported symbol `{symbol}` is not allowed; use fully qualified paths."
				),
				false,
			);
		}
	}
}

pub(super) fn apply_import009_rules(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	analysis: &ImportAnalysis<'_>,
	skip_lines: &HashSet<usize>,
) -> HashSet<usize> {
	let mut import009_fixed_lines = HashSet::new();
	let mut locked_use_ranges = HashSet::new();

	for (symbol, imported_paths) in &analysis.imported_symbol_maps.full_paths_by_symbol {
		if analysis
			.imported_symbol_maps
			.symbol_lines
			.get(symbol)
			.is_some_and(|lines| lines.iter().any(|line| skip_lines.contains(line)))
		{
			continue;
		}
		if imported_paths.len() != 1 || analysis.local_defined_symbols.contains(symbol) {
			continue;
		}

		let Some(imported_path) = imported_paths.iter().next().cloned() else {
			continue;
		};
		let Some((fixable, type_rewrites, value_rewrites, use_item_plans)) = build_import009_plan(
			ctx,
			&analysis.use_item_analyses,
			symbol,
			&imported_path,
			&analysis.qualified_type_paths_by_symbol,
			&analysis.qualified_value_paths_by_symbol,
		) else {
			continue;
		};

		for line in analysis
			.imported_symbol_maps
			.symbol_lines
			.get(symbol)
			.into_iter()
			.flat_map(|lines| lines.iter())
		{
			shared::push_violation(
				violations,
				ctx,
				*line,
				"RUST-STYLE-IMPORT-009",
				&format!(
					"Ambiguous symbol `{symbol}` should use fully qualified paths consistently."
				),
				fixable,
			);
		}

		if !emit_edits || !fixable {
			continue;
		}
		if use_item_plans
			.iter()
			.any(|(item, _, _)| locked_use_ranges.contains(&paths::use_item_lock_key(item)))
		{
			continue;
		}

		let mut planned_use_item_edits = Vec::new();

		for (item, _qualified_symbol_path, rewritten_use_path) in &use_item_plans {
			let Some(edit) = paths::build_use_item_rewrite_edit(
				ctx,
				item,
				rewritten_use_path.as_deref(),
				"RUST-STYLE-IMPORT-009",
			) else {
				planned_use_item_edits.clear();

				break;
			};

			planned_use_item_edits.push((item.line, paths::use_item_lock_key(item), edit));
		}

		if planned_use_item_edits.is_empty() {
			continue;
		}

		for (start, end, replacement) in type_rewrites {
			edits.push(Edit { start, end, replacement, rule: "RUST-STYLE-IMPORT-009" });
		}
		for (start, end, replacement) in value_rewrites {
			edits.push(Edit { start, end, replacement, rule: "RUST-STYLE-IMPORT-009" });
		}
		for (line, lock_key, edit) in planned_use_item_edits {
			edits.push(edit);
			import009_fixed_lines.insert(line);
			locked_use_ranges.insert(lock_key);
		}
	}

	import009_fixed_lines
}

fn symbol_imported_from_std_like_root(ctx: &FileContext, path: &str, symbol: &str) -> bool {
	paths::imported_full_paths_from_use_path(path).into_iter().any(|full_path| {
		paths::symbol_from_full_import_path(&full_path).as_deref() == Some(symbol)
			&& import_path_has_std_root(ctx, &full_path, &HashSet::new())
	})
}

fn import_path_has_std_root(ctx: &FileContext, path: &str, visited: &HashSet<String>) -> bool {
	let path = path.strip_prefix("::").unwrap_or(path);

	if matches!(path.split("::").next(), Some("std" | "core" | "alloc")) {
		return true;
	}

	let Some(tail) = path.strip_prefix("crate::") else {
		return false;
	};
	let mut visited = visited.clone();

	if !visited.insert(path.to_owned()) {
		return false;
	}

	let mut segments = tail.split("::").map(str::to_owned).collect::<Vec<_>>();
	let Some(symbol) = segments.pop() else {
		return false;
	};
	let Some((_, scope)) = scopes::resolve_crate_module_scope(ctx, &segments) else {
		return false;
	};
	let items = scope.children().filter_map(Item::cast).collect::<Vec<_>>();

	if items.iter().any(|item| matches!(item, Item::MacroRules(item) if item.name().is_some_and(|name| name.text() == symbol))) {
		return false;
	}

	let prefix = scopes::crate_absolute_use_path(&segments, "");
	let mut targets = HashSet::new();

	for item in &items {
		let Item::Use(import) = item else {
			continue;
		};
		let Some(tree) = import.use_tree() else {
			continue;
		};
		let use_path = tree.syntax().text().to_string();

		if !paths::imported_symbols_from_use_path(&use_path).contains(&symbol) {
			continue;
		}

		let aliases = paths::collect_non_keep_alive_alias_bindings(&use_path);

		for original in paths::imported_full_paths_from_use_path(&use_path) {
			let binding = aliases
				.iter()
				.find(|(_, path)| paths::compact_path_for_match(path) == original)
				.map(|(alias, _)| alias.clone())
				.or_else(|| paths::symbol_from_full_import_path(&original));

			if binding.as_deref() == Some(&symbol) {
				let Some(target) = scopes::scope_import_target(&original, &prefix, &items) else {
					return false;
				};

				targets.insert(target);
			}
		}
	}

	!targets.is_empty()
		&& targets.iter().all(|target| import_path_has_std_root(ctx, target, &visited))
}

fn is_local_fn_defined(ctx: &FileContext, symbol: &str) -> bool {
	let local_fn_def_re = Regex::new(&format!(
		r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:const\s+)?(?:unsafe\s+)?fn\s+{}\b",
		regex::escape(symbol)
	))
	.expect("Expected operation to succeed.");

	ctx.lines.iter().any(|line| {
		let code = shared::strip_string_and_line_comment(line, false).0;

		local_fn_def_re.is_match(&code)
	})
}

fn is_local_macro_defined(ctx: &FileContext, symbol: &str) -> bool {
	let local_macro_def_re = Regex::new(&format!(
		r"^\s*(?:macro_rules!\s*{}\b|macro\s+{}\b)",
		regex::escape(symbol),
		regex::escape(symbol),
	))
	.expect("Expected operation to succeed.");

	ctx.lines.iter().any(|line| {
		let code = shared::strip_string_and_line_comment(line, false).0;

		local_macro_def_re.is_match(&code)
	})
}

fn build_import009_plan<'a>(
	ctx: &FileContext,
	use_item_analyses: &'a [UseItemAnalysis<'a>],
	symbol: &str,
	imported_path: &str,
	qualified_type_paths_by_symbol: &HashMap<String, HashSet<String>>,
	qualified_value_paths_by_symbol: &HashMap<String, HashSet<String>>,
) -> Option<Import009Plan<'a>> {
	let equivalent_qualified_paths = import009_equivalent_qualified_paths(ctx, imported_path);
	let mut type_rewrites = references::unqualified_type_path_rewrites(ctx, symbol, imported_path);

	type_rewrites.extend(references::unqualified_derive_attr_symbol_rewrites(
		ctx,
		symbol,
		imported_path,
	));

	let value_rewrites = references::unqualified_value_path_rewrites(ctx, symbol, imported_path);
	let has_unqualified_type_uses = !type_rewrites.is_empty();
	let has_unqualified_value_uses = !value_rewrites.is_empty();
	let has_unqualified_uses = has_unqualified_type_uses || has_unqualified_value_uses;
	let has_any_qualified_type_path =
		qualified_type_paths_by_symbol.get(symbol).is_some_and(|paths| !paths.is_empty());
	let has_conflicting_qualified_value_path =
		qualified_value_paths_by_symbol.get(symbol).is_some_and(|paths| {
			paths.iter().any(|path| !equivalent_qualified_paths.contains(path.as_str()))
		});
	let has_qualified_value_same_path =
		qualified_value_paths_by_symbol.get(symbol).is_some_and(|paths| {
			paths.iter().any(|path| equivalent_qualified_paths.contains(path.as_str()))
		});
	let has_qualified_type_same_path =
		qualified_type_paths_by_symbol.get(symbol).is_some_and(|paths| {
			paths.iter().any(|path| equivalent_qualified_paths.contains(path.as_str()))
		});
	let value_uses_record_only = has_unqualified_value_uses
		&& references::unqualified_value_paths_are_record_only(ctx, symbol);
	let rewrite_value_only_conflicts = has_unqualified_value_uses
		&& !has_unqualified_type_uses
		&& !value_uses_record_only
		&& references::is_import009_type_like_symbol(symbol)
		&& has_conflicting_qualified_value_path;

	if !has_unqualified_type_uses
		&& !has_any_qualified_type_path
		&& !value_uses_record_only
		&& !rewrite_value_only_conflicts
		&& !has_qualified_value_same_path
	{
		// Keep IMPORT-009 focused on type ambiguity (plus struct-literal style value paths
		// and type-like associated value paths).
		// Pure module/value symbol overlap like `feeds` vs `store::feed::feeds` remains ignored.
		return None;
	}

	let mut qualified_paths = HashSet::new();

	if let Some(paths) = qualified_type_paths_by_symbol.get(symbol) {
		qualified_paths.extend(paths.iter().cloned());
	}
	if let Some(paths) = qualified_value_paths_by_symbol.get(symbol) {
		qualified_paths.extend(paths.iter().cloned());
	}

	let has_any_qualified_path = !qualified_paths.is_empty();
	let has_qualified_same_path =
		qualified_paths.iter().any(|path| equivalent_qualified_paths.contains(path.as_str()));
	let has_conflicting_qualified_path =
		qualified_paths.iter().any(|path| !equivalent_qualified_paths.contains(path.as_str()));
	let rewrite_value_only_records =
		has_unqualified_value_uses && !has_unqualified_type_uses && value_uses_record_only;
	let imported_root = imported_path.split("::").next().unwrap_or_default();
	let guard_shortening_cycle = !matches!(imported_root, "crate" | "self" | "super");
	let import008_cycle_risk = guard_shortening_cycle
		&& !has_conflicting_qualified_path
		&& has_unqualified_value_uses
		&& has_qualified_type_same_path;

	if has_unqualified_uses {
		if !has_any_qualified_path {
			return None;
		}
	} else if !has_qualified_same_path {
		// Avoid "unused import" style deletion in this rule, because imports can have effects
		// (for example, trait method resolution) even when the symbol name is not referenced.
		return None;
	}
	if (!has_unqualified_uses && has_conflicting_qualified_path && !rewrite_value_only_records)
		|| (has_unqualified_uses
			&& !has_qualified_same_path
			&& !has_conflicting_qualified_path
			&& !rewrite_value_only_records)
	{
		return Some((false, Vec::new(), Vec::new(), Vec::new()));
	}
	if import008_cycle_risk || references::symbol_has_bare_identifier_pattern(ctx, symbol) {
		return Some((false, Vec::new(), Vec::new(), Vec::new()));
	}

	type_rewrites.extend(references::dependent_use_root_rewrites(ctx, symbol, imported_path));

	let mut use_item_plans = Vec::new();
	let mut fixable = true;

	for use_item_analysis in use_item_analyses {
		if !use_item_imports_symbol_path(&use_item_analysis.path, symbol, imported_path) {
			continue;
		}

		let removal_fix_plan = import004_removal_fix_plan(
			ctx,
			use_item_analysis.item,
			&use_item_analysis.path,
			symbol,
		);
		let Some((qualified_symbol_path, rewritten_use_path)) = removal_fix_plan else {
			fixable = false;

			break;
		};

		use_item_plans.push((use_item_analysis.item, qualified_symbol_path, rewritten_use_path));
	}

	Some((fixable && !use_item_plans.is_empty(), type_rewrites, value_rewrites, use_item_plans))
}

fn import009_equivalent_qualified_paths<'a>(
	ctx: &FileContext,
	imported_path: &'a str,
) -> HashSet<Cow<'a, str>> {
	let mut paths = HashSet::from([Cow::Borrowed(imported_path)]);
	let Some((parent_module_path, symbol)) = imported_path.rsplit_once("::") else {
		return paths;
	};
	let Some(module_symbol) = parent_module_path.rsplit("::").next() else {
		return paths;
	};
	let module_symbol = syntax::normalize_ident(module_symbol);

	if module_symbol.is_empty() || matches!(module_symbol, "crate" | "self" | "super") {
		return paths;
	}

	if let Some(module_access_plan) =
		import004_preferred_module_access_plan(ctx, None, None, parent_module_path, module_symbol)
	{
		paths.insert(Cow::Owned(format!("{}::{}", module_access_plan.access_path, symbol)));
	}

	paths
}

fn collect_import004_qualified_function_candidates(
	ctx: &FileContext,
	skip_cfg_test_module_paths: bool,
) -> Vec<Import004QualifiedFunctionCandidate> {
	let mut candidates = Vec::new();
	let mut seen_ranges = HashSet::new();

	collect_import004_qualified_function_call_candidates(
		ctx,
		skip_cfg_test_module_paths,
		&mut candidates,
		&mut seen_ranges,
	);
	collect_import004_qualified_function_macro_candidates(
		ctx,
		skip_cfg_test_module_paths,
		&mut candidates,
		&mut seen_ranges,
	);

	candidates
}

fn collect_import004_qualified_function_call_candidates(
	ctx: &FileContext,
	skip_cfg_test_module_paths: bool,
	candidates: &mut Vec<Import004QualifiedFunctionCandidate>,
	seen_ranges: &mut HashSet<(usize, usize)>,
) {
	for call_expr in ctx.source_file.syntax().descendants().filter_map(CallExpr::cast) {
		let Some(expr) = call_expr.expr() else {
			continue;
		};
		let Some(path_expr) = PathExpr::cast(expr.syntax().clone()) else {
			continue;
		};
		let Some(path) = path_expr.path() else {
			continue;
		};

		if path.qualifier().is_none()
			|| paths::path_is_qualifier_subpath(&path)
			|| paths::syntax_is_inside_cfg_guarded_scope(path.syntax())
			|| (skip_cfg_test_module_paths && paths::is_inside_cfg_test_module(&path))
		{
			continue;
		}

		let mut segments = Vec::new();

		if !paths::collect_path_segment_texts(&path, &mut segments) {
			continue;
		}

		let Some(leaf_text) = path.segment().map(|segment| segment.syntax().text().to_string())
		else {
			continue;
		};
		let range = path.syntax().text_range();

		import004_push_qualified_function_candidate(
			ctx,
			candidates,
			seen_ranges,
			&segments,
			usize::from(range.start()),
			usize::from(range.end()),
			leaf_text,
		);
	}
}

fn collect_import004_qualified_function_macro_candidates(
	ctx: &FileContext,
	skip_cfg_test_module_paths: bool,
	candidates: &mut Vec<Import004QualifiedFunctionCandidate>,
	seen_ranges: &mut HashSet<(usize, usize)>,
) {
	for macro_call in ctx.source_file.syntax().descendants().filter_map(MacroCall::cast) {
		if paths::syntax_is_inside_cfg_guarded_scope(macro_call.syntax())
			|| (skip_cfg_test_module_paths
				&& paths::syntax_is_inside_cfg_test_module(macro_call.syntax()))
		{
			continue;
		}

		let Some(token_tree) = macro_call.token_tree() else {
			continue;
		};
		let tree_text = token_tree.syntax().text().to_string();
		let tree_start = usize::from(token_tree.syntax().text_range().start());

		for found in paths::qualified_macro_path_regex().find_iter(&tree_text) {
			if !references::macro_symbol_has_import004_function_follow(&tree_text, found.end())
				|| !references::macro_symbol_is_import009_unqualified(
					ctx,
					&tree_text,
					tree_start,
					found.start(),
				) {
				continue;
			}

			let segments = found
				.as_str()
				.split("::")
				.map(str::trim)
				.filter(|segment| !segment.is_empty())
				.map(ToOwned::to_owned)
				.collect::<Vec<_>>();
			let Some(leaf_text) = segments.last().cloned() else {
				continue;
			};

			import004_push_qualified_function_candidate(
				ctx,
				candidates,
				seen_ranges,
				&segments,
				tree_start + found.start(),
				tree_start + found.end(),
				leaf_text,
			);
		}
	}
}

fn import004_push_qualified_function_candidate(
	ctx: &FileContext,
	candidates: &mut Vec<Import004QualifiedFunctionCandidate>,
	seen_ranges: &mut HashSet<(usize, usize)>,
	segments: &[String],
	start: usize,
	end: usize,
	leaf_text: String,
) {
	if segments.len() < 3 || start >= end || !seen_ranges.insert((start, end)) {
		return;
	}

	let symbol = syntax::normalize_ident(&leaf_text).to_owned();

	if symbol.is_empty() || !symbol.chars().next().is_some_and(char::is_lowercase) {
		return;
	}

	let parent_module_path = segments[..segments.len() - 1].join("::");
	let Some(module_symbol) = parent_module_path.rsplit("::").next() else {
		return;
	};
	let module_symbol = syntax::normalize_ident(module_symbol).to_owned();

	if module_symbol.is_empty() || !module_symbol.chars().next().is_some_and(char::is_lowercase) {
		return;
	}

	candidates.push(Import004QualifiedFunctionCandidate {
		line: shared::line_from_offset(&ctx.line_starts, start),
		start,
		end,
		full_path: segments.join("::"),
		parent_module_path,
		module_symbol,
		leaf_text,
	});
}

fn use_item_imports_symbol_path(path: &str, symbol: &str, import_path: &str) -> bool {
	if !paths::imported_symbols_from_use_path(path).iter().any(|item_symbol| item_symbol == symbol)
	{
		return false;
	}

	paths::imported_full_paths_from_use_path(path).into_iter().any(|path| path == import_path)
}

fn retain_parent_call_ranges(
	ctx: &FileContext,
	fn_ranges: &mut Vec<(usize, usize)>,
	macro_ranges: &mut Vec<(usize, usize)>,
) {
	let children = ctx
		.source_file
		.syntax()
		.descendants()
		.filter_map(Module::cast)
		.filter_map(|module| module.item_list())
		.map(|items| items.syntax().text_range())
		.collect::<Vec<_>>();
	let local = |range: &(usize, usize)| {
		!children.iter().any(|child| {
			usize::from(child.start()) <= range.0 && range.1 <= usize::from(child.end())
		})
	};

	fn_ranges.retain(local);
	macro_ranges.retain(local);
}

fn import004_removal_fix_plan(
	ctx: &FileContext,
	item: &TopItem,
	path: &str,
	symbol: &str,
) -> Option<(String, Option<String>)> {
	if scopes::symbol_referenced_by_child_module(ctx, symbol) {
		return None;
	}

	import004_free_fn_fix_plan(ctx, item, path, symbol, &[], false)
}

fn import004_free_fn_fix_plan(
	ctx: &FileContext,
	current_item: &TopItem,
	path: &str,
	symbol: &str,
	planned_import_paths: &[PlannedImportPath],
	preserve_binding: bool,
) -> Option<(String, Option<String>)> {
	let (default_qualified_symbol_path, rewritten_use_path_without_symbol) =
		paths::import004_fix_plan(path, symbol)?;

	if preserve_binding {
		return Some((default_qualified_symbol_path, Some(path.to_owned())));
	}

	let Some((parent_module_path, module_symbol)) = import004_parent_module_target(path, symbol)
	else {
		return Some((default_qualified_symbol_path, rewritten_use_path_without_symbol));
	};
	let full_symbol_path = format!("{parent_module_path}::{symbol}");

	if !references::unqualified_macro_call_ranges(ctx, symbol).is_empty()
		&& import004_has_conflicting_root_qualified_path_usage(ctx, symbol, &full_symbol_path)
	{
		// A single import can bind a macro and a module in separate namespaces.
		return Some((full_symbol_path, Some(path.to_owned())));
	}

	let Some(mut module_access_plan) = import004_preferred_module_access_plan(
		ctx,
		Some(current_item),
		Some(path),
		&parent_module_path,
		&module_symbol,
	) else {
		return Some((default_qualified_symbol_path, rewritten_use_path_without_symbol));
	};

	for planned in planned_import_paths {
		if import004_use_path_conflicts_with_parent_module(
			&planned.path,
			&parent_module_path,
			&module_symbol,
		) {
			return None;
		}
		if import004_use_path_imports_parent_module(&planned.path, &parent_module_path) {
			if !planned.conditions.is_empty()
				&& planned.conditions != import_conditions(current_item)
			{
				return Some((default_qualified_symbol_path, rewritten_use_path_without_symbol));
			}

			module_access_plan.keep_parent_module_import = false;
		}
	}

	let (_, rewritten_use_path_with_parent_module) = import004_fix_plan_with_parent_module(
		ctx,
		path,
		symbol,
		&parent_module_path,
		&module_access_plan.access_path,
	)?;
	let rewritten_use_path = if module_access_plan.keep_parent_module_import {
		rewritten_use_path_with_parent_module
	} else {
		rewritten_use_path_without_symbol
	};

	Some((format!("{}::{}", module_access_plan.access_path, symbol), rewritten_use_path))
}

fn import004_parent_module_target(path: &str, symbol: &str) -> Option<(String, String)> {
	let mut parent_module_paths = paths::imported_full_paths_from_use_path(path)
		.into_iter()
		.filter(|full_path| {
			paths::symbol_from_full_import_path(full_path)
				.as_deref()
				.is_some_and(|leaf| syntax::is_same_ident(leaf, symbol))
		})
		.filter_map(|full_path| {
			full_path.rsplit_once("::").map(|(parent_module_path, _)| parent_module_path.to_owned())
		})
		.collect::<Vec<_>>();

	parent_module_paths.sort();
	parent_module_paths.dedup();

	if parent_module_paths.len() != 1 {
		return None;
	}

	let parent_module_path = parent_module_paths.into_iter().next()?;
	let module_symbol = parent_module_path.rsplit("::").next()?.trim().to_owned();

	if module_symbol.is_empty() || matches!(module_symbol.as_str(), "crate" | "self" | "super") {
		return None;
	}

	Some((parent_module_path, syntax::normalize_ident(&module_symbol).to_owned()))
}

fn import_conditions(item: &TopItem) -> Vec<String> {
	item.attrs.iter().filter(|attr| attr.trim_start().starts_with("#[cfg")).cloned().collect()
}

fn import004_preferred_module_access_plan(
	ctx: &FileContext,
	current_item: Option<&TopItem>,
	current_use_path: Option<&str>,
	parent_module_path: &str,
	module_symbol: &str,
) -> Option<Import004ModuleAccessPlan> {
	if matches!(module_symbol, "crate" | "self" | "super") {
		return None;
	}

	let compact_parent_module_path = paths::compact_path_for_match(parent_module_path);
	let mut keep_parent_module_import =
		compact_parent_module_path.trim_start_matches("::").contains("::");

	if current_use_path.is_some_and(|path| {
		import004_use_path_imports_parent_module(path, &compact_parent_module_path)
	}) {
		keep_parent_module_import = false;
	}

	for item in &ctx.top_items {
		let Some(other_path) = item.use_path.clone().or_else(|| paths::extract_use_path(ctx, item))
		else {
			if item.kind != TopKind::Use
				&& item
					.name
					.as_deref()
					.is_some_and(|name| syntax::is_same_ident(name, module_symbol))
			{
				return None;
			}

			continue;
		};
		let is_current_item = current_item.is_some_and(|current_item| {
			paths::use_item_lock_key(item) == paths::use_item_lock_key(current_item)
		});

		if item.kind != TopKind::Use {
			if item.name.as_deref().is_some_and(|name| syntax::is_same_ident(name, module_symbol)) {
				return None;
			}

			continue;
		}
		if import004_use_path_imports_parent_module(&other_path, &compact_parent_module_path) {
			let conditions = import_conditions(item);
			let current_conditions = current_item.map(import_conditions).unwrap_or_default();

			if !conditions.is_empty() && conditions != current_conditions {
				return None;
			}

			keep_parent_module_import = false;

			if is_current_item {
				continue;
			}
		}
		if is_current_item {
			continue;
		}
		if import004_use_path_conflicts_with_parent_module(
			&other_path,
			&compact_parent_module_path,
			module_symbol,
		) {
			return None;
		}
	}

	if keep_parent_module_import
		&& import004_has_conflicting_root_qualified_path_usage(
			ctx,
			module_symbol,
			&compact_parent_module_path,
		) {
		return None;
	}

	Some(Import004ModuleAccessPlan {
		access_path: module_symbol.to_owned(),
		keep_parent_module_import,
	})
}

fn import004_use_path_imports_parent_module(path: &str, parent_module_path: &str) -> bool {
	paths::imported_full_paths_from_use_path(path)
		.into_iter()
		.any(|full_path| paths::compact_path_for_match(&full_path) == parent_module_path)
		|| paths::imported_self_full_paths_from_use_path(path)
			.into_iter()
			.any(|full_path| paths::compact_path_for_match(&full_path) == parent_module_path)
}

fn import004_use_path_conflicts_with_parent_module(
	path: &str,
	parent_module_path: &str,
	module_symbol: &str,
) -> bool {
	paths::imported_full_paths_from_use_path(path).into_iter().any(|full_path| {
		import004_imported_full_path_conflicts_with_parent_module(
			&full_path,
			parent_module_path,
			module_symbol,
		)
	}) || paths::imported_self_full_paths_from_use_path(path).into_iter().any(|full_path| {
		import004_imported_self_path_conflicts_with_parent_module(
			&full_path,
			parent_module_path,
			module_symbol,
		)
	})
}

fn import004_imported_full_path_conflicts_with_parent_module(
	full_path: &str,
	parent_module_path: &str,
	module_symbol: &str,
) -> bool {
	let compact_full_path = paths::compact_path_for_match(full_path);

	if compact_full_path == parent_module_path
		|| compact_full_path.starts_with(&format!("{parent_module_path}::"))
	{
		return false;
	}

	import004_root_module_symbol_matches_conflicting_path(&compact_full_path, module_symbol)
		|| paths::symbol_from_full_import_path(&compact_full_path)
			.as_deref()
			.is_some_and(|symbol| syntax::is_same_ident(symbol, module_symbol))
}

fn import004_imported_self_path_conflicts_with_parent_module(
	full_path: &str,
	parent_module_path: &str,
	module_symbol: &str,
) -> bool {
	let compact_full_path = paths::compact_path_for_match(full_path);

	if compact_full_path == parent_module_path
		|| compact_full_path.starts_with(&format!("{parent_module_path}::"))
	{
		return false;
	}

	import004_root_module_symbol_matches_conflicting_path(&compact_full_path, module_symbol)
		|| compact_full_path
			.rsplit("::")
			.next()
			.is_some_and(|segment| syntax::is_same_ident(segment, module_symbol))
}

fn import004_root_module_symbol_matches_conflicting_path(path: &str, module_symbol: &str) -> bool {
	path.trim_start_matches(':')
		.split("::")
		.next()
		.is_some_and(|root| !root.is_empty() && syntax::is_same_ident(root, module_symbol))
}

fn import004_has_conflicting_root_qualified_path_usage(
	ctx: &FileContext,
	module_symbol: &str,
	parent_module_path: &str,
) -> bool {
	import004_has_conflicting_root_qualified_ast_path_usage(ctx, module_symbol, parent_module_path)
		|| import004_has_conflicting_root_qualified_macro_path_usage(
			ctx,
			module_symbol,
			parent_module_path,
		)
}

fn import004_has_conflicting_root_qualified_ast_path_usage(
	ctx: &FileContext,
	module_symbol: &str,
	parent_module_path: &str,
) -> bool {
	for path in ctx.source_file.syntax().descendants().filter_map(Path::cast) {
		if paths::path_is_qualifier_subpath(&path)
			|| path.qualifier().is_none()
			|| !import004_is_in_current_scope(path.syntax())
		{
			continue;
		}
		if path.syntax().ancestors().any(|node| Use::cast(node).is_some()) {
			continue;
		}

		let mut segments = Vec::new();

		if !paths::collect_path_segment_texts(&path, &mut segments) || segments.len() < 2 {
			continue;
		}
		if !segments.first().is_some_and(|root| syntax::is_same_ident(root, module_symbol)) {
			continue;
		}

		let compact_full_path = paths::compact_path_for_match(&segments.join("::"));

		if compact_full_path == parent_module_path
			|| compact_full_path.starts_with(&format!("{parent_module_path}::"))
		{
			continue;
		}

		return true;
	}

	false
}

fn import004_has_conflicting_root_qualified_macro_path_usage(
	ctx: &FileContext,
	module_symbol: &str,
	parent_module_path: &str,
) -> bool {
	for macro_call in ctx.source_file.syntax().descendants().filter_map(MacroCall::cast) {
		if !import004_is_in_current_scope(macro_call.syntax()) {
			continue;
		}

		let Some(token_tree) = macro_call.token_tree() else {
			continue;
		};
		let tree_text = token_tree.syntax().text().to_string();
		let tree_start = usize::from(token_tree.syntax().text_range().start());

		for found in paths::qualified_macro_path_regex().find_iter(&tree_text) {
			if !references::macro_symbol_is_import009_unqualified(
				ctx,
				&tree_text,
				tree_start,
				found.start(),
			) {
				continue;
			}

			let segments = found
				.as_str()
				.split("::")
				.map(str::trim)
				.filter(|segment| !segment.is_empty())
				.collect::<Vec<_>>();

			if segments.len() < 2
				|| !segments.first().is_some_and(|root| syntax::is_same_ident(root, module_symbol))
			{
				continue;
			}

			let compact_full_path = paths::compact_path_for_match(&segments.join("::"));

			if compact_full_path == parent_module_path
				|| compact_full_path.starts_with(&format!("{parent_module_path}::"))
			{
				continue;
			}

			return true;
		}
	}

	false
}

fn import004_is_in_current_scope(syntax: &SyntaxNode) -> bool {
	!syntax.ancestors().any(|node| Module::cast(node).is_some())
}

fn import004_fix_plan_with_parent_module(
	ctx: &FileContext,
	path: &str,
	symbol: &str,
	parent_module_path: &str,
	module_access: &str,
) -> Option<(String, Option<String>)> {
	if let Some((prefix, imported_symbol)) = paths::simple_import_prefix_symbol(path)
		&& syntax::is_same_ident(&imported_symbol, symbol)
	{
		return Some((format!("{module_access}::{imported_symbol}"), Some(prefix)));
	}

	import004_braced_fix_plan_with_parent_module(
		ctx,
		path,
		symbol,
		parent_module_path,
		module_access,
	)
}

fn import004_braced_fix_plan_with_parent_module(
	ctx: &FileContext,
	path: &str,
	symbol: &str,
	parent_module_path: &str,
	module_access: &str,
) -> Option<(String, Option<String>)> {
	let (_default_qualified_symbol_path, rewritten_use_path_without_symbol) =
		paths::braced_import_fix_plan(path, symbol)?;
	let rewritten_use_path = match rewritten_use_path_without_symbol {
		Some(path_without_symbol) => Some(import004_merge_parent_module_use_path(
			ctx,
			&path_without_symbol,
			parent_module_path,
		)?),
		None => Some(parent_module_path.to_owned()),
	};

	Some((format!("{module_access}::{symbol}"), rewritten_use_path))
}

fn import004_merge_parent_module_use_path(
	ctx: &FileContext,
	path_without_symbol: &str,
	parent_module_path: &str,
) -> Option<String> {
	if import004_use_path_imports_parent_module(
		path_without_symbol,
		&paths::compact_path_for_match(parent_module_path),
	) {
		return Some(path_without_symbol.to_owned());
	}

	let (prefix, close, mut segments) =
		paths::parse_braced_path_parts_allow_alias(path_without_symbol)?;
	let parent_segment = import004_parent_module_relative_segment(&prefix, parent_module_path)?;

	if !segments.iter().any(|segment| {
		paths::compact_path_for_match(segment) == paths::compact_path_for_match(&parent_segment)
	}) {
		segments.insert(0, parent_segment);
	}

	let combined_path =
		format!("{prefix}{}{}", segments.join(", "), &path_without_symbol[close..=close]);

	Some(
		groups::normalize_mixed_self_child_use_path_preserve_self(ctx, &combined_path)
			.unwrap_or(combined_path),
	)
}

fn import004_parent_module_relative_segment(
	braced_prefix: &str,
	parent_module_path: &str,
) -> Option<String> {
	let prefix_module_path =
		braced_prefix.trim_end_matches('{').trim().trim_end_matches("::").trim();
	let compact_prefix_module_path = paths::compact_path_for_match(prefix_module_path);
	let compact_parent_module_path = paths::compact_path_for_match(parent_module_path);

	if compact_prefix_module_path == compact_parent_module_path {
		return Some("self".to_owned());
	}

	let relative =
		compact_parent_module_path.strip_prefix(&format!("{compact_prefix_module_path}::"))?.trim();

	if relative.is_empty() { None } else { Some(relative.to_owned()) }
}
