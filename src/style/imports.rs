use std::{
	collections::{BTreeMap, BTreeSet, HashMap, HashSet},
	fs,
	path::PathBuf,
};

use ra_ap_syntax::{
	AstNode, Edition, SyntaxNode,
	ast::{self, HasAttrs, HasName, HasVisibility, Item, Module, Use},
};
use regex::Regex;

use crate::style::shared::{
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
	qualified_value_paths_by_symbol: &'a HashMap<String, HashSet<String>>,
}

#[derive(Debug, Clone)]
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
	let use_runs = collect_non_pub_use_runs(ctx);
	let use_items = use_runs.iter().flat_map(|run| run.iter().copied()).collect::<Vec<_>>();
	let import010_fixed_lines =
		apply_import010_no_super_use_rule(ctx, violations, edits, emit_edits, &use_items);
	let import007_fixed_lines =
		apply_import007_no_glob_use_rule(ctx, violations, edits, emit_edits, &use_items);

	if ctx.path.file_name().is_some_and(|name| name == "error.rs") {
		return;
	}

	let local_module_roots = collect_local_module_roots(ctx);
	let mut use_item_skip_lines = HashSet::new();

	use_item_skip_lines.extend(import010_fixed_lines.iter().copied());
	use_item_skip_lines.extend(import007_fixed_lines.iter().copied());

	let import004_fixed_lines =
		apply_use_item_rules(ctx, violations, edits, emit_edits, &use_items, &use_item_skip_lines);
	let imported_symbol_maps = collect_imported_symbol_maps(ctx, &use_items);

	push_import004_ambiguous_symbol_violations(ctx, violations, &imported_symbol_maps);

	let local_defined_symbols = collect_local_defined_symbols(ctx);
	let qualified_type_paths_by_symbol = collect_qualified_type_paths_by_symbol(ctx);
	let qualified_value_paths_by_symbol = collect_qualified_value_paths_by_symbol(ctx);
	let import009_ctx = Import009Context {
		use_items: &use_items,
		maps: &imported_symbol_maps,
		local_defined_symbols: &local_defined_symbols,
		qualified_type_paths_by_symbol: &qualified_type_paths_by_symbol,
		qualified_value_paths_by_symbol: &qualified_value_paths_by_symbol,
	};
	let import009_fixed_lines = apply_import009_rules(
		ctx,
		violations,
		edits,
		emit_edits,
		&import009_ctx,
		&use_item_skip_lines,
	);
	let import008_group_skip_lines = apply_import008_rules(
		ctx,
		violations,
		edits,
		emit_edits,
		&use_runs,
		&local_module_roots,
		&local_defined_symbols,
		&imported_symbol_maps.full_paths_by_symbol,
	);
	let mut import_group_skip_lines = HashSet::new();

	import_group_skip_lines.extend(import010_fixed_lines);
	import_group_skip_lines.extend(import007_fixed_lines);
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

fn apply_import010_no_super_use_rule(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	use_items: &[&TopItem],
) -> HashSet<usize> {
	let mut fixed_top_level_lines = HashSet::new();
	let top_use_lines = use_items.iter().map(|item| item.line).collect::<HashSet<_>>();

	for use_item in ctx.source_file.syntax().descendants().filter_map(Use::cast) {
		let Some(use_tree) = use_item.use_tree() else {
			continue;
		};
		let use_path = compact_path_for_match(&use_tree.syntax().text().to_string());
		let Some((super_depth, tail)) = leading_super_depth_and_tail(&use_path) else {
			continue;
		};
		let current_module_path = current_module_path_segments(ctx, &use_item);
		let fixable = super_depth <= current_module_path.len();
		let start = usize::from(use_item.syntax().text_range().start());
		let end = usize::from(use_item.syntax().text_range().end());
		let line = shared::line_from_offset(&ctx.line_starts, start);

		shared::push_violation(
			violations,
			ctx,
			line,
			"RUST-STYLE-IMPORT-010",
			"Do not use `super` imports; use crate-absolute imports.",
			fixable,
		);

		if !emit_edits || !fixable {
			continue;
		}

		let parent_depth = current_module_path.len() - super_depth;
		let replacement_path = crate_absolute_use_path(&current_module_path[..parent_depth], tail);
		let replacement =
			rewrite_use_item_with_path(&use_item.syntax().text().to_string(), &replacement_path);
		let Some(replacement) = replacement else {
			continue;
		};

		edits.push(Edit { start, end, replacement, rule: "RUST-STYLE-IMPORT-010" });

		if top_use_lines.contains(&line) {
			fixed_top_level_lines.insert(line);
		}
	}

	fixed_top_level_lines
}

fn apply_import007_no_glob_use_rule(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	use_items: &[&TopItem],
) -> HashSet<usize> {
	let mut fixed_top_level_lines = HashSet::new();
	let top_use_lines = use_items.iter().map(|item| item.line).collect::<HashSet<_>>();

	for use_item in ctx.source_file.syntax().descendants().filter_map(Use::cast) {
		let Some(use_tree) = use_item.use_tree() else {
			continue;
		};
		let use_path = use_tree.syntax().text().to_string();

		if !use_path.contains('*') {
			continue;
		}

		let start = usize::from(use_item.syntax().text_range().start());
		let end = usize::from(use_item.syntax().text_range().end());
		let line = shared::line_from_offset(&ctx.line_starts, start);
		let replacement = build_glob_use_replacement(ctx, &use_item, &use_path);
		let fixable = replacement.is_some();

		shared::push_violation(
			violations,
			ctx,
			line,
			"RUST-STYLE-IMPORT-007",
			"Glob imports are not allowed; import explicit symbols.",
			fixable,
		);

		if !emit_edits {
			continue;
		}

		if let Some(replacement) = replacement {
			edits.push(Edit { start, end, replacement, rule: "RUST-STYLE-IMPORT-007" });

			if top_use_lines.contains(&line) {
				fixed_top_level_lines.insert(line);
			}
		}
	}

	fixed_top_level_lines
}

fn build_glob_use_replacement(ctx: &FileContext, use_item: &Use, use_path: &str) -> Option<String> {
	let compact = compact_path_for_match(use_path);
	let start = usize::from(use_item.syntax().text_range().start());
	let line = shared::line_from_offset(&ctx.line_starts, start);
	let line_start = shared::offset_from_line(&ctx.line_starts, line).unwrap_or(start);
	let indent = ctx
		.text
		.get(line_start..start)
		.unwrap_or_default()
		.chars()
		.take_while(|ch| ch.is_whitespace())
		.collect::<String>();
	let vis = use_item
		.visibility()
		.map(|visibility| format!("{} ", visibility.syntax().text()))
		.unwrap_or_default();

	if compact.contains("crate::{") && compact.contains("prelude::*") {
		let used_symbols = collect_used_symbols_from_syntax(ctx.source_file.syntax());
		let symbols = exported_symbols_from_crate_prelude(ctx)?
			.into_iter()
			.filter(|symbol| used_symbols.contains(symbol))
			.collect::<Vec<_>>();

		if symbols.is_empty() {
			return None;
		}

		let replacement =
			compact.replace("prelude::*", &format!("prelude::{{{}}}", symbols.join(",")));

		return Some(format!("{indent}{vis}use {replacement};"));
	}

	let prefix = compact.strip_suffix("::*")?;

	if prefix == "rayon::prelude" {
		let traits = rayon_traits_for_usage(ctx);

		if traits.is_empty() {
			return None;
		}

		return Some(format!("{indent}{vis}use rayon::iter::{{{}}};", traits.join(", ")));
	}

	let symbols = exported_symbols_for_glob_prefix(ctx, use_item, prefix)?;

	if symbols.is_empty() {
		return None;
	}

	let used_symbols = collect_used_symbols_from_syntax(ctx.source_file.syntax());
	let names =
		symbols.into_iter().filter(|symbol| used_symbols.contains(symbol)).collect::<Vec<_>>();

	if names.is_empty() {
		return None;
	}

	let names = names.join(", ");

	Some(format!("{indent}{vis}use {prefix}::{{{names}}};"))
}

