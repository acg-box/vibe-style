use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use ast::Path;
use ra_ap_syntax::{
	AstNode,
	ast::{self, HasAttrs},
};
use regex::Regex;

use super::shared::{
	self, Edit, FileContext, TopItem, TopKind, USE_RE, Violation, WORKSPACE_IMPORT_ROOTS,
};

type Import009Plan<'a> = (
	bool,
	Vec<(usize, usize, String)>,
	Vec<(usize, usize, String)>,
	Vec<(&'a TopItem, String, Option<String>)>,
);

#[derive(Debug, Clone)]
struct Import008Candidate {
	line: usize,
	start: usize,
	end: usize,
	symbol: String,
	import_path: String,
	replacement: String,
}

#[derive(Default)]
struct ImportedSymbolMaps {
	symbol_paths: HashMap<String, HashSet<String>>,
	symbol_lines: HashMap<String, HashSet<usize>>,
	full_paths_by_symbol: HashMap<String, HashSet<String>>,
}

struct Import009Context<'a> {
	use_items: &'a [&'a TopItem],
	maps: &'a ImportedSymbolMaps,
	local_defined_symbols: &'a HashSet<String>,
	qualified_type_paths_by_symbol: &'a HashMap<String, HashSet<String>>,
	glob_roots: &'a HashSet<String>,
}

