use std::{
	collections::{BTreeMap, BTreeSet, HashMap, HashSet},
	fs,
	path::{Path, PathBuf},
};

use ra_ap_syntax::{
	AstNode as _, AstToken as _, Edition, SyntaxKind, SyntaxNode,
	ast::{
		self, Attr, HasAttrs as _, HasName as _, HasVisibility as _, Item, MacroCall, Meta, Module,
		NameRef, TokenTree, Use, UseTree,
	},
};
use regex::Regex;

use crate::style::{
	imports::paths,
	shared::{self, Edit, FileContext, TopItem, Violation},
	syntax, workspace,
};

pub(crate) fn exported_symbols_from_super_scope(use_item: &Use) -> Option<BTreeSet<String>> {
	exported_symbols_from_super_scope_impl(use_item)
}

pub(crate) fn declared_file_is_test_only(path: &Path) -> bool {
	declared_file_module_contexts(path)
		.is_some_and(|contexts| !contexts.is_empty() && contexts.iter().all(|(_, test)| *test))
}

pub(super) fn apply_import010_no_super_use_rule(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	use_items: &[&TopItem],
) -> HashSet<usize> {
	let top_use_lines = use_items.iter().map(|item| item.line).collect::<HashSet<_>>();
	let mut fixed_top_level_lines = HashSet::new();

	for use_item in ctx.source_file.syntax().descendants().filter_map(Use::cast) {
		let Some(use_tree) = use_item.use_tree() else {
			continue;
		};
		let use_path = use_tree.syntax().text().to_string();
		let start = usize::from(use_item.syntax().text_range().start());
		let end = usize::from(use_item.syntax().text_range().end());
		let line = shared::line_from_offset(&ctx.line_starts, start);

		if let Some((super_depth, tail)) = leading_super_depth_and_tail(&use_path) {
			let current_module_path = current_module_path_segments(ctx, &use_item);
			let replacement_path = current_module_path.as_ref().and_then(|path| {
				let parent_depth = path.len().checked_sub(super_depth)?;

				resolve_parent_use_tail(&path[..parent_depth], tail)
			});
			let replacement_path = replacement_path.filter(|_| {
				// Keep the parent glob available until named trait aliases are normalized.
				super_depth != 1 || tail != "*" || parent_scope_trait_symbols(&use_item).is_some()
			});
			let fixable = replacement_path.is_some();

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

			let Some(replacement_path) = replacement_path else {
				continue;
			};
			let replacement = paths::rewrite_use_item_with_path(
				&use_item.syntax().text().to_string(),
				&replacement_path,
			);
			let Some(replacement) = replacement else {
				continue;
			};

			edits.push(Edit { start, end, replacement, rule: "RUST-STYLE-IMPORT-010" });

			if top_use_lines.contains(&line) {
				fixed_top_level_lines.insert(line);
			}

			continue;
		}
	}

	fixed_top_level_lines
}