fn rayon_traits_for_usage(ctx: &FileContext) -> Vec<&'static str> {
	let mut traits = Vec::new();

	if ctx.text.contains(".par_iter(") || ctx.text.contains(".par_iter()") {
		traits.push("IntoParallelRefIterator");
	}
	if ctx.text.contains(".par_iter_mut(") || ctx.text.contains(".par_iter_mut()") {
		traits.push("IntoParallelRefMutIterator");
	}
	if ctx.text.contains(".into_par_iter(") || ctx.text.contains(".into_par_iter()") {
		traits.push("IntoParallelIterator");
	}
	if !traits.is_empty() {
		traits.push("ParallelIterator");
	}

	traits.sort_unstable();
	traits.dedup();

	traits
}

fn exported_symbols_for_glob_prefix(
	ctx: &FileContext,
	use_item: &Use,
	prefix: &str,
) -> Option<BTreeSet<String>> {
	if prefix == "crate::prelude" {
		return exported_symbols_from_crate_prelude(ctx);
	}
	if prefix == "super" {
		return exported_symbols_from_super_scope(use_item);
	}

	None
}

fn exported_symbols_from_crate_prelude(ctx: &FileContext) -> Option<BTreeSet<String>> {
	let crate_dir = find_crate_dir(&ctx.path)?;
	let src_dir = crate_dir.join("src");
	let root_candidates = [src_dir.join("lib.rs"), src_dir.join("main.rs")];

	for root in root_candidates {
		if !root.is_file() {
			continue;
		}

		let symbols = exported_symbols_from_named_module(&root, "prelude");

		if !symbols.is_empty() {
			return Some(symbols);
		}
	}

	None
}

fn exported_symbols_from_named_module(
	root_file: &std::path::Path,
	module_name: &str,
) -> BTreeSet<String> {
	let Ok(text) = fs::read_to_string(root_file) else {
		return BTreeSet::new();
	};
	let parsed = ra_ap_syntax::SourceFile::parse(&text, Edition::CURRENT).tree();
	let Some(module) = parsed
		.syntax()
		.children()
		.filter_map(Module::cast)
		.find(|item| item.name().is_some_and(|name| name.text() == module_name))
	else {
		return BTreeSet::new();
	};

	if let Some(item_list) = module.item_list() {
		return exported_symbols_from_use_items(
			item_list.syntax().children().filter_map(Item::cast),
		);
	}

	if module.semicolon_token().is_some() {
		let module_rs = root_file
			.parent()
			.map(|parent| parent.join(format!("{module_name}.rs")))
			.unwrap_or_else(|| PathBuf::from(format!("{module_name}.rs")));
		let module_mod_rs = root_file
			.parent()
			.map(|parent| parent.join(module_name).join("mod.rs"))
			.unwrap_or_else(|| PathBuf::from(module_name).join("mod.rs"));

		for candidate in [module_rs, module_mod_rs] {
			if !candidate.is_file() {
				continue;
			}

			let Ok(module_text) = fs::read_to_string(&candidate) else {
				continue;
			};
			let module_parsed =
				ra_ap_syntax::SourceFile::parse(&module_text, Edition::CURRENT).tree();
			let exported = exported_symbols_from_use_items(
				module_parsed.syntax().children().filter_map(Item::cast),
			);

			if !exported.is_empty() {
				return exported;
			}
		}
	}

	BTreeSet::new()
}

fn exported_symbols_from_use_items(items: impl Iterator<Item = Item>) -> BTreeSet<String> {
	let mut symbols = BTreeSet::new();

	for item in items {
		let Item::Use(use_item) = item else {
			continue;
		};

		if use_item.visibility().is_none() {
			continue;
		}

		let Some(use_tree) = use_item.use_tree() else {
			continue;
		};
		let use_path = use_tree.syntax().text().to_string();

		for symbol in imported_symbols_from_use_path(&use_path) {
			symbols.insert(symbol);
		}
	}

	symbols
}

fn find_crate_dir(path: &std::path::Path) -> Option<PathBuf> {
	let mut dir = path.parent()?.to_path_buf();

	loop {
		if dir.join("Cargo.toml").is_file() {
			return Some(dir);
		}
		if !dir.pop() {
			break;
		}
	}

	None
}

fn current_module_path_segments(ctx: &FileContext, use_item: &Use) -> Vec<String> {
	let mut module_path = file_module_path_segments(&ctx.path);
	let mut inline_ancestors = use_item
		.syntax()
		.ancestors()
		.filter_map(Module::cast)
		.filter_map(|module| module.name().map(|name| name.text().to_string()))
		.collect::<Vec<_>>();

	inline_ancestors.reverse();
	module_path.extend(inline_ancestors);

	module_path
}

fn file_module_path_segments(path: &std::path::Path) -> Vec<String> {
	let Some(crate_dir) = find_crate_dir(path) else {
		return Vec::new();
	};
	let src_dir = crate_dir.join("src");
	let Ok(relative) = path.strip_prefix(src_dir) else {
		return Vec::new();
	};
	let mut components = relative
		.iter()
		.map(|component| component.to_string_lossy().to_string())
		.collect::<Vec<_>>();
	let Some(file_name) = components.pop() else {
		return Vec::new();
	};

	match file_name.as_str() {
		"lib.rs" | "main.rs" => Vec::new(),
		"mod.rs" => components,
		_ => {
			let stem = file_name.strip_suffix(".rs").unwrap_or(file_name.as_str());

			components.push(stem.to_owned());

			components
		},
	}
}

fn leading_super_depth_and_tail(path: &str) -> Option<(usize, &str)> {
	let mut depth = 0_usize;
	let mut rest = path;

	loop {
		if rest == "super" {
			depth += 1;
			rest = "";

			break;
		}

		let Some(after) = rest.strip_prefix("super::") else {
			break;
		};

		depth += 1;
		rest = after;

		if !rest.starts_with("super") {
			break;
		}
	}

	if depth == 0 { None } else { Some((depth, rest)) }
}

fn crate_absolute_use_path(parent_segments: &[String], tail: &str) -> String {
	let mut path = String::from("crate");

	for segment in parent_segments {
		path.push_str("::");
		path.push_str(segment);
	}

	if !tail.is_empty() {
		path.push_str("::");
		path.push_str(tail);
	}

	path
}