struct UseItemFlags {
	has_prelude_glob: bool,
	glob_roots: HashSet<String>,
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

#[derive(Clone)]
enum BracedImportSegment {
	Simple(String),
	Nested { head: String, children: Vec<String> },
}

pub(crate) fn check_import_rules(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	if ctx.path.file_name().is_some_and(|name| name == "error.rs") {
		return;
	}

	let local_module_roots = collect_local_module_roots(ctx);
	let use_runs = collect_non_pub_use_runs(ctx);
	let use_items = use_runs.iter().flat_map(|run| run.iter().copied()).collect::<Vec<_>>();
	let use_flags = collect_use_item_flags(ctx, &use_items);
	let (import007_lines, import004_fixed_lines) = apply_use_item_rules(
		ctx,
		violations,
		edits,
		emit_edits,
		&use_items,
		use_flags.has_prelude_glob,
	);
	let imported_symbol_maps = collect_imported_symbol_maps(ctx, &use_items);

	push_import004_ambiguous_symbol_violations(ctx, violations, &imported_symbol_maps);

	let local_defined_symbols = collect_local_defined_symbols(ctx);
	let qualified_type_paths_by_symbol = collect_qualified_type_paths_by_symbol(ctx);
	let import009_ctx = Import009Context {
		use_items: &use_items,
		maps: &imported_symbol_maps,
		local_defined_symbols: &local_defined_symbols,
		qualified_type_paths_by_symbol: &qualified_type_paths_by_symbol,
		glob_roots: &use_flags.glob_roots,
	};
	let import009_fixed_lines =
		apply_import009_rules(ctx, violations, edits, emit_edits, &import009_ctx);
	let import008_group_skip_lines = apply_import008_rules(
		ctx,
		violations,
		edits,
		emit_edits,
		&use_runs,
		&local_module_roots,
		&local_defined_symbols,
		&imported_symbol_maps.full_paths_by_symbol,
		use_flags.has_prelude_glob,
		&use_flags.glob_roots,
	);
	let mut import_group_skip_lines = import007_lines;

	import_group_skip_lines.extend(import004_fixed_lines);
	import_group_skip_lines.extend(import009_fixed_lines);
	import_group_skip_lines.extend(import008_group_skip_lines);

	let (planned_import_group_edits, fixable_import_group_lines) =
		build_import_group_fix_plans(ctx, &use_runs, &import_group_skip_lines, &local_module_roots);

	if emit_edits {
		edits.extend(planned_import_group_edits);
	}

	push_import_group_order_spacing_violations(
		ctx,
		violations,
		&use_runs,
		&fixable_import_group_lines,
		&local_module_roots,
	);
}

fn collect_use_item_flags(ctx: &FileContext, use_items: &[&TopItem]) -> UseItemFlags {
	let mut has_prelude_glob = false;
	let mut glob_roots = HashSet::new();

	for item in use_items {
		let Some(path) = extract_use_path(ctx, item) else {
			continue;
		};
		let compact_path = path.replace(' ', "");

		if compact_path == "crate::prelude::*" {
			has_prelude_glob = true;
		}
		if let Some(root) = glob_import_root(&compact_path) {
			glob_roots.insert(root);
		}
	}

	UseItemFlags { has_prelude_glob, glob_roots }
}

fn apply_use_item_rules(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	use_items: &[&TopItem],
	has_prelude_glob: bool,
) -> (HashSet<usize>, HashSet<usize>) {
	let mut import007_lines = HashSet::new();
	let mut import004_fixed_lines = HashSet::new();

	for item in use_items {
		let Some(path) = extract_use_path(ctx, item) else {
			continue;
		};
		if apply_import009_std_fmt_result_rule(ctx, violations, edits, emit_edits, item, &path) {
			import004_fixed_lines.insert(item.line);

			continue;
		}
		if apply_import009_non_importable_root_use_rule(
			ctx, violations, edits, emit_edits, item, &path,
		) {
			import004_fixed_lines.insert(item.line);

			continue;
		}

		apply_import002_normalization_rule(ctx, violations, edits, emit_edits, item, &path);
		push_alias_violation_if_needed(ctx, violations, item, &path);

		if apply_import007_rule(ctx, violations, edits, emit_edits, item, &path, has_prelude_glob) {
			import007_lines.insert(item.line);
		}
		if apply_import004_free_fn_macro_rule(ctx, violations, edits, emit_edits, item, &path) {
			import004_fixed_lines.insert(item.line);
		}
	}

	(import007_lines, import004_fixed_lines)
}

fn apply_import009_std_fmt_result_rule(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	item: &TopItem,
	path: &str,
) -> bool {
	if !imported_full_paths_from_use_path(path).iter().any(|full| full == "std::fmt::Result") {
		return false;
	}

	let nongeneric_rewrites =
		unqualified_nongeneric_type_path_rewrites(ctx, "Result", "std::fmt::Result");
	let has_generic_result_uses = has_unqualified_generic_type_path_use(ctx, "Result");
	let Some((_qualified_symbol_path, rewritten_use_path)) = import004_fix_plan(path, "Result")
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

	apply_import009_use_item_rewrite(ctx, edits, item, rewritten_use_path.as_deref())
}

fn apply_import009_non_importable_root_use_rule(
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

	if !is_non_importable_use_root(root) {
		return false;
	}

	let mut rewrites = Vec::new();

	for full_path in imported_full_paths_from_use_path(path) {
		let Some(symbol) = symbol_from_full_import_path(&full_path) else {
			continue;
		};

		rewrites.extend(unqualified_type_path_rewrites(ctx, &symbol, &full_path));
		rewrites.extend(unqualified_value_path_rewrites(ctx, &symbol, &full_path));
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

	apply_import009_use_item_rewrite(ctx, edits, item, None)
}

fn apply_import002_normalization_rule(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	item: &TopItem,
	path: &str,
) {
	let Some(normalized) = normalize_mixed_self_child_use_path(path) else {
		return;
	};

	shared::push_violation(
		violations,
		ctx,
		item.line,
		"RUST-STYLE-IMPORT-002",
		"Normalize imports like `use a::{b, b::c}` to `use a::{b::{self, c}}`.",
		true,
	);

	if !emit_edits {
		return;
	}

	if let Some((start, end)) = item_text_range(ctx, item)
		&& let Some(raw) = ctx.text.get(start..end)
		&& let Some(rewritten) = rewrite_use_item_with_path(raw, &normalized)
	{
		edits.push(Edit { start, end, replacement: rewritten, rule: "RUST-STYLE-IMPORT-002" });
	}
}

fn push_alias_violation_if_needed(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	item: &TopItem,
	path: &str,
) {
	let Some(alias_caps) = Regex::new(r"\bas\s+([A-Za-z_][A-Za-z0-9_]*)\b")
		.expect("Expected operation to succeed.")
		.captures(path)
	else {
		return;
	};

	if alias_caps.get(1).map(|m| m.as_str()) != Some("_") {
		shared::push_violation(
			violations,
			ctx,
			item.line,
			"RUST-STYLE-IMPORT-003",
			"Import aliases are not allowed except `as _` in test keep-alive modules.",
			false,
		);
	}
}

fn apply_import007_rule(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	item: &TopItem,
	path: &str,
	has_prelude_glob: bool,
) -> bool {
	let compact_path = path.replace(' ', "");

	if !(has_prelude_glob
		&& compact_path.starts_with("crate::")
		&& compact_path != "crate::prelude::*")
	{
		return false;
	}

	shared::push_violation(
		violations,
		ctx,
		item.line,
		"RUST-STYLE-IMPORT-007",
		"Avoid redundant crate imports when crate::prelude::* is imported.",
		true,
	);

	if emit_edits
		&& let (Some(start), Some(next)) = (
			shared::offset_from_line(&ctx.line_starts, item.start_line),
			shared::offset_from_line(&ctx.line_starts, item.end_line + 1),
		) {
		edits.push(Edit {
			start,
			end: next,
			replacement: String::new(),
			rule: "RUST-STYLE-IMPORT-007",
		});
	}

	true
}

fn apply_import004_free_fn_macro_rule(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	item: &TopItem,
	path: &str,
) -> bool {
	if !path.contains("::") {
		return false;
	}

	for symbol in imported_symbols_from_use_path(path) {
		if symbol.is_empty() || !symbol.chars().next().is_some_and(char::is_lowercase) {
			continue;
		}

		let local_fn_defined = is_local_fn_defined(ctx, &symbol);
		let local_macro_defined = is_local_macro_defined(ctx, &symbol);
		let fn_ranges = unqualified_function_call_ranges(ctx, &symbol);
		let macro_ranges = unqualified_macro_call_ranges(ctx, &symbol);
		let needs_fn_fix = !fn_ranges.is_empty() && !local_fn_defined;
		let needs_macro_fix = !macro_ranges.is_empty() && !local_macro_defined;

		if !(needs_fn_fix || needs_macro_fix) {
			continue;
		}

		let mut fixed_this_item = false;
		let mut fixable = false;

		if let Some((qualified_symbol_path, rewritten_use_path)) = import004_fix_plan(path, &symbol)
		{
			fixable = true;

			if emit_edits {
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

				fixed_this_item = apply_import004_use_item_rewrite(
					ctx,
					edits,
					item,
					rewritten_use_path.as_deref(),
				);
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

		return fixed_this_item;
	}

	false
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

fn apply_import004_use_item_rewrite(
	ctx: &FileContext,
	edits: &mut Vec<Edit>,
	item: &TopItem,
	rewritten_use_path: Option<&str>,
) -> bool {
	if let Some(new_use_path) = rewritten_use_path {
		if let Some((start, end)) = item_text_range(ctx, item)
			&& let Some(raw) = ctx.text.get(start..end)
			&& let Some(rewritten) = rewrite_use_item_with_path(raw, new_use_path)
		{
			edits.push(Edit { start, end, replacement: rewritten, rule: "RUST-STYLE-IMPORT-004" });

			return true;
		}

		return false;
	}
	if let (Some(start), Some(next)) = (
		shared::offset_from_line(&ctx.line_starts, item.start_line),
		shared::offset_from_line(&ctx.line_starts, item.end_line + 1),
	) {
		edits.push(Edit {
			start,
			end: next,
			replacement: String::new(),
			rule: "RUST-STYLE-IMPORT-004",
		});

		return true;
	}

	false
}

fn collect_imported_symbol_maps(ctx: &FileContext, use_items: &[&TopItem]) -> ImportedSymbolMaps {
	let mut maps = ImportedSymbolMaps::default();

	for item in use_items {
		let Some(path) = extract_use_path(ctx, item) else {
			continue;
		};

		for symbol in imported_symbols_from_use_path(&path) {
			maps.symbol_paths.entry(symbol.clone()).or_default().insert(path.clone());
			maps.symbol_lines.entry(symbol).or_default().insert(item.line);
		}
		for full_path in imported_full_paths_from_use_path(&path) {
			let Some(symbol) = symbol_from_full_import_path(&full_path) else {
				continue;
			};

			maps.full_paths_by_symbol.entry(symbol).or_default().insert(full_path);
		}
	}

	maps
}

fn push_import004_ambiguous_symbol_violations(
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

fn collect_local_defined_symbols(ctx: &FileContext) -> HashSet<String> {
	let mut out = HashSet::new();

	for item in &ctx.top_items {
		let Some(name) = item.name.as_deref() else {
			continue;
		};

		out.insert(normalize_ident(name).to_owned());
	}

	out
}

fn apply_import009_rules(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	import009_ctx: &Import009Context<'_>,
) -> HashSet<usize> {
	let mut import009_fixed_lines = HashSet::new();

	for (symbol, imported_paths) in &import009_ctx.maps.full_paths_by_symbol {
		if imported_paths.len() != 1 || import009_ctx.local_defined_symbols.contains(symbol) {
			continue;
		}

		let Some(imported_path) = imported_paths.iter().next().cloned() else {
			continue;
		};
		let Some((fixable, type_rewrites, value_rewrites, use_item_plans)) = build_import009_plan(
			ctx,
			import009_ctx.use_items,
			symbol,
			&imported_path,
			import009_ctx.qualified_type_paths_by_symbol,
			import009_ctx.glob_roots,
		) else {
			continue;
		};

		for line in
			import009_ctx.maps.symbol_lines.get(symbol).into_iter().flat_map(|lines| lines.iter())
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

		for (start, end, replacement) in type_rewrites {
			edits.push(Edit { start, end, replacement, rule: "RUST-STYLE-IMPORT-009" });
		}
		for (start, end, replacement) in value_rewrites {
			edits.push(Edit { start, end, replacement, rule: "RUST-STYLE-IMPORT-009" });
		}
		for (item, _qualified_symbol_path, rewritten_use_path) in use_item_plans {
			if apply_import009_use_item_rewrite(ctx, edits, item, rewritten_use_path.as_deref()) {
				import009_fixed_lines.insert(item.line);
			}
		}
	}

	import009_fixed_lines
}

fn build_import009_plan<'a>(
	ctx: &FileContext,
	use_items: &'a [&'a TopItem],
	symbol: &str,
	imported_path: &str,
	qualified_type_paths_by_symbol: &HashMap<String, HashSet<String>>,
	glob_roots: &HashSet<String>,
) -> Option<Import009Plan<'a>> {
	let type_rewrites = unqualified_type_path_rewrites(ctx, symbol, imported_path);
	let value_rewrites = unqualified_value_path_rewrites(ctx, symbol, imported_path);
	let has_unqualified_uses = !type_rewrites.is_empty() || !value_rewrites.is_empty();
	let has_other_qualified_path = qualified_type_paths_by_symbol
		.get(symbol)
		.is_some_and(|paths| paths.iter().any(|path| path != imported_path));
	let has_local_glob_shadow_ambiguity =
		has_high_risk_glob_shadow_ambiguity(symbol, imported_path, glob_roots)
			&& has_unqualified_uses;

	if !has_unqualified_uses || (!has_other_qualified_path && !has_local_glob_shadow_ambiguity) {
		return None;
	}

	let mut use_item_plans = Vec::new();
	let mut fixable = true;

	for item in use_items {
		let Some(path) = extract_use_path(ctx, item) else {
			continue;
		};

		if !use_item_imports_symbol_path(&path, symbol, imported_path) {
			continue;
		}

		let Some((qualified_symbol_path, rewritten_use_path)) = import004_fix_plan(&path, symbol)
		else {
			fixable = false;

			break;
		};

		use_item_plans.push((*item, qualified_symbol_path, rewritten_use_path));
	}

	if use_item_plans.is_empty() {
		fixable = false;
	}

	Some((fixable, type_rewrites, value_rewrites, use_item_plans))
}

fn apply_import009_use_item_rewrite(
	ctx: &FileContext,
	edits: &mut Vec<Edit>,
	item: &TopItem,
	rewritten_use_path: Option<&str>,
) -> bool {
	if let Some(new_use_path) = rewritten_use_path {
		if let Some((start, end)) = item_text_range(ctx, item)
			&& let Some(raw) = ctx.text.get(start..end)
			&& let Some(rewritten) = rewrite_use_item_with_path(raw, new_use_path)
		{
			edits.push(Edit { start, end, replacement: rewritten, rule: "RUST-STYLE-IMPORT-009" });

			return true;
		}

		return false;
	}
	if let (Some(start), Some(next)) = (
		shared::offset_from_line(&ctx.line_starts, item.start_line),
		shared::offset_from_line(&ctx.line_starts, item.end_line + 1),
	) {
		edits.push(Edit {
			start,
			end: next,
			replacement: String::new(),
			rule: "RUST-STYLE-IMPORT-009",
		});

		return true;
	}

	false
}

#[allow(clippy::too_many_arguments)]
fn apply_import008_rules(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	use_runs: &[Vec<&TopItem>],
	local_module_roots: &HashSet<String>,
	local_defined_symbols: &HashSet<String>,
	imported_full_paths_by_symbol: &HashMap<String, HashSet<String>>,
	has_prelude_glob: bool,
	glob_roots: &HashSet<String>,
) -> HashSet<usize> {
	let import008_candidates = collect_import008_candidates(ctx, has_prelude_glob);
	let mut candidate_paths_by_symbol: HashMap<String, HashSet<String>> = HashMap::new();

	for candidate in &import008_candidates {
		candidate_paths_by_symbol
			.entry(candidate.symbol.clone())
			.or_default()
			.insert(candidate.import_path.clone());
	}

	let mut blocked_symbols = HashSet::new();

	for (symbol, candidate_paths) in &candidate_paths_by_symbol {
		let mut all_paths = imported_full_paths_by_symbol.get(symbol).cloned().unwrap_or_default();

		all_paths.extend(candidate_paths.iter().cloned());

		if all_paths.len() > 1 || local_defined_symbols.contains(symbol) {
			blocked_symbols.insert(symbol.clone());
		}
		if candidate_paths
			.iter()
			.any(|path| has_high_risk_glob_shadow_ambiguity(symbol, path, glob_roots))
		{
			blocked_symbols.insert(symbol.clone());
		}
	}

	let mut pending_import_paths = BTreeSet::new();
	let mut import008_group_skip_lines = HashSet::new();

	for candidate in import008_candidates {
		if blocked_symbols.contains(&candidate.symbol) {
			continue;
		}

		shared::push_violation(
			violations,
			ctx,
			candidate.line,
			"RUST-STYLE-IMPORT-008",
			"Prefer importing non-function, non-macro symbols and using short paths when unambiguous.",
			true,
		);

		if !emit_edits {
			continue;
		}

		let already_imported = imported_full_paths_by_symbol
			.get(&candidate.symbol)
			.is_some_and(|paths| paths.contains(&candidate.import_path));

		edits.push(Edit {
			start: candidate.start,
			end: candidate.end,
			replacement: candidate.replacement,
			rule: "RUST-STYLE-IMPORT-008",
		});
		import008_group_skip_lines.insert(candidate.line);

		if !already_imported {
			pending_import_paths.insert(candidate.import_path);
		}
	}

	if emit_edits {
			let (pending_after_merge, merged_lines) = merge_import008_into_existing_module_use_items(
				ctx,
				edits,
				use_runs,
				&pending_import_paths,
				imported_full_paths_by_symbol,
			);

		import008_group_skip_lines.extend(merged_lines);

		if let Some((edit, touched_lines)) =
			build_import008_insert_edit(ctx, use_runs, local_module_roots, &pending_after_merge)
		{
			edits.push(edit);
			import008_group_skip_lines.extend(touched_lines);
		}
	}

	import008_group_skip_lines
}

fn push_import_group_order_spacing_violations(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	use_runs: &[Vec<&TopItem>],
	fixable_import_group_lines: &HashSet<usize>,
	local_module_roots: &HashSet<String>,
) {
	for run in use_runs {
		for pair in run.windows(2) {
			let prev = pair[0];
			let curr = pair[1];
			let Some(prev_path) = extract_use_path(ctx, prev) else {
				continue;
			};
			let Some(curr_path) = extract_use_path(ctx, curr) else {
				continue;
			};
			let prev_origin = use_origin(&prev_path, local_module_roots);
			let curr_origin = use_origin(&curr_path, local_module_roots);
			let is_fixable = fixable_import_group_lines.contains(&curr.line);
			let between = separator_lines(ctx, prev, curr);
			let has_blank = between.iter().any(|line| line.trim().is_empty());
			let has_header_comment = between.iter().any(|line| line.trim_start().starts_with("//"));

			if curr_origin < prev_origin {
				shared::push_violation(
					violations,
					ctx,
					curr.line,
					"RUST-STYLE-IMPORT-001",
					"Import groups must be ordered: std, third-party, self/workspace.",
					is_fixable,
				);
			}
			if curr_origin != prev_origin && !has_blank {
				shared::push_violation(
					violations,
					ctx,
					curr.line,
					"RUST-STYLE-IMPORT-002",
					"Separate import groups with one blank line.",
					is_fixable,
				);
			}
			if curr_origin == prev_origin && has_blank {
				shared::push_violation(
					violations,
					ctx,
					curr.line,
					"RUST-STYLE-IMPORT-002",
					"Do not place blank lines inside an import group.",
					is_fixable,
				);
			}
			if has_header_comment {
				shared::push_violation(
					violations,
					ctx,
					curr.line,
					"RUST-STYLE-IMPORT-002",
					"Do not use header comments for import groups.",
					is_fixable,
				);
			}
		}
	}
}

fn collect_import008_candidates(
	ctx: &FileContext,
	has_prelude_glob: bool,
) -> Vec<Import008Candidate> {
	let mut candidates = Vec::new();
	let mut seen_ranges = HashSet::new();

	for path_type in ctx.source_file.syntax().descendants().filter_map(ast::PathType::cast) {
		let Some(path) = path_type.path() else {
			continue;
		};

		if path.qualifier().is_none() || is_inside_cfg_test_module(&path) {
			continue;
		}

		let mut segments = Vec::new();

		if !collect_path_segment_texts(&path, &mut segments) {
			continue;
		}
		if segments.len() < 2 {
			continue;
		}

		let symbol = normalize_ident(segments[segments.len() - 1].as_str()).to_owned();

		if matches!(symbol.as_str(), "" | "self" | "super" | "crate" | "Self") {
			continue;
		}
		let root = segments[0].as_str();

		if is_non_importable_root(root) {
			continue;
		}

		let import_path = segments.join("::");

		if matches!(
			import_path.as_str(),
			"std::fmt::Result"
				| "core::fmt::Result"
				| "std::result::Result"
				| "core::result::Result"
		) {
			continue;
		}

		if has_prelude_glob && import_path.starts_with("crate::") {
			continue;
		}

		let Some(segment) = path.segment() else {
			continue;
		};
		let replacement = segment.syntax().text().to_string();
		let range = path.syntax().text_range();
		let start = usize::from(range.start());
		let end = usize::from(range.end());

		if start >= end || !seen_ranges.insert((start, end)) {
			continue;
		}

		let line = shared::line_from_offset(&ctx.line_starts, start);

		candidates.push(Import008Candidate { line, start, end, symbol, import_path, replacement });
	}

	candidates
}

fn collect_qualified_type_paths_by_symbol(ctx: &FileContext) -> HashMap<String, HashSet<String>> {
	let mut out: HashMap<String, HashSet<String>> = HashMap::new();

	for path_type in ctx.source_file.syntax().descendants().filter_map(ast::PathType::cast) {
		let Some(path) = path_type.path() else {
			continue;
		};

		if path.qualifier().is_none() || is_inside_cfg_test_module(&path) {
			continue;
		}

		let mut segments = Vec::new();

		if !collect_path_segment_texts(&path, &mut segments) {
			continue;
		}
		if segments.len() < 2 {
			continue;
		}
		let root = segments[0].as_str();

		if is_non_importable_root(root) {
			continue;
		}

		let full_path = segments.join("::");
		let Some(symbol) = symbol_from_full_import_path(&full_path) else {
			continue;
		};

		out.entry(symbol).or_default().insert(full_path);
	}

	out
}

fn unqualified_type_path_rewrites(
	ctx: &FileContext,
	symbol: &str,
	qualified_path: &str,
) -> Vec<(usize, usize, String)> {
	let mut rewrites = Vec::new();

	for path_type in ctx.source_file.syntax().descendants().filter_map(ast::PathType::cast) {
		let Some(path) = path_type.path() else {
			continue;
		};

		if path.qualifier().is_some() || is_inside_cfg_test_module(&path) {
			continue;
		}

		let Some(segment) = path.segment() else {
			continue;
		};
		let Some(name_ref) = segment.name_ref() else {
			continue;
		};

		if !is_same_ident(name_ref.text().as_str(), symbol) {
			continue;
		}

		let segment_text = segment.syntax().text().to_string();
		let name_text = name_ref.text().to_string();
		let suffix = segment_text.strip_prefix(&name_text).unwrap_or_default();

		rewrites.push((
			usize::from(path.syntax().text_range().start()),
			usize::from(path.syntax().text_range().end()),
			format!("{qualified_path}{suffix}"),
		));
	}

	rewrites
}

fn unqualified_nongeneric_type_path_rewrites(
	ctx: &FileContext,
	symbol: &str,
	qualified_path: &str,
) -> Vec<(usize, usize, String)> {
	let mut rewrites = Vec::new();

	for path_type in ctx.source_file.syntax().descendants().filter_map(ast::PathType::cast) {
		let Some(path) = path_type.path() else {
			continue;
		};

		if path.qualifier().is_some() || is_inside_cfg_test_module(&path) {
			continue;
		}

		let Some(segment) = path.segment() else {
			continue;
		};
		let Some(name_ref) = segment.name_ref() else {
			continue;
		};

		if !is_same_ident(name_ref.text().as_str(), symbol) {
			continue;
		}

		let segment_text = segment.syntax().text().to_string();
		let name_text = name_ref.text().to_string();
		let suffix = segment_text.strip_prefix(&name_text).unwrap_or_default();

		if !suffix.is_empty() {
			continue;
		}

		rewrites.push((
			usize::from(path.syntax().text_range().start()),
			usize::from(path.syntax().text_range().end()),
			qualified_path.to_owned(),
		));
	}

	rewrites
}

fn has_unqualified_generic_type_path_use(ctx: &FileContext, symbol: &str) -> bool {
	for path_type in ctx.source_file.syntax().descendants().filter_map(ast::PathType::cast) {
		let Some(path) = path_type.path() else {
			continue;
		};

		if path.qualifier().is_some() || is_inside_cfg_test_module(&path) {
			continue;
		}

		let Some(segment) = path.segment() else {
			continue;
		};
		let Some(name_ref) = segment.name_ref() else {
			continue;
		};

		if !is_same_ident(name_ref.text().as_str(), symbol) {
			continue;
		}

		let segment_text = segment.syntax().text().to_string();
		let name_text = name_ref.text().to_string();
		let suffix = segment_text.strip_prefix(&name_text).unwrap_or_default();

		if !suffix.is_empty() {
			return true;
		}
	}

	false
}

fn unqualified_value_path_rewrites(
	ctx: &FileContext,
	symbol: &str,
	qualified_path: &str,
) -> Vec<(usize, usize, String)> {
	let mut rewrites = Vec::new();

	for path in ctx.source_file.syntax().descendants().filter_map(ast::Path::cast) {
		if path.qualifier().is_some() || is_inside_cfg_test_module(&path) {
			continue;
		}
		if path.syntax().ancestors().any(|node| ast::Use::cast(node).is_some()) {
			continue;
		}
		if path.syntax().ancestors().any(|node| ast::PathType::cast(node).is_some()) {
			continue;
		}
		if path.syntax().ancestors().any(|node| ast::MacroCall::cast(node).is_some()) {
			continue;
		}
		if !path.syntax().ancestors().any(|node| {
			ast::PathExpr::cast(node.clone()).is_some() || ast::PathPat::cast(node).is_some()
		}) {
			continue;
		}

		let Some(segment) = path.segment() else {
			continue;
		};
		let Some(name_ref) = segment.name_ref() else {
			continue;
		};

		if !is_same_ident(name_ref.text().as_str(), symbol) {
			continue;
		}

		let segment_text = segment.syntax().text().to_string();
		let name_text = name_ref.text().to_string();
		let suffix = segment_text.strip_prefix(&name_text).unwrap_or_default();

		rewrites.push((
			usize::from(path.syntax().text_range().start()),
			usize::from(path.syntax().text_range().end()),
			format!("{qualified_path}{suffix}"),
		));
	}

	rewrites
}

fn use_item_imports_symbol_path(path: &str, symbol: &str, import_path: &str) -> bool {
	if !imported_symbols_from_use_path(path).iter().any(|item_symbol| item_symbol == symbol) {
		return false;
	}

	imported_full_paths_from_use_path(path).into_iter().any(|path| path == import_path)
}

fn is_inside_cfg_test_module(path: &Path) -> bool {
	path.syntax().ancestors().filter_map(ast::Module::cast).any(|module| {
		module
			.attrs()
			.any(|attr| attr.syntax().text().to_string().replace(' ', "").contains("cfg(test)"))
	})
}

fn collect_path_segment_texts(path: &Path, out: &mut Vec<String>) -> bool {
	if let Some(qualifier) = path.qualifier()
		&& !collect_path_segment_texts(&qualifier, out)
	{
		return false;
	}

	let Some(segment) = path.segment() else {
		return false;
	};

	if segment.type_anchor().is_some()
		|| segment.parenthesized_arg_list().is_some()
		|| segment.ret_type().is_some()
		|| segment.return_type_syntax().is_some()
	{
		return false;
	}

	if let Some(name_ref) = segment.name_ref() {
		out.push(name_ref.text().to_string());

		return true;
	}

	let raw = segment.syntax().text().to_string();
	let head = raw.split('<').next().unwrap_or(raw.as_str()).trim();

	if matches!(head, "crate" | "self" | "super") {
		out.push(head.to_owned());

		true
	} else {
		false
	}
}

fn symbol_from_full_import_path(path: &str) -> Option<String> {
	let symbol = path.rsplit("::").next()?.trim();
	let symbol = normalize_ident(symbol);

	if matches!(symbol, "" | "*" | "self" | "super" | "crate") {
		None
	} else {
		Some(symbol.to_owned())
	}
}

fn imported_full_paths_from_use_path(path: &str) -> Vec<String> {
	let mut paths = Vec::new();

	if !collect_full_paths_from_use_segment(path.trim(), &mut paths) {
		return Vec::new();
	}

	paths
		.into_iter()
		.map(|path| path.replace(' ', ""))
		.filter(|path| !path.is_empty() && !path.ends_with("::*"))
		.collect()
}

fn collect_full_paths_from_use_segment(segment: &str, out: &mut Vec<String>) -> bool {
	let trimmed = segment.trim();

	if trimmed.is_empty() {
		return true;
	}
	if trimmed.ends_with("::*") {
		return true;
	}

	let mut brace_start = None;
	let mut depth = 0_i32;
	let mut brace_end = None;

	for (idx, ch) in trimmed.char_indices() {
		if ch == '{' {
			if brace_start.is_none() {
				brace_start = Some(idx);
			}

			depth += 1;
		} else if ch == '}' {
			depth -= 1;

			if depth < 0 {
				return false;
			}
			if depth == 0 {
				brace_end = Some(idx);
			}
		}
	}

	if depth != 0 {
		return false;
	}

	if let (Some(open), Some(close)) = (brace_start, brace_end) {
		let prefix = trimmed[..open].trim();
		let inner = &trimmed[open + 1..close];
		let suffix = trimmed[close + 1..].trim();

		if !suffix.is_empty() {
			return false;
		}
		if !prefix.is_empty() && !prefix.ends_with("::") {
			return false;
		}

		let prefix = prefix.strip_suffix("::").unwrap_or(prefix).trim();

		for child in split_top_level_csv(inner) {
			let child = child.trim();

			if child.is_empty() {
				continue;
			}
			if child == "self" {
				if !prefix.is_empty() {
					out.push(prefix.to_owned());
				}

				continue;
			}

			let expanded =
				if prefix.is_empty() { child.to_owned() } else { format!("{prefix}::{child}") };

			if !collect_full_paths_from_use_segment(&expanded, out) {
				return false;
			}
		}

		return true;
	}

	let base = trimmed.split(" as ").next().unwrap_or(trimmed).trim();

	if base.is_empty() || base == "self" || base == "super" || base == "crate" || base == "*" {
		return true;
	}

	out.push(base.to_owned());

	true
}

fn build_import008_insert_edit(
	ctx: &FileContext,
	use_runs: &[Vec<&TopItem>],
	local_module_roots: &HashSet<String>,
	pending_import_paths: &BTreeSet<String>,
) -> Option<(Edit, HashSet<usize>)> {
	if pending_import_paths.is_empty() {
		return None;
	}

	let mut grouped: BTreeMap<usize, Vec<String>> = BTreeMap::new();

	for path in pending_import_paths {
		grouped.entry(use_origin(path, local_module_roots)).or_default().push(path.clone());
	}
	for paths in grouped.values_mut() {
		paths.sort();
		paths.dedup();
	}

	let mut block = String::new();

	for (group_idx, paths) in grouped.values().enumerate() {
		if group_idx > 0 {
			block.push_str("\n\n");
		}

		for (idx, path) in paths.iter().enumerate() {
			if idx > 0 {
				block.push('\n');
			}

			block.push_str("use ");
			block.push_str(path);
			block.push(';');
		}
	}

	if block.is_empty() {
		return None;
	}

	block.push('\n');

	if let Some(run) = use_runs.first() {
		let (Some(first), Some(last)) = (run.first(), run.last()) else {
			return None;
		};
		let (_, run_end) = run_text_range(ctx, first, last)?;
		let last_origin =
			extract_use_path(ctx, last).map(|path| use_origin(&path, local_module_roots));
		let first_new_origin = grouped.keys().next().copied();
		let mut replacement = String::new();

		if let (Some(last_origin), Some(first_new_origin)) = (last_origin, first_new_origin)
			&& last_origin != first_new_origin
		{
			replacement.push('\n');
		}

		replacement.push_str(&block);

		return Some((
			Edit { start: run_end, end: run_end, replacement, rule: "RUST-STYLE-IMPORT-008" },
			run.iter().map(|item| item.line).collect::<HashSet<_>>(),
		));
	}

	let insert_line = import008_insert_line(ctx);
	let insert_pos =
		shared::offset_from_line(&ctx.line_starts, insert_line).unwrap_or(ctx.text.len());
	let mut replacement = block;

	if !ctx.top_items.is_empty() {
		replacement.push('\n');
	}

	Some((
		Edit { start: insert_pos, end: insert_pos, replacement, rule: "RUST-STYLE-IMPORT-008" },
		HashSet::new(),
	))
}

fn merge_import008_into_existing_module_use_items(
	ctx: &FileContext,
	edits: &mut Vec<Edit>,
	use_runs: &[Vec<&TopItem>],
	pending_import_paths: &BTreeSet<String>,
	imported_full_paths_by_symbol: &HashMap<String, HashSet<String>>,
) -> (BTreeSet<String>, HashSet<usize>) {
	if pending_import_paths.is_empty() {
		return (BTreeSet::new(), HashSet::new());
	}

	#[derive(Clone)]
	struct MergeTarget {
		anchor_line: usize,
	}

	let mut remaining = BTreeSet::new();
	let mut plans: BTreeMap<usize, (String, BTreeSet<String>)> = BTreeMap::new();
	let mut touched_lines = HashSet::new();
	let mut use_items = HashMap::new();
	let mut merge_targets_by_pending: HashMap<String, MergeTarget> = HashMap::new();
	let mut successful_merge_lines = HashSet::new();

	for item in use_runs.iter().flat_map(|run| run.iter().copied()) {
		use_items.insert(item.line, item);
	}

	for pending in pending_import_paths {
		let Some((root, child_tail)) = pending.split_once("::") else {
			remaining.insert(pending.clone());

			continue;
		};
		let child_tail = child_tail.trim();

		if child_tail.is_empty() {
			remaining.insert(pending.clone());

			continue;
		}
		let Some(root_full_paths) = imported_full_paths_by_symbol.get(root) else {
			remaining.insert(pending.clone());

			continue;
		};
		if root_full_paths.len() != 1 {
			remaining.insert(pending.clone());

			continue;
		}
		let Some(root_full) = root_full_paths.iter().next() else {
			remaining.insert(pending.clone());

			continue;
		};
		let Some(anchor_line) = find_exact_use_item_line_for_path(ctx, use_runs, root_full) else {
			remaining.insert(pending.clone());

			continue;
		};

		plans
			.entry(anchor_line)
			.and_modify(|(_, children)| {
				children.insert(child_tail.to_owned());
			})
			.or_insert_with(|| {
				let mut children = BTreeSet::new();

				children.insert(child_tail.to_owned());

				(root_full.clone(), children)
			});
		merge_targets_by_pending.insert(pending.clone(), MergeTarget { anchor_line });
	}

	for (line, (root_full, children)) in plans {
		let Some(item) = use_items.get(&line).copied() else {
			continue;
		};
		let Some((start, end)) = item_text_range(ctx, item) else {
			continue;
		};
		let Some(raw) = ctx.text.get(start..end) else {
			continue;
		};
		let merged_path = format!(
			"{root_full}::{{self, {}}}",
			children.into_iter().collect::<Vec<_>>().join(", ")
		);

		if let Some(rewritten) = rewrite_use_item_with_path(raw, &merged_path)
			&& rewritten != raw
		{
			edits.push(Edit { start, end, replacement: rewritten, rule: "RUST-STYLE-IMPORT-008" });
			touched_lines.insert(line);
			successful_merge_lines.insert(line);
		}
	}

	for pending in pending_import_paths {
		let Some(target) = merge_targets_by_pending.get(pending) else {
			remaining.insert(pending.clone());

			continue;
		};

		let can_consume = successful_merge_lines.contains(&target.anchor_line);

		if !can_consume {
			remaining.insert(pending.clone());
		}
	}

	(remaining, touched_lines)
}

fn find_exact_use_item_line_for_path(
	ctx: &FileContext,
	use_runs: &[Vec<&TopItem>],
	target_path: &str,
) -> Option<usize> {
	let target_compact = target_path.replace(' ', "");

	for item in use_runs.iter().flat_map(|run| run.iter().copied()) {
		let Some(path) = extract_use_path(ctx, item) else {
			continue;
		};

		if path.replace(' ', "") == target_compact {
			return Some(item.line);
		}
	}

	None
}

fn import008_insert_line(ctx: &FileContext) -> usize {
	if ctx.top_items.is_empty() {
		return 1;
	}

	let first_item_line = ctx.top_items[0].start_line;
	let mut after_leading_mods = first_item_line;
	let mut saw_leading_mod = false;

	for item in &ctx.top_items {
		if item.kind == TopKind::Mod && !is_cfg_test_attrs(&item.attrs) {
			after_leading_mods = item.end_line + 1;
			saw_leading_mod = true;
		} else {
			break;
		}
	}

	if saw_leading_mod { after_leading_mods } else { first_item_line }
}

fn is_cfg_test_attrs(attrs: &[String]) -> bool {
	attrs.iter().any(|attr| attr.replace(' ', "").contains("#[cfg(test)]"))
}

fn extract_use_path_from_line(line: &str) -> Option<String> {
	USE_RE
		.captures(line)
		.and_then(|caps| caps.get(2).map(|capture| capture.as_str().trim().to_owned()))
}

fn extract_use_path_from_text(text: &str) -> Option<String> {
	find_use_path_range(text)
		.and_then(|(start, end)| text.get(start..end).map(|s| s.trim().to_owned()))
}

fn find_use_path_range(text: &str) -> Option<(usize, usize)> {
	for (idx, _) in text.match_indices("use") {
		let prev = text[..idx].chars().next_back();
		let next = text.get(idx + 3..).and_then(|tail| tail.chars().next());
		let is_prev_boundary = prev.is_none_or(|ch| !(ch.is_ascii_alphanumeric() || ch == '_'));
		let is_next_whitespace = next.is_some_and(char::is_whitespace);

		if !is_prev_boundary || !is_next_whitespace {
			continue;
		}

		let mut start = idx + 3;
		let bytes = text.as_bytes();

		while start < bytes.len() && bytes[start].is_ascii_whitespace() {
			start += 1;
		}

		let tail = text.get(start..)?;
		let semi = tail.find(';')?;

		return Some((start, start + semi));
	}

	None
}

fn extract_use_path(ctx: &FileContext, item: &TopItem) -> Option<String> {
	extract_use_path_from_text(&item.raw).or_else(|| {
		ctx.lines.get(item.line.saturating_sub(1)).and_then(|line| extract_use_path_from_line(line))
	})
}

fn imported_symbols_from_use_path(path: &str) -> Vec<String> {
	let compact = path.replace(' ', "");

	if compact.ends_with("::*") {
		return Vec::new();
	}

	fn normalize_symbol(segment: &str) -> Option<String> {
		let mut symbol = segment.trim().to_owned();

		if symbol.is_empty() {
			return None;
		}

		if let Some((left, _)) = symbol.split_once(" as ") {
			symbol = left.trim().to_owned();
		}

		if matches!(symbol.as_str(), "*" | "self" | "super" | "crate") {
			return None;
		}

		if let Some((_, right)) = symbol.rsplit_once("::") {
			symbol = right.to_owned();
		}
		if let Some(stripped) = symbol.strip_prefix("r#") {
			symbol = stripped.to_owned();
		}

		if matches!(symbol.as_str(), "*" | "self" | "super" | "crate") {
			return None;
		}

		if symbol.is_empty() { None } else { Some(symbol) }
	}

	fn collect_symbols_from_segment(
		segment: &str,
		out: &mut Vec<String>,
		normalize_symbol: &impl Fn(&str) -> Option<String>,
	) -> bool {
		let trimmed = segment.trim();

		if trimmed.is_empty() {
			return true;
		}

		let mut brace_start = None;
		let mut depth = 0_i32;
		let mut brace_end = None;

		for (idx, ch) in trimmed.char_indices() {
			if ch == '{' {
				if brace_start.is_none() {
					brace_start = Some(idx);
				}

				depth += 1;
			} else if ch == '}' {
				depth -= 1;

				if depth < 0 {
					return false;
				}
				if depth == 0 {
					brace_end = Some(idx);
				}
			}
		}

		if depth != 0 {
			return false;
		}

		if let (Some(open), Some(close)) = (brace_start, brace_end) {
			let prefix = trimmed[..open].trim();
			let inner = &trimmed[open + 1..close];
			let suffix = trimmed[close + 1..].trim();

			if !suffix.is_empty() {
				return false;
			}
			if !prefix.is_empty() && !prefix.ends_with("::") {
				return false;
			}

			for child in split_top_level_csv(inner) {
				let expanded = if prefix.is_empty() { child } else { format!("{prefix}{child}") };

				if !collect_symbols_from_segment(&expanded, out, normalize_symbol) {
					return false;
				}
			}

			return true;
		}
		if let Some(symbol) = normalize_symbol(trimmed) {
			out.push(symbol);
		}

		true
	}

	let mut out = Vec::new();

	if !collect_symbols_from_segment(path, &mut out, &normalize_symbol) {
		return Vec::new();
	}

	out
}

fn normalize_ident(name: &str) -> &str {
	name.strip_prefix("r#").unwrap_or(name)
}

fn is_same_ident(lhs: &str, rhs: &str) -> bool {
	normalize_ident(lhs) == normalize_ident(rhs)
}

fn is_non_importable_root(root: &str) -> bool {
	if matches!(root, "Self" | "self" | "super" | "crate") {
		return root == "Self";
	}

	root.chars().next().is_some_and(char::is_uppercase)
}

fn is_non_importable_use_root(root: &str) -> bool {
	root == "Self" || (root.len() == 1 && root.chars().next().is_some_and(char::is_uppercase))
}

fn is_high_risk_shadow_symbol(symbol: &str) -> bool {
	matches!(symbol, "Result" | "Error")
}

fn glob_import_root(path: &str) -> Option<String> {
	let compact = path.replace(' ', "");

	if !compact.ends_with("::*") {
		return None;
	}

	let prefix = compact.trim_end_matches("::*");
	let root = prefix.split("::").next().map(normalize_ident)?.trim();

	if root.is_empty() { None } else { Some(root.to_owned()) }
}

fn import_root(path: &str) -> Option<String> {
	let root = path.split("::").next().map(normalize_ident)?.trim();

	if root.is_empty() { None } else { Some(root.to_owned()) }
}

fn has_high_risk_glob_shadow_ambiguity(
	symbol: &str,
	import_path: &str,
	glob_roots: &HashSet<String>,
) -> bool {
	if !is_high_risk_shadow_symbol(symbol) || glob_roots.is_empty() {
		return false;
	}

	let Some(candidate_root) = import_root(import_path) else {
		return false;
	};

	glob_roots.iter().any(|root| root != &candidate_root)
}

fn simple_import_prefix_symbol(path: &str) -> Option<(String, String)> {
	if path.contains('{') || path.contains('}') || path.contains('*') || path.contains(" as ") {
		return None;
	}

	let compact = path.replace(' ', "");
	let mut parts = compact.split("::").collect::<Vec<_>>();

	if parts.len() < 2 {
		return None;
	}

	let symbol = parts.pop()?.to_owned();
	let prefix = parts.join("::");

	if prefix.is_empty() || symbol.is_empty() {
		return None;
	}

	Some((prefix, symbol))
}

fn braced_import_fix_plan(path: &str, symbol: &str) -> Option<(String, Option<String>)> {
	let (open, close) = top_level_brace_range(path)?;

	if !path[close + 1..].trim().is_empty() {
		return None;
	}

	let prefix = braced_import_prefix(path, open)?;
	let inside = &path[open + 1..close];
	let parsed_segments = parse_braced_import_segments(inside)?;
	let (qualified_symbol_path, kept) =
		extract_braced_import_target_and_kept(&prefix, &parsed_segments, symbol)?;
	let module_path = prefix.trim_end_matches("::");
	let use_module_alias = module_alias_from_parent_path(module_path);
	let qualified_symbol_path = apply_parent_alias_to_qualified_path(
		module_path,
		use_module_alias.as_deref(),
		qualified_symbol_path,
	);

	build_braced_import_rewritten_path(
		&prefix,
		module_path,
		use_module_alias.as_deref(),
		qualified_symbol_path,
		kept,
	)
}

fn top_level_brace_range(text: &str) -> Option<(usize, usize)> {
	let open = text.find('{')?;
	let mut depth = 0_i32;
	let mut close = None;

	for (idx, ch) in text.char_indices().skip(open) {
		if ch == '{' {
			depth += 1;
		} else if ch == '}' {
			depth -= 1;

			if depth == 0 {
				close = Some(idx);

				break;
			}
		}
	}

	if depth != 0 {
		return None;
	}

	Some((open, close?))
}

fn braced_import_prefix(path: &str, open: usize) -> Option<String> {
	let mut prefix = path[..open].trim().to_owned();

	if prefix.is_empty() {
		return None;
	}
	if !prefix.ends_with("::") {
		prefix.push_str("::");
	}

	Some(prefix)
}

fn parse_braced_import_segments(inside: &str) -> Option<Vec<BracedImportSegment>> {
	let segments = split_top_level_csv(inside);

	if segments.is_empty() {
		return None;
	}

	let mut out = Vec::new();

	for segment in segments {
		let trimmed = segment.trim();

		if trimmed.is_empty() {
			continue;
		}
		if trimmed.contains(" as ") || trimmed == "*" {
			return None;
		}

		if let Some((head, rest)) = trimmed.split_once("::{") {
			let nested = parse_nested_braced_segment(head, rest)?;

			out.push(nested);

			continue;
		}

		if trimmed.contains('{') || trimmed.contains('}') {
			return None;
		}

		out.push(BracedImportSegment::Simple(trimmed.to_owned()));
	}

	if out.is_empty() { None } else { Some(out) }
}

fn parse_nested_braced_segment(head: &str, rest: &str) -> Option<BracedImportSegment> {
	if !rest.ends_with('}') {
		return None;
	}

	let nested_inside = &rest[..rest.len().saturating_sub(1)];
	let nested_children = split_top_level_csv(nested_inside);

	if nested_children.is_empty() {
		return None;
	}
	if nested_children.iter().any(|child| {
		let child = child.trim();

		child.is_empty()
			|| child == "*"
			|| child.contains(" as ")
			|| child.contains('{')
			|| child.contains('}')
	}) {
		return None;
	}

	Some(BracedImportSegment::Nested {
		head: head.trim().to_owned(),
		children: nested_children.into_iter().map(|child| child.trim().to_owned()).collect(),
	})
}

fn extract_braced_import_target_and_kept(
	prefix: &str,
	segments: &[BracedImportSegment],
	symbol: &str,
) -> Option<(String, Vec<String>)> {
	let mut qualified_symbol_path = None::<String>;
	let mut kept = Vec::new();

	for segment in segments {
		match segment {
			BracedImportSegment::Simple(name) => {
				if qualified_symbol_path.is_none() && is_same_ident(name, symbol) {
					qualified_symbol_path = Some(format!("{prefix}{name}"));
				} else {
					kept.push(name.clone());
				}
			},
			BracedImportSegment::Nested { head, children } => {
				let mut child_kept = Vec::new();

				for child in children {
					if qualified_symbol_path.is_none() && is_same_ident(child, symbol) {
						qualified_symbol_path = Some(format!("{prefix}{head}::{child}"));
					} else {
						child_kept.push(child.clone());
					}
				}

				if !child_kept.is_empty() {
					kept.push(format!("{head}::{{{}}}", child_kept.join(", ")));
				}
			},
		}
	}

	Some((qualified_symbol_path?, kept))
}

fn apply_parent_alias_to_qualified_path(
	module_path: &str,
	use_module_alias: Option<&str>,
	qualified_symbol_path: String,
) -> String {
	let Some(alias) = use_module_alias else {
		return qualified_symbol_path;
	};

	if let Some(tail) =
		qualified_symbol_path.strip_prefix(module_path).and_then(|rest| rest.strip_prefix("::"))
	{
		format!("{alias}::{tail}")
	} else {
		qualified_symbol_path
	}
}

fn build_braced_import_rewritten_path(
	prefix: &str,
	module_path: &str,
	use_module_alias: Option<&str>,
	qualified_symbol_path: String,
	kept: Vec<String>,
) -> Option<(String, Option<String>)> {
	if kept.is_empty() {
		if use_module_alias.is_some() {
			return Some((qualified_symbol_path, Some(module_path.to_owned())));
		}

		return Some((qualified_symbol_path, None));
	}

	let rewritten_use_path = if use_module_alias.is_some() {
		let mut kept_no_self = kept;

		kept_no_self.retain(|segment| segment != "self");

		if kept_no_self.is_empty() {
			format!("{prefix}{{self}}")
		} else {
			format!("{prefix}{{self, {}}}", kept_no_self.join(", "))
		}
	} else {
		format!("{prefix}{{{}}}", kept.join(", "))
	};

	Some((qualified_symbol_path, Some(rewritten_use_path)))
}

fn parse_braced_path_parts(path: &str) -> Option<(String, usize, Vec<String>)> {
	let (open, close) = top_level_brace_range(path)?;

	if !path[close + 1..].trim().is_empty() {
		return None;
	}

	let prefix = path[..open + 1].to_owned();
	let inner = &path[open + 1..close];
	let segments = split_top_level_csv(inner);

	if segments.is_empty() || segments.iter().any(|segment| segment.contains(" as ")) {
		return None;
	}

	Some((prefix, close, segments))
}

fn import004_fix_plan(path: &str, symbol: &str) -> Option<(String, Option<String>)> {
	if let Some((prefix, imported_symbol)) = simple_import_prefix_symbol(path)
		&& is_same_ident(&imported_symbol, symbol)
	{
		if let Some(alias) = module_alias_from_parent_path(&prefix) {
			return Some((format!("{alias}::{imported_symbol}"), Some(prefix)));
		}

		return Some((format!("{prefix}::{imported_symbol}"), None));
	}

	braced_import_fix_plan(path, symbol)
}

fn module_alias_from_parent_path(path: &str) -> Option<String> {
	if !path.starts_with("super::") {
		return None;
	}

	path.rsplit("::").next().filter(|segment| !segment.is_empty()).map(str::to_owned)
}

fn unqualified_function_call_ranges(ctx: &FileContext, symbol: &str) -> Vec<(usize, usize)> {
	let mut ranges = Vec::new();

	for call_expr in ctx.source_file.syntax().descendants().filter_map(ast::CallExpr::cast) {
		let Some(expr) = call_expr.expr() else {
			continue;
		};
		let Some(path_expr) = ast::PathExpr::cast(expr.syntax().clone()) else {
			continue;
		};
		let Some(path) = path_expr.path() else {
			continue;
		};

		if path.qualifier().is_some() {
			continue;
		}

		let Some(seg) = path.segment() else {
			continue;
		};
		let Some(name_ref) = seg.name_ref() else {
			continue;
		};

		if !is_same_ident(name_ref.text().as_str(), symbol) {
			continue;
		}

		ranges.push((
			usize::from(path.syntax().text_range().start()),
			usize::from(path.syntax().text_range().end()),
		));
	}

	ranges
}

fn unqualified_macro_call_ranges(ctx: &FileContext, symbol: &str) -> Vec<(usize, usize)> {
	let mut ranges = Vec::new();

	for macro_call in ctx.source_file.syntax().descendants().filter_map(ast::MacroCall::cast) {
		let Some(path) = macro_call.path() else {
			continue;
		};

		if path.qualifier().is_some() {
			continue;
		}

		let Some(seg) = path.segment() else {
			continue;
		};
		let Some(name_ref) = seg.name_ref() else {
			continue;
		};

		if !is_same_ident(name_ref.text().as_str(), symbol) {
			continue;
		}

		ranges.push((
			usize::from(path.syntax().text_range().start()),
			usize::from(path.syntax().text_range().end()),
		));
	}

	ranges
}

fn use_origin(path: &str, local_module_roots: &HashSet<String>) -> usize {
	let trimmed = path.replace("pub ", "");
	let root = trimmed.trim_start_matches(':').split("::").next().unwrap_or_default();
	let normalized_root = normalize_ident(root);

	if matches!(root, "std" | "core" | "alloc") {
		0
	} else if matches!(root, "crate" | "self" | "super")
		|| local_module_roots.contains(normalized_root)
		|| WORKSPACE_IMPORT_ROOTS.contains(normalized_root)
		|| WORKSPACE_IMPORT_ROOTS.contains(&normalized_root.replace('-', "_"))
	{
		2
	} else {
		1
	}
}

fn collect_local_module_roots(ctx: &FileContext) -> HashSet<String> {
	ctx.top_items
		.iter()
		.filter(|item| item.kind == TopKind::Mod)
		.filter_map(|item| item.name.as_deref())
		.map(|name| normalize_ident(name).to_owned())
		.collect()
}

fn split_top_level_csv(text: &str) -> Vec<String> {
	let mut out = Vec::new();
	let mut start = 0_usize;
	let mut depth_brace = 0_i32;
	let mut depth_angle = 0_i32;
	let chars = text.char_indices().collect::<Vec<_>>();

	for (idx, ch) in &chars {
		match ch {
			'{' => depth_brace += 1,
			'}' => depth_brace = (depth_brace - 1).max(0),
			'<' => depth_angle += 1,
			'>' => depth_angle = (depth_angle - 1).max(0),
			',' if depth_brace == 0 && depth_angle == 0 => {
				let segment = text[start..*idx].trim();

				if !segment.is_empty() {
					out.push(segment.to_owned());
				}

				start = *idx + 1;
			},
			_ => {},
		}
	}

	let tail = text[start..].trim();

	if !tail.is_empty() {
		out.push(tail.to_owned());
	}

	out
}

fn normalize_mixed_self_child_use_path(path: &str) -> Option<String> {
	let (prefix, close, segments) = parse_braced_path_parts(path)?;
	let (groups, parsed_heads) = build_mixed_use_groups(&segments);
	let rewritten = rewrite_mixed_use_segments(&segments, &groups, &parsed_heads)?;
	let rewritten_path = format!("{prefix}{}{}", rewritten.join(", "), &path[close..=close]);

	if rewritten_path == path { None } else { Some(rewritten_path) }
}

fn build_mixed_use_groups(
	segments: &[String],
) -> (HashMap<String, MixedUseGroup>, Vec<Option<String>>) {
	let mut groups: HashMap<String, MixedUseGroup> = HashMap::new();
	let mut parsed_heads = Vec::with_capacity(segments.len());

	for (idx, segment) in segments.iter().enumerate() {
		let parsed_head = parse_mixed_use_segment_into_group(idx, segment, &mut groups);

		parsed_heads.push(parsed_head);
	}

	(groups, parsed_heads)
}

fn parse_mixed_use_segment_into_group(
	idx: usize,
	segment: &str,
	groups: &mut HashMap<String, MixedUseGroup>,
) -> Option<String> {
	if let Some((head, rest)) = segment.split_once("::") {
		let head = head.trim();

		if head.is_empty() || head.contains('{') || head.contains('}') {
			return None;
		}
		if rest.starts_with('{') && rest.ends_with('}') {
			return parse_mixed_nested_group(idx, head, rest, groups);
		}

		let child = rest.trim();

		if child.is_empty() || child.contains("::") {
			return None;
		}

		let group = groups.entry(head.to_owned()).or_default();

		group.indices.push(idx);
		group.children.push(child.to_owned());

		return Some(head.to_owned());
	}

	let head = segment.trim();

	if head.is_empty() || head.contains('{') || head.contains('}') {
		return None;
	}

	let group = groups.entry(head.to_owned()).or_default();

	group.indices.push(idx);

	group.has_self = true;

	Some(head.to_owned())
}

fn parse_mixed_nested_group(
	idx: usize,
	head: &str,
	rest: &str,
	groups: &mut HashMap<String, MixedUseGroup>,
) -> Option<String> {
	let inner = &rest[1..rest.len().saturating_sub(1)];
	let child_parts = split_top_level_csv(inner);
	let group = groups.entry(head.to_owned()).or_default();

	group.indices.push(idx);

	for child in child_parts {
		if child == "self" {
			group.has_self = true;
		} else {
			group.children.push(child);
		}
	}

	Some(head.to_owned())
}

fn rewrite_mixed_use_segments(
	segments: &[String],
	groups: &HashMap<String, MixedUseGroup>,
	parsed_heads: &[Option<String>],
) -> Option<Vec<String>> {
	let mut emit = vec![true; segments.len()];
	let mut merged = false;
	let mut rewritten = Vec::new();

	for (idx, segment) in segments.iter().enumerate() {
		if !emit[idx] {
			continue;
		}

		let Some(head) = parsed_heads.get(idx).cloned().flatten() else {
			rewritten.push(segment.to_owned());

			continue;
		};
		let Some(group) = groups.get(&head) else {
			rewritten.push(segment.to_owned());

			continue;
		};

		if !can_merge_mixed_group(group, idx) {
			rewritten.push(segment.to_owned());

			continue;
		}

		let children = dedup_mixed_group_children(group);
		let combined = format!("{head}::{{self, {}}}", children.join(", "));

		rewritten.push(combined);

		merged = true;

		for original_idx in group.indices.iter().skip(1) {
			emit[*original_idx] = false;
		}
	}

	if merged { Some(rewritten) } else { None }
}

fn can_merge_mixed_group(group: &MixedUseGroup, idx: usize) -> bool {
	group.has_self
		&& !group.children.is_empty()
		&& group.indices.len() > 1
		&& group.indices.first().copied() == Some(idx)
}

fn dedup_mixed_group_children(group: &MixedUseGroup) -> Vec<String> {
	let mut seen = HashSet::new();
	let mut children = Vec::new();

	for child in &group.children {
		if seen.insert(child.clone()) {
			children.push(child.clone());
		}
	}

	children
}

fn rewrite_use_item_with_path(raw: &str, new_path: &str) -> Option<String> {
	let (start, end) = find_use_path_range(raw)?;
	let mut out = String::new();

	out.push_str(&raw[..start]);
	out.push_str(new_path);
	out.push_str(&raw[end..]);

	Some(out)
}

fn collect_non_pub_use_runs(ctx: &FileContext) -> Vec<Vec<&TopItem>> {
	let mut runs = Vec::new();
	let mut current = Vec::new();

	for item in &ctx.top_items {
		if item.kind == TopKind::Use && !item.is_pub {
			current.push(item);

			continue;
		}
		if !current.is_empty() {
			runs.push(std::mem::take(&mut current));
		}
	}

	if !current.is_empty() {
		runs.push(current);
	}

	runs
}

fn separator_lines<'a>(ctx: &'a FileContext, prev: &TopItem, curr: &TopItem) -> &'a [String] {
	let start = prev.end_line;
	let end = curr.start_line.saturating_sub(1);

	if start >= end || end > ctx.lines.len() { &[] } else { &ctx.lines[start..end] }
}

fn item_text_range(ctx: &FileContext, item: &TopItem) -> Option<(usize, usize)> {
	let start = shared::offset_from_line(&ctx.line_starts, item.start_line)?;
	let end =
		shared::offset_from_line(&ctx.line_starts, item.end_line + 1).unwrap_or(ctx.text.len());

	if end < start { None } else { Some((start, end)) }
}

fn run_text_range(ctx: &FileContext, first: &TopItem, last: &TopItem) -> Option<(usize, usize)> {
	let start = shared::offset_from_line(&ctx.line_starts, first.start_line)?;
	let end =
		shared::offset_from_line(&ctx.line_starts, last.end_line + 1).unwrap_or(ctx.text.len());

	if end < start { None } else { Some((start, end)) }
}

fn build_import_group_fix_plans(
	ctx: &FileContext,
	use_runs: &[Vec<&TopItem>],
	skip_lines: &HashSet<usize>,
	local_module_roots: &HashSet<String>,
) -> (Vec<Edit>, HashSet<usize>) {
	let mut planned_edits = Vec::new();
	let mut fixable_lines = HashSet::new();

	for run in use_runs {
		if !is_use_run_rewrite_candidate(run, skip_lines) {
			continue;
		}
		if !use_run_has_blank_only_separators(ctx, run) {
			continue;
		}

		let Some(entries) = collect_use_run_entries(ctx, run, local_module_roots) else {
			continue;
		};
		let Some((run_start, run_end)) = run_text_range(ctx, run[0], run[run.len() - 1]) else {
			continue;
		};
		let Some(original) = ctx.text.get(run_start..run_end) else {
			continue;
		};
		let replacement = build_use_run_replacement(original, &entries);

		if replacement == original {
			continue;
		}

		for entry in entries {
			fixable_lines.insert(entry.item.line);
		}

		planned_edits.push(Edit {
			start: run_start,
			end: run_end,
			replacement,
			rule: "RUST-STYLE-IMPORT-002",
		});
	}

	(planned_edits, fixable_lines)
}

fn is_use_run_rewrite_candidate(run: &[&TopItem], skip_lines: &HashSet<usize>) -> bool {
	run.len() >= 2 && !run.iter().any(|item| skip_lines.contains(&item.line))
}

fn use_run_has_blank_only_separators(ctx: &FileContext, run: &[&TopItem]) -> bool {
	run.windows(2).all(|pair| {
		separator_lines(ctx, pair[0], pair[1]).iter().all(|line| line.trim().is_empty())
	})
}

fn collect_use_run_entries<'a>(
	ctx: &'a FileContext,
	run: &'a [&'a TopItem],
	local_module_roots: &HashSet<String>,
) -> Option<Vec<UseEntry<'a>>> {
	let mut entries = Vec::with_capacity(run.len());

	for (order, item) in run.iter().enumerate() {
		let path = extract_use_path(ctx, item)?;
		let (start, end) = item_text_range(ctx, item)?;
		let block = ctx.text.get(start..end)?;
		let normalized_block = normalize_use_item_block(block)?;

		entries.push(UseEntry {
			item,
			origin: use_origin(&path, local_module_roots),
			order,
			block: normalized_block,
		});
	}

	Some(entries)
}