pub(super) fn apply_import007_no_glob_use_rule(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	use_items: &[&TopItem],
) -> HashSet<usize> {
	let top_use_lines = use_items.iter().map(|item| item.line).collect::<HashSet<_>>();
	let mut fixed_top_level_lines = HashSet::new();

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
		let replacement = if paths::compact_path_for_match(&use_path) == "super::*" {
			build_parent_glob_item_replacement(ctx, &use_item)
		} else if let Some(replacement) =
			build_crate_glob_item_replacement(ctx, &use_item, &use_path)
		{
			Some(replacement)
		} else {
			build_glob_use_replacement(ctx, &use_item, &use_path)
				.and_then(|replacement_path| {
					paths::rewrite_use_item_with_path(
						&use_item.syntax().text().to_string(),
						&replacement_path,
					)
				})
				.or_else(|| build_mixed_crate_glob_item_replacement(ctx, &use_item))
		};
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

pub(super) fn scope_import_target(
	path: &str,
	source_prefix: &str,
	items: &[Item],
) -> Option<String> {
	let path = paths::compact_path_for_match(path);
	let source = source_prefix.strip_prefix("crate")?.trim_start_matches("::");
	let owner =
		source.split("::").filter(|part| !part.is_empty()).map(str::to_owned).collect::<Vec<_>>();

	if path.starts_with("super::") {
		return resolve_parent_use_tail(&owner, &path);
	}

	if let Some(tail) = path.strip_prefix("self::") {
		return Some(crate_absolute_use_path(&owner, tail));
	}

	if path.starts_with("crate::") || path.starts_with("::") {
		return Some(path);
	}

	let head = path.split("::").next()?;

	for item in items {
		if let Item::Module(module) = item
			&& module.name().is_some_and(|name| name.text() == head)
		{
			return Some(crate_absolute_use_path(&owner, &path));
		}
		if let Item::Use(import) = item {
			let imported = import.use_tree()?.syntax().text().to_string();

			if paths::imported_symbols_from_use_path(&imported).iter().any(|name| name == head)
				&& !paths::imported_full_paths_from_use_path(&imported)
					.iter()
					.any(|target| target == head)
			{
				return None;
			}
		}
	}

	Some(path)
}

pub(super) fn resolve_crate_module_scope(
	ctx: &FileContext,
	segments: &[String],
) -> Option<(PathBuf, SyntaxNode)> {
	let crate_dir = find_crate_dir(&ctx.path)?;
	let roots = workspace::package_target_roots(&crate_dir)
		.ok()
		.flatten()
		.unwrap_or_else(|| vec![crate_dir.join("src/lib.rs"), crate_dir.join("src/main.rs")]);
	let mut result: Option<(PathBuf, SyntaxNode)> = None;

	for root in roots {
		let Some(candidate) = resolve_file_module_scope(&root, segments) else {
			continue;
		};

		if let Some(previous) = &result
			&& (previous.0 != candidate.0 || previous.1.text_range() != candidate.1.text_range())
		{
			return None;
		}

		result = Some(candidate);
	}

	result
}

pub(super) fn crate_absolute_use_path(parent_segments: &[String], tail: &str) -> String {
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

pub(super) fn symbol_referenced_by_child_module(ctx: &FileContext, symbol: &str) -> bool {
	let Some(owner) =
		ctx.declared_module_path.get_or_init(|| declared_file_module_path(&ctx.path)).as_ref()
	else {
		return true;
	};

	symbol_referenced_by_descendant(ctx, ctx.source_file.syntax(), owner, symbol)
}

fn exported_symbols_from_super_scope_impl(use_item: &Use) -> Option<BTreeSet<String>> {
	let current_module = use_item.syntax().ancestors().find_map(Module::cast)?;
	let current_module_item_list = current_module.item_list()?;
	let current_module_name = current_module.name().map(|name| name.text().to_string());
	let already_imported =
		imported_symbols_from_current_module_use_items(&current_module, use_item);
	let used_symbols = collect_used_symbols_from_syntax(current_module_item_list.syntax());
	let parent_traits = parent_scope_trait_symbols(use_item)?;
	let mut symbols = BTreeSet::new();

	collect_scope_symbols_from_items(parent_scope_items(use_item)?.into_iter(), &mut symbols)?;

	if symbols.is_empty() {
		return None;
	}

	let used = symbols
		.into_iter()
		.filter(|symbol| current_module_name.as_deref() != Some(symbol.as_str()))
		.filter(|symbol| !matches!(symbol.as_str(), "tests" | "_test"))
		.filter(|symbol| !already_imported.contains(symbol))
		.filter(|symbol| used_symbols.contains(symbol) || parent_traits.contains(symbol))
		.collect::<BTreeSet<_>>();

	Some(used)
}

fn parent_scope_trait_symbols(use_item: &Use) -> Option<HashSet<String>> {
	scope_trait_symbols(&parent_scope_items(use_item)?)
}

fn scope_trait_symbols(items: &[Item]) -> Option<HashSet<String>> {
	let mut traits = HashSet::new();

	for item in items {
		match item {
			Item::Trait(item) =>
				if let Some(name) = item.name() {
					traits.insert(name.text().to_string());
				},
			Item::Use(item) => {
				let path = item.use_tree()?.syntax().text().to_string();
				let aliases = paths::collect_non_keep_alive_alias_bindings(&path);

				for full_path in paths::imported_full_paths_from_use_path(&path) {
					let Some(symbol) = paths::symbol_from_full_import_path(&full_path) else {
						continue;
					};

					if paths::looks_like_trait_import(&symbol, &full_path) {
						// Normalize named trait aliases before expanding a dependent glob.
						if aliases.iter().any(|(_, original)| {
							paths::compact_path_for_match(original) == full_path
						}) {
							return None;
						}

						traits.insert(symbol);
					}
				}
			},
			_ => {},
		}
	}

	Some(traits)
}

fn parent_scope_items(use_item: &Use) -> Option<Vec<Item>> {
	let module = use_item.syntax().ancestors().find_map(Module::cast)?;
	let scope = module
		.syntax()
		.ancestors()
		.skip(1)
		.find(|node| Module::can_cast(node.kind()) || ast::SourceFile::can_cast(node.kind()))?;
	let scope = if let Some(parent) = Module::cast(scope.clone()) {
		parent.item_list()?.syntax().clone()
	} else {
		scope
	};

	Some(scope.children().filter_map(Item::cast).collect())
}

fn cfg_meta_predicate(meta: Meta) -> Option<String> {
	match meta {
		Meta::CfgMeta(meta) => Some(meta.cfg_predicate()?.syntax().text().to_string()),
		Meta::CfgAttrMeta(meta) => {
			let condition = meta.cfg_predicate()?.syntax().text().to_string();
			let predicates = meta
				.metas()
				.map(cfg_meta_predicate)
				.collect::<Option<Vec<_>>>()?
				.into_iter()
				.filter(|predicate| !predicate.is_empty())
				.collect::<Vec<_>>();

			Some(if predicates.is_empty() {
				String::new()
			} else {
				format!("any(not({condition}), all({}))", predicates.join(", "))
			})
		},
		_ => Some(String::new()),
	}
}

fn item_cfg_predicate(item: &Item) -> Option<String> {
	let mut predicates = Vec::new();

	for attr in item.syntax().children().filter_map(Attr::cast) {
		let predicate = cfg_meta_predicate(attr.meta()?)?;

		if !predicate.is_empty() {
			predicates.push(predicate);
		}
	}

	Some(match predicates.as_slice() {
		[] => String::new(),
		[predicate] => predicate.clone(),
		_ => format!("all({})", predicates.join(", ")),
	})
}

fn scope_glob_symbol_conditions(
	items: &[Item],
	names: &BTreeSet<String>,
) -> Option<BTreeMap<String, BTreeSet<String>>> {
	let mut conditions = BTreeMap::<String, BTreeSet<String>>::new();

	for item in items {
		let symbols = if let Item::Use(import) = &item {
			paths::imported_symbols_from_use_path(&import.use_tree()?.syntax().text().to_string())
		} else {
			item_name_text(item).into_iter().collect()
		};
		let selected = symbols.into_iter().filter(|name| names.contains(name)).collect::<Vec<_>>();

		if selected.is_empty() {
			continue;
		}

		let predicate = item_cfg_predicate(item)?;

		for name in selected {
			conditions.entry(name).or_default().insert(predicate.clone());
		}
	}

	Some(conditions)
}

fn build_parent_glob_item_replacement(ctx: &FileContext, use_item: &Use) -> Option<String> {
	let names = exported_symbols_from_super_scope(use_item)?;
	let mut owner = current_module_path_segments(ctx, use_item)?;

	owner.pop()?;

	build_scoped_glob_item_replacement(
		use_item,
		"super",
		&crate_absolute_use_path(&owner, ""),
		names,
		&parent_scope_items(use_item)?,
	)
}

fn build_scoped_glob_item_replacement(
	use_item: &Use,
	prefix: &str,
	source_prefix: &str,
	names: BTreeSet<String>,
	items: &[Item],
) -> Option<String> {
	let conditions = scope_glob_symbol_conditions(items, &names)?;
	let mut groups = BTreeMap::<String, Vec<String>>::new();

	for name in names {
		let predicates = conditions.get(&name)?;
		let condition = if predicates.contains("") {
			String::new()
		} else if predicates.len() == 1 {
			format!("#[cfg({})]\n", predicates.first()?)
		} else {
			format!("#[cfg(any({}))]\n", predicates.iter().cloned().collect::<Vec<_>>().join(", "))
		};

		groups.entry(condition).or_default().push(name);
	}

	let mut imports = anonymous_glob_imports(use_item, source_prefix, items)?;

	if groups.is_empty() && imports.is_empty() {
		return None;
	}

	let raw = use_item.syntax().text().to_string();

	for (condition, names) in groups {
		let rewritten = paths::rewrite_use_item_with_path(
			&raw,
			&format_expanded_braced_use_path(prefix, &names),
		)?;

		imports.push(format!("{condition}{rewritten}"));
	}

	Some(imports.join("\n"))
}

fn anonymous_glob_imports(
	use_item: &Use,
	source_prefix: &str,
	items: &[Item],
) -> Option<Vec<String>> {
	let raw = use_item.syntax().text().to_string();
	let mut imports = Vec::new();

	for item in items {
		let Item::Use(source) = item else {
			continue;
		};
		let path = source.use_tree()?.syntax().text().to_string();
		let mut aliases = Vec::new();

		if !paths::collect_alias_bindings_from_use_segment(&path, &mut aliases) {
			return None;
		}

		for (_, path) in aliases.into_iter().filter(|(alias, _)| alias == "_") {
			let target = scope_import_target(&path, source_prefix, items)?;
			let condition = item_cfg_predicate(item)?;
			let rewritten = paths::rewrite_use_item_with_path(&raw, &format!("{target} as _"))?;

			imports.push(if condition.is_empty() {
				rewritten
			} else {
				format!("#[cfg({condition})]\n{rewritten}")
			});
		}
	}

	Some(imports)
}

fn build_mixed_crate_glob_item_replacement(ctx: &FileContext, use_item: &Use) -> Option<String> {
	let tree = use_item.use_tree()?;

	if tree.use_tree_list().is_none()
		|| use_item.visibility().is_some()
		|| use_item
			.syntax()
			.descendants_with_tokens()
			.any(|part| part.kind() == SyntaxKind::COMMENT)
	{
		return None;
	}

	let mut paths = Vec::new();

	flatten_use_tree_paths(&tree, "", &mut paths);

	if paths.iter().filter(|path| path.ends_with("::*")).count() != 1 {
		return None;
	}

	let raw = use_item.syntax().text().to_string();
	let mut replacements = Vec::new();

	for path in paths {
		let replacement = if path.ends_with("::*") {
			build_crate_glob_item_replacement(ctx, use_item, &path)?
		} else {
			paths::rewrite_use_item_with_path(&raw, &path)?
		};

		replacements.push(replacement);
	}

	Some(replacements.join("\n"))
}

fn flatten_use_tree_paths(tree: &UseTree, prefix: &str, paths: &mut Vec<String>) {
	let path = tree.path().map(|path| path.syntax().text().to_string()).unwrap_or_default();
	let module_self = path == "self" && !prefix.is_empty();
	let full = if path.is_empty() || module_self {
		prefix.to_owned()
	} else if prefix.is_empty() {
		path
	} else {
		format!("{prefix}::{path}")
	};

	if let Some(list) = tree.use_tree_list() {
		for child in list.syntax().children().filter_map(UseTree::cast) {
			flatten_use_tree_paths(&child, &full, paths);
		}
	} else if tree.star_token().is_some() {
		paths.push(format!("{full}::*"));
	} else {
		let rename =
			tree.rename().map(|rename| format!(" {}", rename.syntax().text())).unwrap_or_default();

		paths.push(if module_self {
			format!("{full}::{{self{rename}}}")
		} else {
			format!("{full}{rename}")
		});
	}
}

fn build_crate_glob_item_replacement(
	ctx: &FileContext,
	use_item: &Use,
	path: &str,
) -> Option<String> {
	let compact = paths::compact_path_for_match(path);
	let prefix = compact.strip_suffix("::*")?;
	let module = prefix.strip_prefix("crate::")?;
	let segments = module.split("::").map(str::to_owned).collect::<Vec<_>>();
	let (_, scope) = resolve_crate_module_scope(ctx, &segments)?;
	let local_scope = use_item.syntax().parent()?;

	// Item macros can create exports or local names that shadow a glob binding.
	if scope.children().any(|node| MacroCall::can_cast(node.kind()))
		|| local_scope.children().any(|node| MacroCall::can_cast(node.kind()))
	{
		return None;
	}

	let caller = current_module_path_segments(ctx, use_item)?;
	let items = scope
		.children()
		.filter_map(Item::cast)
		.filter(|item| caller.starts_with(&segments) || module_item_is_pub(item))
		.collect::<Vec<_>>();
	let mut symbols = BTreeSet::new();

	collect_scope_symbols_from_items(items.iter().cloned(), &mut symbols)?;

	let mut traits = scope_trait_symbols(&items)?;

	traits.extend(crate_scope_trait_symbols(ctx, prefix, &items)?);

	let used = collect_used_symbols_from_syntax(&local_scope);
	let local = local_scope
		.children()
		.filter_map(Item::cast)
		.filter(|item| item.syntax().text_range() != use_item.syntax().text_range())
		.flat_map(|item| match item {
			Item::Use(item) => item
				.use_tree()
				.map(|tree| {
					paths::imported_symbols_from_use_path(&tree.syntax().text().to_string())
				})
				.unwrap_or_default(),
			_ => item_name_text(&item).into_iter().collect(),
		})
		.chain(paths::imported_symbols_from_use_path(
			&use_item.use_tree()?.syntax().text().to_string(),
		))
		.collect::<HashSet<_>>();

	symbols.retain(|name| {
		!local.contains(name)
			&& (used.contains(name)
				|| traits.contains(name)
				|| symbol_referenced_by_descendant(ctx, &local_scope, &caller, name))
	});

	build_scoped_glob_item_replacement(use_item, prefix, prefix, symbols, &items)
}

fn crate_scope_trait_symbols(
	ctx: &FileContext,
	prefix: &str,
	items: &[Item],
) -> Option<HashSet<String>> {
	let mut traits = HashSet::new();

	for item in items {
		let Item::Use(import) = item else {
			continue;
		};
		let path = import.use_tree()?.syntax().text().to_string();
		let aliases = paths::collect_non_keep_alive_alias_bindings(&path);

		for full_path in paths::imported_full_paths_from_use_path(&path) {
			let Some(target) = scope_import_target(&full_path, prefix, items) else {
				continue;
			};
			let Some(tail) = target.strip_prefix("crate::") else {
				continue;
			};
			let mut segments = tail.split("::").map(str::to_owned).collect::<Vec<_>>();
			let symbol = segments.pop()?;
			let Some((_, scope)) = resolve_crate_module_scope(ctx, &segments) else {
				continue;
			};

			if scope.children().filter_map(Item::cast).any(|item| {
				matches!(item, Item::Trait(item) if item.name().is_some_and(|name| name.text() == symbol))
			}) {
				if aliases.iter().any(|(_, original)| paths::compact_path_for_match(original) == full_path) {
					return None;
				}

				traits.insert(symbol);
			}
		}
	}

	Some(traits)
}

fn resolve_file_module_scope(file: &Path, segments: &[String]) -> Option<(PathBuf, SyntaxNode)> {
	let file = file.canonicalize().ok()?;
	let text = fs::read_to_string(&file).ok()?;
	let parsed = ra_ap_syntax::SourceFile::parse(&text, Edition::CURRENT);

	if !parsed.errors().is_empty() {
		return None;
	}

	let parent = file.parent()?;
	let directory = if matches!(file.file_name()?.to_str(), Some("lib.rs" | "main.rs" | "mod.rs")) {
		parent.to_path_buf()
	} else {
		parent.join(file.file_stem()?)
	};

	resolve_inline_module_scope(&file, parsed.tree().syntax(), parent, &directory, segments)
}

fn resolve_inline_module_scope(
	file: &Path,
	scope: &SyntaxNode,
	path_directory: &Path,
	directory: &Path,
	segments: &[String],
) -> Option<(PathBuf, SyntaxNode)> {
	let Some((name, rest)) = segments.split_first() else {
		return Some((file.to_path_buf(), scope.clone()));
	};
	let mut candidates = scope
		.children()
		.filter_map(Module::cast)
		.filter(|module| module.name().is_some_and(|candidate| candidate.text() == name));
	let module = candidates.next()?;

	if candidates.next().is_some() {
		return None;
	}

	if let Some(items) = module.item_list() {
		let nested = directory.join(name);

		resolve_inline_module_scope(file, items.syntax(), &nested, &nested, rest)
	} else {
		let child = declared_child_file(&module, path_directory, directory, name);

		resolve_file_module_scope(&child, rest)
	}
}

fn build_glob_use_replacement(ctx: &FileContext, use_item: &Use, use_path: &str) -> Option<String> {
	let compact = paths::compact_path_for_match(use_path);

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

		return Some(replacement);
	}

	let prefix = compact.strip_suffix("::*")?;

	if prefix == "rayon::prelude" {
		let traits = rayon_traits_for_usage(ctx);

		if traits.is_empty() {
			return None;
		}

		let names = traits.into_iter().map(str::to_owned).collect::<Vec<_>>();

		return Some(format_expanded_braced_use_path("rayon::iter", &names));
	}

	let symbols = exported_symbols_for_glob_prefix(ctx, use_item, prefix)?;

	if symbols.is_empty() {
		return None;
	}

	let names = if use_item.visibility().is_some() {
		symbols.into_iter().collect::<Vec<_>>()
	} else {
		let used_symbols = collect_used_symbols_from_syntax(ctx.source_file.syntax());

		symbols.into_iter().filter(|symbol| used_symbols.contains(symbol)).collect::<Vec<_>>()
	};

	if names.is_empty() {
		return None;
	}

	Some(format_expanded_braced_use_path(prefix, &names))
}

fn format_expanded_braced_use_path(prefix: &str, names: &[String]) -> String {
	format!("{prefix}::{{{}}}", names.join(", "))
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

	if let Some(module_name) = prefix.strip_prefix("self::")
		&& !module_name.is_empty()
		&& !module_name.contains("::")
	{
		return exported_symbols_from_sibling_module(use_item, module_name);
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

fn exported_symbols_from_named_module(root_file: &Path, module_name: &str) -> BTreeSet<String> {
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
		return exported_symbols_from_module_items(
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
			let exported = exported_symbols_from_module_items(
				module_parsed.syntax().children().filter_map(Item::cast),
			);

			if !exported.is_empty() {
				return exported;
			}
		}
	}

	BTreeSet::new()
}

fn exported_symbols_from_sibling_module(
	use_item: &Use,
	module_name: &str,
) -> Option<BTreeSet<String>> {
	let parent = use_item.syntax().parent()?;

	for item in parent.children().filter_map(Item::cast) {
		let Item::Module(module_item) = item else {
			continue;
		};
		let Some(name) = module_item.name() else {
			continue;
		};

		if name.text() != module_name {
			continue;
		}

		let item_list = module_item.item_list()?;
		let symbols = exported_symbols_from_module_items(
			item_list.syntax().children().filter_map(Item::cast),
		);

		if !symbols.is_empty() {
			return Some(symbols);
		}
	}

	None
}

fn exported_symbols_from_module_items(items: impl Iterator<Item = Item>) -> BTreeSet<String> {
	let mut symbols = BTreeSet::new();

	for item in items {
		match item {
			Item::Use(use_item) => {
				if use_item.visibility().is_none() {
					continue;
				}

				let Some(use_tree) = use_item.use_tree() else {
					continue;
				};
				let use_path = use_tree.syntax().text().to_string();

				for symbol in paths::imported_symbols_from_use_path(&use_path) {
					symbols.insert(symbol);
				}
			},
			Item::MacroCall(macro_call) => {
				let text = macro_call.syntax().text().to_string();

				for symbol in exported_symbols_from_macro_call_text(text.as_str()) {
					symbols.insert(symbol);
				}
			},
			_ => {
				if !module_item_is_pub(&item) {
					continue;
				}

				if let Some(name) = item_name_text(&item) {
					symbols.insert(name);
				}
			},
		}
	}

	symbols
}

fn module_item_is_pub(item: &Item) -> bool {
	match item {
		Item::Module(node) => node.visibility().is_some(),
		Item::Use(node) => node.visibility().is_some(),
		Item::TypeAlias(node) => node.visibility().is_some(),
		Item::Const(node) => node.visibility().is_some(),
		Item::Static(node) => node.visibility().is_some(),
		Item::Trait(node) => node.visibility().is_some(),
		Item::Enum(node) => node.visibility().is_some(),
		Item::Struct(node) => node.visibility().is_some(),
		Item::Fn(node) => node.visibility().is_some(),
		Item::Impl(node) => node.visibility().is_some(),
		Item::Union(node) => node.visibility().is_some(),
		Item::MacroRules(node) => node.visibility().is_some(),
		_ => false,
	}
}

fn exported_symbols_from_macro_call_text(text: &str) -> BTreeSet<String> {
	let re = Regex::new(r"\b([A-Z][A-Z0-9_]{2,})\s*=").expect("Compile macro export symbol regex.");
	let mut symbols = BTreeSet::new();

	for capture in re.captures_iter(text) {
		if let Some(name) = capture.get(1) {
			symbols.insert(name.as_str().to_owned());
		}
	}

	symbols
}

fn find_crate_dir(path: &Path) -> Option<PathBuf> {
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

fn current_module_path_segments(ctx: &FileContext, use_item: &Use) -> Option<Vec<String>> {
	let mut module_path =
		ctx.declared_module_path.get_or_init(|| declared_file_module_path(&ctx.path)).clone()?;
	let mut inline_ancestors = use_item
		.syntax()
		.ancestors()
		.filter_map(Module::cast)
		.filter_map(|module| module.name().map(|name| name.text().to_string()))
		.collect::<Vec<_>>();

	inline_ancestors.reverse();
	module_path.extend(inline_ancestors);

	Some(module_path)
}

fn declared_file_module_path(path: &Path) -> Option<Vec<String>> {
	let Some(contexts) = declared_file_module_contexts(path) else {
		return Some(file_module_path_segments(path));
	};
	let matches = contexts.into_iter().map(|(owner, _)| owner).collect::<BTreeSet<_>>();

	match matches.len() {
		0 => Some(file_module_path_segments(path)),
		1 => matches.into_iter().next(),
		_ => None,
	}
}

fn declared_file_module_contexts(path: &Path) -> Option<BTreeSet<(Vec<String>, bool)>> {
	let crate_dir = find_crate_dir(path)?;
	let target = path.canonicalize().ok()?;
	let roots = workspace::package_target_roots(&crate_dir)
		.ok()
		.flatten()
		.unwrap_or_else(|| vec![crate_dir.join("src/lib.rs"), crate_dir.join("src/main.rs")]);
	let mut matches = BTreeSet::new();
	let mut stack = HashSet::new();

	for root in roots {
		collect_declared_module_paths(
			&root,
			&(Vec::new(), false),
			&target,
			&mut stack,
			&mut matches,
		);
	}

	Some(matches)
}

fn collect_declared_module_paths(
	file: &Path,
	context: &(Vec<String>, bool),
	target: &Path,
	stack: &mut HashSet<PathBuf>,
	matches: &mut BTreeSet<(Vec<String>, bool)>,
) {
	let Ok(file) = file.canonicalize() else {
		return;
	};

	if !stack.insert(file.clone()) {
		return;
	}
	if file == target {
		matches.insert(context.clone());
	}

	if let Ok(text) = fs::read_to_string(&file) {
		let parsed = ra_ap_syntax::SourceFile::parse(&text, Edition::CURRENT);
		let parent = file.parent().unwrap_or(&file);
		let directory = if matches!(
			file.file_name().and_then(|name| name.to_str()),
			Some("lib.rs" | "main.rs" | "mod.rs")
		) {
			parent.to_path_buf()
		} else {
			parent.join(file.file_stem().unwrap_or_default())
		};

		collect_module_declarations(
			parsed.tree().syntax(),
			parent,
			&directory,
			context,
			target,
			stack,
			matches,
		);
	}

	stack.remove(&file);
}

fn collect_module_declarations(
	syntax: &SyntaxNode,
	path_directory: &Path,
	directory: &Path,
	owner: &(Vec<String>, bool),
	target: &Path,
	stack: &mut HashSet<PathBuf>,
	matches: &mut BTreeSet<(Vec<String>, bool)>,
) {
	for module in syntax.children().filter_map(Module::cast) {
		let Some(name) = module.name().map(|name| name.text().to_string()) else {
			continue;
		};
		let test_only = owner.1
			|| module.attrs().any(|attr| {
				attr.syntax().text().to_string().split_whitespace().collect::<String>()
					== "#[cfg(test)]"
			});
		let mut module_path = owner.0.clone();

		module_path.push(name.clone());

		let context = (module_path, test_only);

		if let Some(items) = module.item_list() {
			let nested = directory.join(&name);

			collect_module_declarations(
				items.syntax(),
				&nested,
				&nested,
				&context,
				target,
				stack,
				matches,
			);

			continue;
		}

		let file = declared_child_file(&module, path_directory, directory, &name);

		collect_declared_module_paths(&file, &context, target, stack, matches);
	}
}

fn declared_child_file(
	module: &Module,
	path_directory: &Path,
	directory: &Path,
	name: &str,
) -> PathBuf {
	let explicit = module.attrs().find_map(|attr| {
		if attr.path()?.syntax().text() != "path" {
			return None;
		}

		let token = attr
			.syntax()
			.descendants_with_tokens()
			.filter_map(|part| part.into_token())
			.find_map(ast::String::cast)?;

		token.value().ok().map(|value| path_directory.join(value.as_ref()))
	});

	explicit.unwrap_or_else(|| {
		let flat = directory.join(format!("{name}.rs"));

		if flat.is_file() { flat } else { directory.join(name).join("mod.rs") }
	})
}

fn file_module_path_segments(path: &Path) -> Vec<String> {
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
	let mut rest = path.trim();

	loop {
		if rest == "super" {
			depth += 1;
			rest = "";

			break;
		}

		let Some(after) =
			rest.strip_prefix("super").and_then(|rest| rest.trim_start().strip_prefix("::"))
		else {
			break;
		};

		depth += 1;
		rest = after.trim_start();
	}

	if depth == 0 { None } else { Some((depth, rest)) }
}

fn resolve_parent_use_tail(parent: &[String], tail: &str) -> Option<String> {
	let tail = tail.trim();

	if let Some((depth, rest)) = leading_super_depth_and_tail(tail) {
		return resolve_parent_use_tail(&parent[..parent.len().checked_sub(depth)?], rest);
	}

	if paths::compact_path_for_match(tail).contains("super::")
		&& let Some(inner) = tail.strip_prefix('{').and_then(|tail| tail.strip_suffix('}'))
	{
		let paths = paths::split_top_level_csv(inner)
			.into_iter()
			.filter(|path| !path.trim().is_empty())
			.map(|path| resolve_parent_use_tail(parent, &path))
			.collect::<Option<Vec<_>>>()?;

		return Some(format!("{{{}}}", paths.join(", ")));
	}

	Some(crate_absolute_use_path(parent, tail))
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

		for symbol in paths::imported_symbols_from_use_path(&use_path) {
			if symbol == "*" {
				continue;
			}

			out.insert(symbol);
		}
	}

	out
}

fn collect_scope_symbols_from_items(
	items: impl Iterator<Item = Item>,
	out: &mut BTreeSet<String>,
) -> Option<()> {
	for item in items {
		match item {
			Item::Use(use_item) => {
				let Some(use_tree) = use_item.use_tree() else {
					continue;
				};
				let use_path = use_tree.syntax().text().to_string();
				// A parent glob can supply names absent from this local inventory.
				// Replacing the child's glob with a partial list loses those bindings.

				if use_path.contains('*') {
					return None;
				}

				for symbol in paths::imported_symbols_from_use_path(&use_path) {
					out.insert(symbol);
				}
			},
			_ =>
				if let Some(name) = item_name_text(&item) {
					out.insert(name);
				},
		}
	}

	Some(())
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

	for name_ref in syntax.descendants().filter_map(NameRef::cast) {
		let in_use_tree = name_ref.syntax().ancestors().any(|node| Use::can_cast(node.kind()));

		if in_use_tree {
			continue;
		}

		used.insert(name_ref.text().to_string());
	}
	for tree in syntax.descendants().filter_map(UseTree::cast) {
		if let Some(name) = paths::use_tree_root_name_ref(&tree) {
			used.insert(name.text().to_string());
		}
	}
	for tree in syntax.descendants().filter_map(TokenTree::cast) {
		for token in tree.syntax().descendants_with_tokens().filter_map(|part| part.into_token()) {
			if token.kind() == SyntaxKind::IDENT {
				used.insert(syntax::normalize_ident(token.text()).to_owned());
			}
		}
	}

	used
}

fn external_module_references(ctx: &FileContext, owner: &[String]) -> Option<HashSet<String>> {
	let parent = ctx.path.parent()?;
	let directory = if matches!(ctx.path.file_name()?.to_str()?, "lib.rs" | "main.rs" | "mod.rs") {
		parent.to_path_buf()
	} else {
		parent.join(ctx.path.file_stem()?)
	};
	let mut references = HashSet::new();
	let mut stack = HashSet::new();

	collect_external_module_references(
		ctx.source_file.syntax(),
		parent,
		&directory,
		owner,
		&mut stack,
		&mut references,
	)?;

	Some(references)
}

fn collect_external_module_references(
	syntax: &SyntaxNode,
	path_directory: &Path,
	directory: &Path,
	owner: &[String],
	stack: &mut HashSet<PathBuf>,
	references: &mut HashSet<String>,
) -> Option<()> {
	for module in syntax.children().filter_map(Module::cast) {
		let name = module.name()?.text().to_string();
		let mut child_owner = owner.to_vec();

		child_owner.push(name.clone());

		if let Some(items) = module.item_list() {
			let nested = directory.join(&name);

			collect_external_module_references(
				items.syntax(),
				&nested,
				&nested,
				&child_owner,
				stack,
				references,
			)?;

			continue;
		}

		let file =
			declared_child_file(&module, path_directory, directory, &name).canonicalize().ok()?;

		if !stack.insert(file.clone()) {
			return None;
		}

		let text = fs::read_to_string(&file).ok()?;
		let parsed = ra_ap_syntax::SourceFile::parse(&text, Edition::CURRENT);

		if !parsed.errors().is_empty() {
			return None;
		}

		collect_file_module_references(parsed.tree().syntax(), &child_owner, references);

		let parent = file.parent()?;
		let child_directory = if file.file_name()?.to_str()? == "mod.rs" {
			parent.to_path_buf()
		} else {
			parent.join(file.file_stem()?)
		};

		collect_external_module_references(
			parsed.tree().syntax(),
			parent,
			&child_directory,
			&child_owner,
			stack,
			references,
		)?;

		stack.remove(&file);
	}

	Some(())
}

fn collect_file_module_references(
	syntax: &SyntaxNode,
	owner: &[String],
	references: &mut HashSet<String>,
) {
	let bindings = module_reference_bindings(syntax, owner);

	for (node, reference) in qualified_reference_paths(syntax) {
		record_module_reference(&node, owner, &reference, references);

		if let Some((head, tail)) = reference.split_once("::")
			&& let Some(targets) = bindings.get(head)
		{
			references.extend(targets.iter().map(|target| format!("{target}::{tail}")));
		}
	}
	for item in syntax.descendants().filter_map(Use::cast) {
		let Some(tree) = item.use_tree() else {
			continue;
		};
		let tree_path = tree.syntax().text().to_string();
		let paths = paths::imported_reference_paths_from_use_path(&tree_path);

		for path in paths {
			record_module_reference(item.syntax(), owner, &path, references);
		}
	}
}

fn qualified_reference_paths(syntax: &SyntaxNode) -> Vec<(SyntaxNode, String)> {
	let tokens = syntax
		.descendants_with_tokens()
		.filter_map(|part| part.into_token())
		.filter(|token| !matches!(token.kind(), SyntaxKind::WHITESPACE | SyntaxKind::COMMENT))
		.collect::<Vec<_>>();
	let mut paths = Vec::new();

	for (index, token) in tokens.iter().enumerate() {
		if token.kind() != SyntaxKind::IDENT && !matches!(token.text(), "crate" | "self" | "super")
		{
			continue;
		}

		let mut cursor = index + 1;
		let mut path = token.text().to_owned();

		while cursor < tokens.len() {
			let separator = if tokens[cursor].text() == "::" {
				1
			} else if tokens[cursor].text() == ":"
				&& tokens.get(cursor + 1).is_some_and(|t| t.text() == ":")
			{
				2
			} else {
				break;
			};
			let Some(next) = tokens.get(cursor + separator) else {
				break;
			};

			if next.kind() != SyntaxKind::IDENT && !matches!(next.text(), "self" | "super") {
				break;
			}

			path.push_str("::");
			path.push_str(next.text());

			cursor += separator + 1;
		}

		if path.contains("::")
			&& let Some(parent) = token.parent()
		{
			paths.push((parent, path));
		}
	}

	paths
}

fn module_reference_bindings(
	syntax: &SyntaxNode,
	owner: &[String],
) -> HashMap<String, HashSet<String>> {
	let mut bindings = HashMap::<String, HashSet<String>>::new();

	for item in syntax.descendants().filter_map(Use::cast) {
		let Some(tree) = item.use_tree() else {
			continue;
		};
		let path = tree.syntax().text().to_string();
		let aliases = paths::collect_non_keep_alive_alias_bindings(&path);

		for original in paths::imported_full_paths_from_use_path(&path) {
			let Some(symbol) = paths::symbol_from_full_import_path(&original) else {
				continue;
			};
			let binding = aliases
				.iter()
				.find(|(_, path)| paths::compact_path_for_match(path) == original)
				.map_or(symbol, |(alias, _)| alias.clone());
			let mut targets = HashSet::new();

			record_module_reference(item.syntax(), owner, &original, &mut targets);

			bindings.entry(binding).or_default().extend(targets);
		}
	}

	bindings
}

fn record_module_reference(
	syntax: &SyntaxNode,
	owner: &[String],
	reference: &str,
	references: &mut HashSet<String>,
) {
	let mut current = owner.to_vec();
	let mut inline = syntax
		.ancestors()
		.filter_map(Module::cast)
		.filter_map(|m| m.name().map(|n| n.text().to_string()))
		.collect::<Vec<_>>();

	inline.reverse();
	current.extend(inline);

	if let Some((depth, tail)) = leading_super_depth_and_tail(reference) {
		if depth <= current.len() {
			references.insert(crate_absolute_use_path(&current[..current.len() - depth], tail));
		}
	} else if let Some(tail) = reference.strip_prefix("self::") {
		references.insert(crate_absolute_use_path(&current, tail));
	} else if reference.starts_with("crate::") {
		references.insert(reference.to_owned());
	}
}

fn symbol_referenced_by_descendant(
	ctx: &FileContext,
	scope: &SyntaxNode,
	owner: &[String],
	symbol: &str,
) -> bool {
	if !scope.descendants().any(|node| Module::can_cast(node.kind())) {
		return false;
	}

	let Some(file_owner) =
		ctx.declared_module_path.get_or_init(|| declared_file_module_path(&ctx.path)).as_ref()
	else {
		return true;
	};
	let binding = crate_absolute_use_path(owner, symbol);
	let binding_prefix = format!("{binding}::");
	let glob = crate_absolute_use_path(owner, "*");
	let relative_binding = owner.last().map(|name| format!("{name}::{symbol}"));
	let Some(external) = ctx
		.external_module_references
		.get_or_init(|| external_module_references(ctx, file_owner))
		.as_ref()
	else {
		return true;
	};

	if external.contains(&glob)
		|| external
			.iter()
			.any(|reference| reference == &binding || reference.starts_with(&binding_prefix))
	{
		return true;
	}

	for (node, reference) in qualified_reference_paths(scope) {
		if !node.ancestors().any(|node| Module::can_cast(node.kind())) {
			continue;
		}

		let mut resolved = HashSet::new();

		record_module_reference(&node, file_owner, &reference, &mut resolved);

		if reference == binding
			|| relative_binding.as_ref() == Some(&reference)
			|| resolved.contains(&binding)
		{
			return true;
		}
	}
	for use_item in scope.descendants().filter_map(Use::cast) {
		if !use_item.syntax().ancestors().any(|node| Module::can_cast(node.kind())) {
			continue;
		}

		let Some(tree) = use_item.use_tree() else {
			continue;
		};
		let tree_path = tree.syntax().text().to_string();
		let paths = paths::imported_reference_paths_from_use_path(&tree_path);

		for path in paths {
			let resolved = if let Some((depth, tail)) = leading_super_depth_and_tail(&path) {
				let Some(current) = current_module_path_segments(ctx, &use_item) else {
					return true;
				};

				if depth > current.len() {
					continue;
				}

				crate_absolute_use_path(&current[..current.len() - depth], tail)
			} else {
				path
			};

			if resolved == binding || resolved == glob {
				return true;
			}
		}
	}

	false
}