fn exported_symbols_from_super_scope(use_item: &Use) -> Option<BTreeSet<String>> {
	let current_module = use_item.syntax().ancestors().find_map(Module::cast)?;
	let current_module_item_list = current_module.item_list()?;
	let current_module_name = current_module.name().map(|name| name.text().to_string());
	let already_imported =
		imported_symbols_from_current_module_use_items(&current_module, use_item);
	let used_symbols = collect_used_symbols_from_syntax(current_module_item_list.syntax());
	let mut symbols = BTreeSet::new();

	if let Some(parent_module) = current_module.syntax().ancestors().skip(1).find_map(Module::cast)
	{
		if let Some(item_list) = parent_module.item_list() {
			collect_scope_symbols_from_items(
				item_list.syntax().children().filter_map(Item::cast),
				&mut symbols,
			);
		}
	} else if let Some(source_file) =
		current_module.syntax().ancestors().find_map(ast::SourceFile::cast)
	{
		collect_scope_symbols_from_items(
			source_file.syntax().children().filter_map(Item::cast),
			&mut symbols,
		);
	}

	if symbols.is_empty() {
		return None;
	}

	let used = symbols
		.into_iter()
		.filter(|symbol| current_module_name.as_deref() != Some(symbol.as_str()))
		.filter(|symbol| !matches!(symbol.as_str(), "tests" | "_test"))
		.filter(|symbol| !already_imported.contains(symbol))
		.filter(|symbol| used_symbols.contains(symbol))
		.collect::<BTreeSet<_>>();

	if used.is_empty() { None } else { Some(used) }
}

fn imported_symbols_from_current_module_use_items(
	current_module: &Module,
	current_use_item: &Use,
) -> HashSet<String> {
	let mut out = HashSet::new();
	let Some(item_list) = current_module.item_list() else {
		return out;
	};

	for item in item_list.syntax().children().filter_map(Item::cast) {
		let Item::Use(use_item) = item else {
			continue;
		};

		if use_item.syntax().text_range() == current_use_item.syntax().text_range() {
			continue;
		}

		let Some(use_tree) = use_item.use_tree() else {
			continue;
		};
		let use_path = use_tree.syntax().text().to_string();

		for symbol in imported_symbols_from_use_path(&use_path) {
			if symbol == "*" {
				continue;
			}

			out.insert(symbol);
		}
	}

	out
}

fn collect_scope_symbols_from_items(items: impl Iterator<Item = Item>, out: &mut BTreeSet<String>) {
	for item in items {
		match item {
			Item::Use(use_item) => {
				let Some(use_tree) = use_item.use_tree() else {
					continue;
				};
				let use_path = use_tree.syntax().text().to_string();

				for symbol in imported_symbols_from_use_path(&use_path) {
					out.insert(symbol);
				}
			},
			_ =>
				if let Some(name) = item_name_text(&item) {
					out.insert(name);
				},
		}
	}
}

fn item_name_text(item: &Item) -> Option<String> {
	match item {
		Item::Fn(it) => it.name().map(|n| n.text().to_string()),
		Item::Struct(it) => it.name().map(|n| n.text().to_string()),
		Item::Enum(it) => it.name().map(|n| n.text().to_string()),
		Item::Trait(it) => it.name().map(|n| n.text().to_string()),
		Item::TypeAlias(it) => it.name().map(|n| n.text().to_string()),
		Item::Const(it) => it.name().map(|n| n.text().to_string()),
		Item::Static(it) => it.name().map(|n| n.text().to_string()),
		Item::Module(it) => it.name().map(|n| n.text().to_string()),
		Item::Union(it) => it.name().map(|n| n.text().to_string()),
		Item::MacroRules(it) => it.name().map(|n| n.text().to_string()),
		_ => None,
	}
}

fn collect_used_symbols_from_syntax(syntax: &SyntaxNode) -> HashSet<String> {
	let mut used = HashSet::new();

	for name_ref in syntax.descendants().filter_map(ast::NameRef::cast) {
		let in_use_tree = name_ref.syntax().ancestors().any(|node| Use::can_cast(node.kind()));

		if in_use_tree {
			continue;
		}

		used.insert(name_ref.text().to_string());
	}

	used
}

fn apply_use_item_rules(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	use_items: &[&TopItem],
	skip_lines: &HashSet<usize>,
) -> HashSet<usize> {
	let mut import004_fixed_lines = HashSet::new();

	for item in use_items {
		if skip_lines.contains(&item.line) {
			continue;
		}

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

		if apply_import003_trait_keep_alive_rule(ctx, violations, edits, emit_edits, item, &path) {
			import004_fixed_lines.insert(item.line);
		}

		push_alias_violation_if_needed(ctx, violations, item, &path);

		if apply_import004_free_fn_macro_rule(ctx, violations, edits, emit_edits, item, &path) {
			import004_fixed_lines.insert(item.line);
		}
	}

	import004_fixed_lines
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

	let Some(edit) = build_use_item_rewrite_edit(
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

	let Some(edit) = build_use_item_rewrite_edit(ctx, item, None, "RUST-STYLE-IMPORT-009") else {
		return false;
	};

	edits.push(edit);

	true
}

fn apply_import002_normalization_rule(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	item: &TopItem,
	path: &str,
) {
	let Some(normalized) = normalize_mixed_self_child_use_path(ctx, path) else {
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
			"Import aliases are not allowed except `as _` keep-alive imports.",
			false,
		);
	}
}

fn apply_import003_trait_keep_alive_rule(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	item: &TopItem,
	path: &str,
) -> bool {
	let Some((rewritten_use_path, affected_symbols)) =
		normalize_trait_keep_alive_use_path(ctx, path)
	else {
		return false;
	};
	let mut symbols = affected_symbols.into_iter().collect::<Vec<_>>();

	symbols.sort();

	for symbol in symbols {
		let referenced_directly = symbol_is_referenced_outside_use(ctx, &symbol);
		let message = if referenced_directly {
			format!("Trait import `{symbol}` is referenced directly; do not use `as _`.")
		} else {
			format!(
				"Trait keep-alive import `{symbol}` should use `as _` when not referenced directly."
			)
		};

		shared::push_violation(violations, ctx, item.line, "RUST-STYLE-IMPORT-003", &message, true);
	}

	if !emit_edits || rewritten_use_path == path {
		return false;
	}

	if let Some((start, end)) = item_text_range(ctx, item)
		&& let Some(raw) = ctx.text.get(start..end)
		&& let Some(rewritten) = rewrite_use_item_with_path(raw, &rewritten_use_path)
	{
		edits.push(Edit { start, end, replacement: rewritten, rule: "RUST-STYLE-IMPORT-003" });

		return true;
	}

	false
}

fn looks_like_trait_import(symbol: &str, full_path: &str) -> bool {
	let symbol_is_common_trait = matches!(
		symbol,
		"Read"
			| "Write" | "BufRead"
			| "Seek" | "AsyncRead"
			| "AsyncWrite"
			| "AsyncBufRead"
			| "AsyncSeek"
			| "Future"
			| "Stream"
			| "Sink" | "Iterator"
			| "IntoIterator"
			| "FromIterator"
			| "ParallelIterator"
			| "IntoParallelIterator"
			| "IntoParallelRefIterator"
			| "Serialize"
			| "Deserialize"
			| "Executor"
	);
	let symbol_is_trait_named =
		symbol.ends_with("Ext") || symbol.ends_with("Trait") || symbol_is_common_trait;
	let path_is_trait_named = full_path.contains("::trait::")
		|| full_path.contains("::traits::")
		|| full_path.contains("::prelude::");

	(symbol_is_trait_named || path_is_trait_named)
		&& symbol.chars().next().is_some_and(char::is_uppercase)
}