fn normalize_use_item_block(block: &str) -> Option<String> {
	let mut block_lines = Vec::new();
	let mut has_item_start = false;

	for line in block.lines() {
		let trimmed = line.trim();

		if !has_item_start {
			if trimmed.is_empty() {
				continue;
			}
			if line.trim_start().starts_with("//") {
				return None;
			}

			has_item_start = true;
		}

		block_lines.push(line);
	}

	if block_lines.is_empty() {
		return None;
	}

	let mut normalized_block = block_lines.join("\n");

	if block.ends_with('\n') {
		normalized_block.push('\n');
	}

	Some(normalized_block)
}

fn build_use_run_replacement(original: &str, entries: &[UseEntry<'_>]) -> String {
	let mut ordered_entries = entries.iter().collect::<Vec<_>>();

	ordered_entries.sort_by_key(|entry| (entry.origin, entry.order));

	let mut replacement = String::new();

	for (idx, entry) in ordered_entries.iter().enumerate() {
		if idx > 0 {
			let prev_origin = ordered_entries[idx - 1].origin;

			if entry.origin == prev_origin {
				replacement.push('\n');
			} else {
				replacement.push_str("\n\n");
			}
		}

		replacement.push_str(entry.block.trim_end_matches('\n'));
	}

	if original.ends_with('\n') {
		replacement.push('\n');
	}

	replacement
}