fn symbol_is_referenced_outside_use(ctx: &FileContext, symbol: &str) -> bool {
	for path in ctx.source_file.syntax().descendants().filter_map(ast::Path::cast) {
		if path.syntax().ancestors().any(|node| Use::cast(node).is_some()) {
			continue;
		}

		let Some(segment) = path.segment() else {
			continue;
		};
		let Some(name_ref) = segment.name_ref() else {
			continue;
		};

		if is_same_ident(name_ref.text().as_str(), symbol) {
			return true;
		}
	}

	let derive_symbol_re = Regex::new(&format!(r"\b{}\b", regex::escape(symbol)))
		.expect("Expected operation to succeed.");

	for attr in ctx.source_file.syntax().descendants().filter_map(ast::Attr::cast) {
		let text = attr.syntax().text().to_string();

		if !text.contains("derive") {
			continue;
		}
		if derive_symbol_re.is_match(&text) {
			return true;
		}
	}

	false
}

fn normalize_trait_keep_alive_use_path(
	ctx: &FileContext,
	path: &str,
) -> Option<(String, HashSet<String>)> {
	let has_child_module_declarations = has_non_inline_child_modules(ctx);

	if path.contains('*') {
		return None;
	}

	if let Some((rewritten_leaf, affected_symbols, changed, _trait_key)) =
		normalize_trait_keep_alive_leaf(path.trim(), ctx, "", has_child_module_declarations)
		&& changed
	{
		return Some((rewritten_leaf, affected_symbols));
	}

	let (prefix, close, segments) = parse_braced_path_parts_allow_alias(path)?;
	let module_prefix = prefix[..prefix.len().saturating_sub(1)]
		.trim()
		.strip_suffix("::")
		.unwrap_or(prefix[..prefix.len().saturating_sub(1)].trim())
		.to_owned();
	let (rewritten_segments, changed, affected_symbols) = normalize_trait_keep_alive_segments(
		segments,
		ctx,
		&module_prefix,
		has_child_module_declarations,
	);

	if !changed {
		return None;
	}

	Some((
		format!("{prefix}{}{}", rewritten_segments.join(", "), &path[close..=close]),
		affected_symbols,
	))
}

fn normalize_trait_keep_alive_segments(
	segments: Vec<String>,
	ctx: &FileContext,
	module_prefix: &str,
	has_child_module_declarations: bool,
) -> (Vec<String>, bool, HashSet<String>) {
	let mut rewritten_segments = Vec::new();
	let mut state = TraitKeepAliveNormalizationState::default();

	for segment in segments {
		let trimmed = segment.trim();

		if trimmed.is_empty() {
			continue;
		}

		if let Some((head, inner)) = parse_single_level_nested_use_segment(trimmed) {
			if let Some(rewritten_segment) = normalize_trait_keep_alive_nested_segment(
				trimmed,
				head,
				inner,
				ctx,
				module_prefix,
				has_child_module_declarations,
				&mut state,
			) {
				rewritten_segments.push(rewritten_segment);
			}

			continue;
		}
		if let Some(rewritten_segment) = normalize_trait_keep_alive_leaf_segment(
			trimmed,
			ctx,
			module_prefix,
			has_child_module_declarations,
			&mut state,
		) {
			rewritten_segments.push(rewritten_segment);
		}
	}

	(rewritten_segments, state.changed, state.affected_symbols)
}

fn normalize_trait_keep_alive_nested_segment(
	original_segment: &str,
	head: &str,
	inner: &str,
	ctx: &FileContext,
	module_prefix: &str,
	has_child_module_declarations: bool,
	state: &mut TraitKeepAliveNormalizationState,
) -> Option<String> {
	let nested_prefix =
		if module_prefix.is_empty() { head.to_owned() } else { format!("{module_prefix}::{head}") };
	let mut rewritten_children = Vec::new();
	let mut changed = false;

	for child in split_top_level_csv(inner) {
		let child_trimmed = child.trim();

		if child_trimmed.is_empty() {
			continue;
		}

		if let Some((rewritten_child, child_symbols, child_changed, trait_key)) =
			normalize_trait_keep_alive_leaf(
				child_trimmed,
				ctx,
				&nested_prefix,
				has_child_module_declarations,
			) {
			for symbol in child_symbols {
				state.affected_symbols.insert(symbol);
			}

			if child_changed {
				changed = true;
			}

			if let Some(key) = trait_key {
				if state.seen_trait_keys.insert(key) {
					rewritten_children.push(rewritten_child);
				} else {
					changed = true;
				}
			} else {
				rewritten_children.push(rewritten_child);
			}
		} else {
			rewritten_children.push(child_trimmed.to_owned());
		}
	}

	if rewritten_children.is_empty() {
		state.changed = true;

		return None;
	}
	if !changed {
		return Some(original_segment.to_owned());
	}

	state.changed = true;

	let rewritten_segment = format!("{head}::{{{}}}", rewritten_children.join(", "));

	Some(rewritten_segment)
}

fn normalize_trait_keep_alive_leaf_segment(
	trimmed: &str,
	ctx: &FileContext,
	module_prefix: &str,
	has_child_module_declarations: bool,
	state: &mut TraitKeepAliveNormalizationState,
) -> Option<String> {
	let Some((rewritten_segment, segment_symbols, segment_changed, trait_key)) =
		normalize_trait_keep_alive_leaf(trimmed, ctx, module_prefix, has_child_module_declarations)
	else {
		return Some(trimmed.to_owned());
	};

	for symbol in segment_symbols {
		state.affected_symbols.insert(symbol);
	}

	if segment_changed {
		state.changed = true;
	}

	if let Some(key) = trait_key {
		if state.seen_trait_keys.insert(key) {
			return Some(rewritten_segment);
		}

		state.changed = true;

		return None;
	}

	Some(rewritten_segment)
}

fn normalize_trait_keep_alive_leaf(
	leaf: &str,
	ctx: &FileContext,
	import_prefix: &str,
	has_child_module_declarations: bool,
) -> Option<(String, HashSet<String>, bool, Option<String>)> {
	if leaf.is_empty() || leaf == "self" || leaf.contains('{') || leaf.contains('}') {
		return None;
	}

	let (base, alias) = split_import_leaf_alias(leaf)?;
	let alias_trimmed = alias.as_deref().map(str::trim);

	if alias_trimmed.is_some_and(|alias| alias != "_") {
		return None;
	}

	let full_path =
		if import_prefix.is_empty() { base.to_owned() } else { format!("{import_prefix}::{base}") };
	let full_path = full_path.replace(' ', "");
	let symbol = symbol_from_full_import_path(&full_path)?;

	if !looks_like_trait_import(&symbol, &full_path) {
		return None;
	}

	let should_keep_alive =
		!symbol_is_referenced_outside_use(ctx, &symbol) && !has_child_module_declarations;
	let rewritten = if should_keep_alive { format!("{base} as _") } else { base.to_owned() };
	let changed = compact_path_for_match(leaf) != compact_path_for_match(&rewritten);
	let mut symbols = HashSet::new();

	symbols.insert(symbol.clone());

	Some((
		rewritten,
		symbols,
		changed,
		Some(format!(
			"{}|{}",
			compact_path_for_match(&full_path),
			if should_keep_alive { "_" } else { "" }
		)),
	))
}

fn split_import_leaf_alias(leaf: &str) -> Option<(&str, Option<String>)> {
	let trimmed = leaf.trim();

	if trimmed.is_empty() {
		return None;
	}

	if let Some((left, right)) = trimmed.rsplit_once(" as ") {
		let base = left.trim();
		let alias = right.trim();

		if base.is_empty() || alias.is_empty() {
			return None;
		}

		return Some((base, Some(alias.to_owned())));
	}

	Some((trimmed, None))
}

fn has_non_inline_child_modules(ctx: &FileContext) -> bool {
	ctx.top_items.iter().any(|item| item.kind == TopKind::Mod && item.raw.trim_end().ends_with(';'))
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

	let mut fixed = false;

	for symbol in imported_symbols_from_use_path(path) {
		if symbol.is_empty() || !symbol.chars().next().is_some_and(char::is_lowercase) {
			continue;
		}

		let local_fn_defined = is_local_fn_defined(ctx, &symbol);
		let local_macro_defined = is_local_macro_defined(ctx, &symbol);
		let fn_ranges = unqualified_function_call_ranges(ctx, &symbol);
		let macro_ranges = unqualified_macro_call_ranges(ctx, &symbol);
		let needs_fn_fix = !fn_ranges.is_empty() && !local_fn_defined;
		let needs_macro_fix = !macro_ranges.is_empty()
			&& !local_macro_defined
			&& !symbol_imported_from_std_like_root(path, &symbol);

		if !(needs_fn_fix || needs_macro_fix) {
			continue;
		}

		let mut fixable = false;
		let mut qualified_symbol_path = String::new();
		let mut use_item_edit = None;

		if let Some((qualified_path, rewritten_use_path)) = import004_fix_plan(path, &symbol) {
			qualified_symbol_path = qualified_path;
			fixable = true;

			if emit_edits {
				use_item_edit = build_use_item_rewrite_edit(
					ctx,
					item,
					rewritten_use_path.as_deref(),
					"RUST-STYLE-IMPORT-004",
				);

				if use_item_edit.is_none() {
					fixable = false;
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

			fixed = true;
		}

		break;
	}

	fixed
}

fn symbol_imported_from_std_like_root(path: &str, symbol: &str) -> bool {
	imported_full_paths_from_use_path(path).into_iter().any(|full_path| {
		if symbol_from_full_import_path(&full_path).as_deref() != Some(symbol) {
			return false;
		}

		matches!(full_path.split("::").next(), Some("std") | Some("core") | Some("alloc"))
	})
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
	skip_lines: &HashSet<usize>,
) -> HashSet<usize> {
	let mut import009_fixed_lines = HashSet::new();
	let mut locked_use_lines = HashSet::new();

	for (symbol, imported_paths) in &import009_ctx.maps.full_paths_by_symbol {
		if import009_ctx
			.maps
			.symbol_lines
			.get(symbol)
			.is_some_and(|lines| lines.iter().any(|line| skip_lines.contains(line)))
		{
			continue;
		}
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
			import009_ctx.qualified_value_paths_by_symbol,
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
		if use_item_plans.iter().any(|(item, _, _)| locked_use_lines.contains(&item.line)) {
			continue;
		}

		let mut planned_use_item_edits = Vec::new();

		for (item, _qualified_symbol_path, rewritten_use_path) in &use_item_plans {
			let Some(edit) = build_use_item_rewrite_edit(
				ctx,
				item,
				rewritten_use_path.as_deref(),
				"RUST-STYLE-IMPORT-009",
			) else {
				planned_use_item_edits.clear();

				break;
			};

			planned_use_item_edits.push((item.line, edit));
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
		for (line, edit) in planned_use_item_edits {
			edits.push(edit);
			import009_fixed_lines.insert(line);
			locked_use_lines.insert(line);
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
	qualified_value_paths_by_symbol: &HashMap<String, HashSet<String>>,
) -> Option<Import009Plan<'a>> {
	let type_rewrites = unqualified_type_path_rewrites(ctx, symbol, imported_path);
	let value_rewrites = unqualified_value_path_rewrites(ctx, symbol, imported_path);
	let has_unqualified_uses = !type_rewrites.is_empty() || !value_rewrites.is_empty();
	let has_any_qualified_path = qualified_type_paths_by_symbol.contains_key(symbol)
		|| qualified_value_paths_by_symbol.contains_key(symbol);
	let has_qualified_same_path = qualified_type_paths_by_symbol
		.get(symbol)
		.is_some_and(|paths| paths.contains(imported_path))
		|| qualified_value_paths_by_symbol
			.get(symbol)
			.is_some_and(|paths| paths.contains(imported_path));

	if has_unqualified_uses {
		if !has_any_qualified_path {
			return None;
		}
	} else if !has_qualified_same_path {
		// Avoid "unused import" style deletion in this rule, because imports can have effects
		// (for example, trait method resolution) even when the symbol name is not referenced.
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

fn build_use_item_rewrite_edit(
	ctx: &FileContext,
	item: &TopItem,
	rewritten_use_path: Option<&str>,
	rule: &'static str,
) -> Option<Edit> {
	if let Some(new_use_path) = rewritten_use_path {
		if let Some((start, end)) = item_text_range(ctx, item)
			&& let Some(raw) = ctx.text.get(start..end)
			&& let Some(rewritten) = rewrite_use_item_with_path(raw, new_use_path)
		{
			return Some(Edit { start, end, replacement: rewritten, rule });
		}

		return None;
	}
	if let (Some(start), Some(next)) = (
		shared::offset_from_line(&ctx.line_starts, item.start_line),
		shared::offset_from_line(&ctx.line_starts, item.end_line + 1),
	) {
		return Some(Edit { start, end: next, replacement: String::new(), rule });
	}

	None
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
) -> HashSet<usize> {
	let import008_candidates = collect_import008_candidates(ctx);
	let import008_use_recovery_candidates =
		collect_import008_use_recovery_candidates(ctx, use_runs, imported_full_paths_by_symbol);
	let blocked_symbols = build_import008_blocked_symbols(
		&import008_candidates,
		imported_full_paths_by_symbol,
		local_defined_symbols,
	);
	let mut pending_import_paths = BTreeSet::new();
	let mut import008_group_skip_lines = HashSet::new();

	apply_import008_shorten_candidates(
		ctx,
		violations,
		edits,
		emit_edits,
		&import008_candidates,
		&blocked_symbols,
		imported_full_paths_by_symbol,
		&mut pending_import_paths,
		&mut import008_group_skip_lines,
	);
	apply_import008_use_recovery_edits(
		ctx,
		violations,
		edits,
		emit_edits,
		&import008_use_recovery_candidates,
		&blocked_symbols,
		local_defined_symbols,
		imported_full_paths_by_symbol,
		&mut pending_import_paths,
		&mut import008_group_skip_lines,
	);

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

fn build_import008_blocked_symbols(
	import008_candidates: &[Import008Candidate],
	imported_full_paths_by_symbol: &HashMap<String, HashSet<String>>,
	local_defined_symbols: &HashSet<String>,
) -> HashSet<String> {
	let mut candidate_paths_by_symbol: HashMap<String, HashSet<String>> = HashMap::new();

	for candidate in import008_candidates {
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
	}

	blocked_symbols
}

#[allow(clippy::too_many_arguments)]
fn apply_import008_shorten_candidates(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	import008_candidates: &[Import008Candidate],
	blocked_symbols: &HashSet<String>,
	imported_full_paths_by_symbol: &HashMap<String, HashSet<String>>,
	pending_import_paths: &mut BTreeSet<String>,
	import008_group_skip_lines: &mut HashSet<usize>,
) {
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
			replacement: candidate.replacement.clone(),
			rule: "RUST-STYLE-IMPORT-008",
		});
		import008_group_skip_lines.insert(candidate.line);

		if !already_imported {
			pending_import_paths.insert(candidate.import_path.clone());
		}
	}
}

#[allow(clippy::too_many_arguments)]
fn apply_import008_use_recovery_edits(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	import008_use_recovery_candidates: &[Import008UseRecoveryCandidate],
	blocked_symbols: &HashSet<String>,
	local_defined_symbols: &HashSet<String>,
	imported_full_paths_by_symbol: &HashMap<String, HashSet<String>>,
	pending_import_paths: &mut BTreeSet<String>,
	import008_group_skip_lines: &mut HashSet<usize>,
) {
	for candidate in import008_use_recovery_candidates {
		if blocked_symbols.contains(&candidate.symbol)
			|| local_defined_symbols.contains(&candidate.symbol)
		{
			continue;
		}

		shared::push_violation(
			violations,
			ctx,
			candidate.line,
			"RUST-STYLE-IMPORT-008",
			"Prefer merging child imports into existing parent module imports.",
			true,
		);

		if !emit_edits {
			continue;
		}

		if let (Some(start), Some(end)) = (
			shared::offset_from_line(&ctx.line_starts, candidate.start_line),
			shared::offset_from_line(&ctx.line_starts, candidate.end_line + 1),
		) {
			edits.push(Edit {
				start,
				end,
				replacement: String::new(),
				rule: "RUST-STYLE-IMPORT-008",
			});
			import008_group_skip_lines.insert(candidate.line);

			let target_compact = compact_path_for_match(&candidate.import_path);
			let already_imported =
				imported_full_paths_by_symbol.get(&candidate.symbol).is_some_and(|paths| {
					paths.iter().any(|path| compact_path_for_match(path) == target_compact)
				});

			if !already_imported {
				pending_import_paths.insert(candidate.import_path.clone());
			}
		}
	}
}

fn collect_import008_use_recovery_candidates(
	ctx: &FileContext,
	use_runs: &[Vec<&TopItem>],
	imported_full_paths_by_symbol: &HashMap<String, HashSet<String>>,
) -> Vec<Import008UseRecoveryCandidate> {
	let mut candidates = Vec::new();

	for item in use_runs.iter().flat_map(|run| run.iter().copied()) {
		let Some(path) = extract_use_path(ctx, item) else {
			continue;
		};
		let Some((prefix, symbol)) = simple_import_prefix_symbol(&path) else {
			continue;
		};

		if prefix.contains("::") {
			continue;
		}
		if matches!(prefix.as_str(), "std" | "core" | "alloc" | "crate" | "self" | "super" | "Self")
		{
			continue;
		}

		let root = normalize_ident(&prefix).to_owned();
		let symbol_normalized = normalize_ident(&symbol).to_owned();
		let Some(root_full_paths) = imported_full_paths_by_symbol.get(&root) else {
			continue;
		};

		if root_full_paths.len() != 1 {
			continue;
		}

		let Some(root_full) = root_full_paths.iter().next() else {
			continue;
		};
		let root_full_compact = compact_path_for_match(root_full);
		let prefix_compact = compact_path_for_match(&prefix);

		if root_full_compact == prefix_compact {
			continue;
		}

		let import_path = format!("{root_full}::{symbol}");
		let current_compact = compact_path_for_match(&path);
		let import_path_compact = compact_path_for_match(&import_path);

		if let Some(existing_symbol_paths) = imported_full_paths_by_symbol.get(&symbol_normalized)
			&& existing_symbol_paths.iter().any(|existing| {
				let existing_compact = compact_path_for_match(existing);

				existing_compact != current_compact && existing_compact != import_path_compact
			}) {
			continue;
		}

		candidates.push(Import008UseRecoveryCandidate {
			line: item.line,
			start_line: item.start_line,
			end_line: item.end_line,
			symbol: symbol_normalized,
			import_path,
		});
	}

	candidates
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

fn collect_import008_candidates(ctx: &FileContext) -> Vec<Import008Candidate> {
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
	for macro_call in ctx.source_file.syntax().descendants().filter_map(ast::MacroCall::cast) {
		let Some(path) = macro_call.path() else {
			continue;
		};

		if path.qualifier().is_none() || is_inside_cfg_test_module(&path) {
			continue;
		}

		let mut segments = Vec::new();

		if !collect_path_segment_texts(&path, &mut segments) {
			continue;
		}
		if segments.len() < 3 {
			continue;
		}

		let module_name = segments[segments.len() - 2].clone();
		let macro_name = segments[segments.len() - 1].clone();

		if !is_same_ident(&module_name, &macro_name) {
			continue;
		}

		let symbol = normalize_ident(&module_name).to_owned();

		if matches!(symbol.as_str(), "" | "self" | "super" | "crate" | "Self") {
			continue;
		}

		let root = segments[0].as_str();

		if is_non_importable_root(root) {
			continue;
		}

		let import_path = segments[..segments.len() - 1].join("::");
		let replacement = format!("{module_name}::{macro_name}");
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

fn collect_qualified_value_paths_by_symbol(ctx: &FileContext) -> HashMap<String, HashSet<String>> {
	let mut out: HashMap<String, HashSet<String>> = HashMap::new();

	for path in ctx.source_file.syntax().descendants().filter_map(ast::Path::cast) {
		if path.qualifier().is_none() || is_inside_cfg_test_module(&path) {
			continue;
		}
		if path.syntax().ancestors().any(|node| Use::cast(node).is_some()) {
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
		if path.syntax().ancestors().any(|node| Use::cast(node).is_some()) {
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

fn is_inside_cfg_test_module(path: &ast::Path) -> bool {
	path.syntax().ancestors().filter_map(Module::cast).any(|module| {
		module
			.attrs()
			.any(|attr| attr.syntax().text().to_string().replace(' ', "").contains("cfg(test)"))
	})
}

fn collect_path_segment_texts(path: &ast::Path, out: &mut Vec<String>) -> bool {
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
		root_full: String,
	}

	let mut remaining = BTreeSet::new();
	let mut plans: BTreeMap<usize, BTreeMap<String, BTreeSet<String>>> = BTreeMap::new();
	let mut touched_lines = HashSet::new();
	let mut use_items = HashMap::new();
	let mut merge_targets_by_pending: HashMap<String, MergeTarget> = HashMap::new();
	let mut successful_merge_roots: HashSet<(usize, String)> = HashSet::new();

	for item in use_runs.iter().flat_map(|run| run.iter().copied()) {
		use_items.insert(item.line, item);
	}
	for pending in pending_import_paths {
		let Some((root_full, child_tail)) =
			resolve_import008_merge_target_for_pending_path(pending, imported_full_paths_by_symbol)
		else {
			remaining.insert(pending.clone());

			continue;
		};
		let Some(anchor_line) = find_use_item_line_importing_full_path(ctx, use_runs, root_full)
		else {
			remaining.insert(pending.clone());

			continue;
		};

		plans
			.entry(anchor_line)
			.or_default()
			.entry(root_full.to_owned())
			.or_default()
			.insert(child_tail.to_owned());
		merge_targets_by_pending
			.insert(pending.clone(), MergeTarget { anchor_line, root_full: root_full.to_owned() });
	}
	for (line, root_plans) in plans {
		let Some(item) = use_items.get(&line).copied() else {
			continue;
		};
		let Some((start, end)) = item_text_range(ctx, item) else {
			continue;
		};
		let Some(raw) = ctx.text.get(start..end) else {
			continue;
		};
		let Some(use_path) = extract_use_path_from_text(raw) else {
			continue;
		};
		let mut merged_path = use_path;
		let mut merged_any = false;

		for (root_full, children) in root_plans {
			let Some(next_path) =
				merge_children_into_use_path_for_root(&merged_path, &root_full, &children)
			else {
				continue;
			};

			successful_merge_roots.insert((line, root_full));

			merged_any |= next_path != merged_path;
			merged_path = next_path;
		}

		if !merged_any {
			continue;
		}

		if let Some(rewritten) = rewrite_use_item_with_path(raw, &merged_path)
			&& rewritten != raw
		{
			edits.push(Edit { start, end, replacement: rewritten, rule: "RUST-STYLE-IMPORT-008" });
			touched_lines.insert(line);
		}
	}
	for pending in pending_import_paths {
		let Some(target) = merge_targets_by_pending.get(pending) else {
			remaining.insert(pending.clone());

			continue;
		};
		let can_consume =
			successful_merge_roots.contains(&(target.anchor_line, target.root_full.clone()));

		if !can_consume {
			remaining.insert(pending.clone());
		}
	}

	(remaining, touched_lines)
}

fn resolve_import008_merge_target_for_pending_path<'a>(
	pending: &'a str,
	imported_full_paths_by_symbol: &'a HashMap<String, HashSet<String>>,
) -> Option<(&'a str, &'a str)> {
	let pending = pending.trim();

	if pending.is_empty() {
		return None;
	}

	if let Some((root, child_tail)) = pending.split_once("::")
		&& let Some(root_full_paths) = imported_full_paths_by_symbol.get(root)
		&& root_full_paths.len() == 1
		&& let Some(root_full) = root_full_paths.iter().next()
	{
		let child_tail = child_tail.trim();

		if !child_tail.is_empty() {
			return Some((root_full.as_str(), child_tail));
		}
	}
	if let Some((parent, module_name)) = pending.rsplit_once("::") {
		let module_name = module_name.trim();
		let module_root = parent.trim();

		if is_same_ident(module_root.rsplit("::").next().unwrap_or(module_root).trim(), module_name)
		{
			let module_compact = compact_path_for_match(module_root);

			if imported_full_paths_by_symbol.values().any(|full_paths| {
				full_paths.iter().any(|full_path| {
					full_path.rsplit_once("::").is_some_and(|(parent_full, _)| {
						compact_path_for_match(parent_full) == module_compact
					})
				})
			}) {
				return Some((module_root, "self"));
			}
		}
	}

	let pending_compact = compact_path_for_match(pending);

	if imported_full_paths_by_symbol.values().any(|full_paths| {
		full_paths.iter().any(|full_path| {
			full_path.rsplit_once("::").is_some_and(|(parent_full, _)| {
				compact_path_for_match(parent_full) == pending_compact
			})
		})
	}) {
		return Some((pending, "self"));
	}

	let mut best: Option<(&str, &str)> = None;

	for root_full in imported_full_paths_by_symbol.values().filter_map(|paths| {
		if paths.len() == 1 { paths.iter().next().map(String::as_str) } else { None }
	}) {
		let prefix = format!("{root_full}::");

		if !pending.starts_with(&prefix) {
			continue;
		}

		let child_tail = pending.strip_prefix(&prefix)?.trim();

		if child_tail.is_empty() {
			continue;
		}

		match best {
			Some((current_root, _)) if current_root.len() >= root_full.len() => {},
			_ => best = Some((root_full, child_tail)),
		}
	}

	best
}

fn find_use_item_line_importing_full_path(
	ctx: &FileContext,
	use_runs: &[Vec<&TopItem>],
	target_path: &str,
) -> Option<usize> {
	let target_compact = compact_path_for_match(target_path);
	let mut containing_line = None;

	for item in use_runs.iter().flat_map(|run| run.iter().copied()) {
		let Some(path) = extract_use_path(ctx, item) else {
			continue;
		};
		let compact_path = compact_path_for_match(&path);

		if let Some((prefix, _, _)) = parse_braced_path_parts_allow_alias(&path) {
			let prefix_root = compact_path_for_match(prefix.trim_end_matches('{'));
			let prefix_root = prefix_root.strip_suffix("::").unwrap_or(&prefix_root);

			if prefix_root == target_compact {
				containing_line.get_or_insert(item.line);
			}
		}

		if compact_path == target_compact {
			return Some(item.line);
		}
		if imported_full_paths_from_use_path(&path)
			.into_iter()
			.any(|full_path| compact_path_for_match(&full_path) == target_compact)
		{
			containing_line.get_or_insert(item.line);
		}
	}

	containing_line
}

fn merge_children_into_use_path_for_root(
	use_path: &str,
	root_full: &str,
	children: &BTreeSet<String>,
) -> Option<String> {
	let mut current = use_path.to_owned();
	let mut changed = false;

	for child in children {
		let next = merge_single_child_into_use_path_for_root(&current, root_full, child)?;

		if next != current {
			changed = true;
			current = next;
		}
	}

	if changed { Some(current) } else { None }
}

fn merge_single_child_into_use_path_for_root(
	use_path: &str,
	root_full: &str,
	child_tail: &str,
) -> Option<String> {
	let root_compact = compact_path_for_match(root_full);

	if compact_path_for_match(use_path) == root_compact {
		return Some(format!("{root_full}::{{self, {child_tail}}}"));
	}

	if let Some(merged) =
		try_merge_child_into_direct_root_braced_use_path(use_path, &root_compact, child_tail)
	{
		return Some(merged);
	}

	try_merge_child_into_parent_braced_use_path(use_path, root_full, child_tail)
}

fn try_merge_child_into_direct_root_braced_use_path(
	use_path: &str,
	root_compact: &str,
	child_tail: &str,
) -> Option<String> {
	let (prefix, close, mut segments) = parse_braced_path_parts_allow_alias(use_path)?;
	let prefix_root = prefix[..prefix.len().saturating_sub(1)].trim();
	let prefix_root = prefix_root.strip_suffix("::").unwrap_or(prefix_root).trim();

	if compact_path_for_match(prefix_root) != *root_compact {
		return None;
	}
	if merge_child_tail_into_braced_segments(&mut segments, child_tail) {
		return Some(format!("{}{}{}", prefix, segments.join(", "), &use_path[close..=close]));
	}

	None
}

fn try_merge_child_into_parent_braced_use_path(
	use_path: &str,
	root_full: &str,
	child_tail: &str,
) -> Option<String> {
	let (parent_full, root_head) = root_full.rsplit_once("::")?;
	let (prefix, close, mut segments) = parse_braced_path_parts_allow_alias(use_path)?;
	let prefix_root = prefix[..prefix.len().saturating_sub(1)].trim();
	let prefix_root = prefix_root.strip_suffix("::").unwrap_or(prefix_root).trim();

	if compact_path_for_match(prefix_root) != compact_path_for_match(parent_full) {
		return None;
	}

	let root_head_compact = compact_path_for_match(root_head);
	let mut matched_root = false;
	let mut changed = false;

	for segment in &mut segments {
		let trimmed = segment.trim();

		if compact_path_for_match(trimmed) == root_head_compact {
			*segment = format!("{root_head}::{{self, {child_tail}}}");
			matched_root = true;
			changed = true;

			break;
		}

		let Some((head, inner)) = parse_single_level_nested_use_segment(trimmed) else {
			continue;
		};

		if compact_path_for_match(head) != root_head_compact {
			continue;
		}

		let mut nested_children = split_top_level_csv(inner);
		let nested_changed =
			merge_child_tail_into_braced_segments(&mut nested_children, child_tail);

		if !nested_changed {
			matched_root = true;

			break;
		}

		*segment = format!("{head}::{{{}}}", nested_children.join(", "));
		matched_root = true;
		changed = true;

		break;
	}

	if !matched_root {
		return None;
	}
	if !changed {
		return Some(use_path.to_owned());
	}

	Some(format!("{}{}{}", prefix, segments.join(", "), &use_path[close..=close]))
}

fn parse_single_level_nested_use_segment(segment: &str) -> Option<(&str, &str)> {
	let (head, rest) = segment.split_once("::{")?;

	if !rest.ends_with('}') {
		return None;
	}

	let inner = &rest[..rest.len().saturating_sub(1)];

	if inner.contains('{') || inner.contains('}') {
		return None;
	}

	Some((head.trim(), inner))
}

fn merge_child_tail_into_braced_segments(segments: &mut Vec<String>, child_tail: &str) -> bool {
	let child_compact = compact_path_for_match(child_tail);

	for segment in segments.iter_mut() {
		let trimmed = segment.trim();

		if compact_path_for_match(trimmed) == child_compact {
			return false;
		}
		if is_keep_alive_alias_for_child(trimmed, child_tail) {
			*segment = child_tail.to_owned();

			return true;
		}

		let Some((head, inner)) = parse_single_level_nested_use_segment(trimmed) else {
			continue;
		};
		let Some((child_head, child_rest)) = child_tail.split_once("::") else {
			continue;
		};

		if compact_path_for_match(head) != compact_path_for_match(child_head) {
			continue;
		}

		let mut children = split_top_level_csv(inner);
		let child_rest_compact = compact_path_for_match(child_rest);
		let mut found_exact = false;
		let mut replaced_alias = false;

		for child in children.iter_mut() {
			let child_trimmed = child.trim();

			if compact_path_for_match(child_trimmed) == child_rest_compact {
				found_exact = true;

				break;
			}
			if is_keep_alive_alias_for_child(child_trimmed, child_rest) {
				*child = child_rest.to_owned();
				replaced_alias = true;

				break;
			}
		}

		if found_exact {
			return false;
		}
		if replaced_alias {
			*segment = format!("{head}::{{{}}}", children.join(", "));

			return true;
		}
		if child_rest_compact == "self" {
			children.insert(0, child_rest.to_owned());
		} else {
			children.push(child_rest.to_owned());
		}

		*segment = format!("{head}::{{{}}}", children.join(", "));

		return true;
	}

	if child_compact == "self" {
		segments.insert(0, child_tail.to_owned());
	} else {
		segments.push(child_tail.to_owned());
	}

	true
}

fn is_keep_alive_alias_for_child(segment: &str, child_tail: &str) -> bool {
	let Some((base, alias)) = split_import_leaf_alias(segment) else {
		return false;
	};

	alias.is_some_and(|value| value == "_")
		&& compact_path_for_match(base) == compact_path_for_match(child_tail)
}

fn compact_path_for_match(path: &str) -> String {
	path.chars().filter(|ch| !ch.is_whitespace()).collect()
}

fn normalize_use_path_for_equivalence(path: &str) -> String {
	let mut normalized = compact_path_for_match(path);

	// Treat trailing commas in braced `use` groups as formatting-only.
	while normalized.contains(",}") {
		normalized = normalized.replace(",}", "}");
	}

	normalized
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

fn parse_braced_path_parts_allow_alias(path: &str) -> Option<(String, usize, Vec<String>)> {
	let (open, close) = top_level_brace_range(path)?;

	if !path[close + 1..].trim().is_empty() {
		return None;
	}

	let prefix = path[..open + 1].to_owned();
	let inner = &path[open + 1..close];
	let segments = split_top_level_csv(inner);

	if segments.is_empty() {
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

fn normalize_mixed_self_child_use_path(ctx: &FileContext, path: &str) -> Option<String> {
	let (prefix, close, segments) = parse_braced_path_parts_allow_alias(path)?;
	let root = prefix
		.trim()
		.trim_start_matches("pub ")
		.trim()
		.trim_end_matches('{')
		.trim()
		.trim_end_matches("::")
		.trim()
		.split("::")
		.next()
		.unwrap_or_default();
	let allow_drop_unused_self = matches!(root, "crate" | "self" | "super");
	let (groups, parsed_heads) = build_mixed_use_groups(&segments);
	let rewritten =
		rewrite_mixed_use_segments(ctx, &segments, &groups, &parsed_heads, allow_drop_unused_self)?;
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
	ctx: &FileContext,
	segments: &[String],
	groups: &HashMap<String, MixedUseGroup>,
	parsed_heads: &[Option<String>],
	allow_drop_unused_self: bool,
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
			if allow_drop_unused_self
				&& group.indices.first().copied() == Some(idx)
				&& group.has_self
				&& !symbol_is_referenced_outside_use(ctx, &head)
				&& let Some(rewritten_segment) =
					drop_unused_self_from_nested_use_segment(segment, &head)
			{
				rewritten.push(rewritten_segment);

				merged = true;

				continue;
			}

			rewritten.push(segment.to_owned());

			continue;
		}

		let children = dedup_mixed_group_children(group);
		let keep_self = symbol_is_referenced_outside_use(ctx, &head);
		let combined = if keep_self {
			format!("{head}::{{self, {}}}", children.join(", "))
		} else {
			format!("{head}::{{{}}}", children.join(", "))
		};

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

fn drop_unused_self_from_nested_use_segment(segment: &str, head: &str) -> Option<String> {
	let trimmed = segment.trim();
	let (nested_head, inner) = parse_single_level_nested_use_segment(trimmed)?;

	if !is_same_ident(nested_head, head) {
		return None;
	}

	let mut children = split_top_level_csv(inner);
	let original_len = children.len();

	children.retain(|child| child.trim() != "self");

	if children.len() == original_len {
		return None;
	}
	if children.is_empty() {
		Some(head.to_owned())
	} else {
		Some(format!("{head}::{{{}}}", children.join(", ")))
	}
}

fn rewrite_use_item_with_path(raw: &str, new_path: &str) -> Option<String> {
	let (start, end) = find_use_path_range(raw)?;
	let original_path = raw.get(start..end)?;

	// Do not rewrite `use` paths when the change is formatting-only. This avoids
	// collapsing already-correct multi-line imports into a single line.
	if normalize_use_path_for_equivalence(original_path)
		== normalize_use_path_for_equivalence(new_path)
	{
		return None;
	}

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
