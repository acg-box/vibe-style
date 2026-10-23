use std::collections::HashSet;

use ra_ap_syntax::{
	AstNode as _,
	ast::{Module, Path, Use},
};
use regex::Regex;

use crate::style::{
	imports::{
		TraitKeepAliveNormalizationState, UseItemAnalysis, groups, paths, qualification,
		references, scopes,
	},
	shared::{self, Edit, FileContext, TopItem, TopKind, Violation},
	syntax,
	workspace::{self, WORKSPACE_IMPORT_ROOTS},
};

pub(super) fn apply_import012_crate_keep_alive_rule(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	use_item_analyses: &[UseItemAnalysis<'_>],
	local_module_roots: &HashSet<String>,
) {
	let keep_alive_imports = use_item_analyses
		.iter()
		.filter_map(|use_item_analysis| {
			import012_keep_alive_root(&use_item_analysis.path, local_module_roots)
				.map(|root| (use_item_analysis.item, root))
		})
		.collect::<Vec<_>>();

	if keep_alive_imports.is_empty() {
		return;
	}

	let mut package_contexts = None;

	for (item, root) in keep_alive_imports {
		if import012_context_mentions_root(ctx, Some(item.start_offset), root.as_str()) {
			continue;
		}

		let package_contexts =
			package_contexts.get_or_insert_with(|| import012_package_contexts(ctx));

		if import012_package_contexts_mention_root(package_contexts, root.as_str()) {
			continue;
		}

		shared::push_violation(
			violations,
			ctx,
			item.line,
			"RUST-STYLE-IMPORT-012",
			"Do not add crate keep-alive imports `use dep as _;` unless another path in the package uses that crate.",
			false,
		);
	}
}

pub(super) fn push_alias_violation_if_needed(
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

pub(super) fn apply_import003_trait_keep_alive_rule(
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
		let referenced_directly = references::symbol_is_referenced_outside_use(ctx, &symbol);
		let message = if referenced_directly {
			format!("Trait import `{symbol}` is referenced directly; do not use `as _`.")
		} else {
			format!(
				"Trait keep-alive import `{symbol}` should use `as _` when not referenced directly."
			)
		};

		shared::push_violation(violations, ctx, item.line, "RUST-STYLE-IMPORT-003", &message, true);
	}

	let has_import004_pressure = qualification::use_path_needs_import004_fix(ctx, path);

	if !emit_edits || rewritten_use_path == path {
		return false;
	}
	if has_import004_pressure {
		return true;
	}

	if let Some((start, end)) = groups::item_text_range(ctx, item)
		&& let Some(raw) = ctx.text.get(start..end)
		&& let Some(rewritten) = paths::rewrite_use_item_with_path(raw, &rewritten_use_path)
	{
		edits.push(Edit { start, end, replacement: rewritten, rule: "RUST-STYLE-IMPORT-003" });

		return true;
	}

	false
}

pub(super) fn apply_import003_non_keep_alive_alias_rule(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	item: &TopItem,
	path: &str,
) -> bool {
	let mut aliases = paths::collect_non_keep_alive_alias_bindings(path);

	aliases.retain(|(alias, qualified_path)| {
		let Some(symbol) = paths::symbol_from_full_import_path(qualified_path) else {
			return true;
		};
		let is_trait_alias = paths::looks_like_trait_import(&symbol, qualified_path);
		let referenced = references::symbol_is_referenced_outside_use(ctx, alias);

		!is_trait_alias || referenced
	});

	if aliases.is_empty() {
		return false;
	}

	let mut referenced_aliases = Vec::new();
	let mut rewrites = Vec::<(usize, usize, String)>::new();
	let mut planned_ranges = Vec::<(usize, usize)>::new();
	let mut fixable = !aliases.iter().any(|(alias, _)| {
		scopes::symbol_referenced_by_child_module(ctx, alias)
			|| references::symbol_has_bare_identifier_pattern(ctx, alias)
	});

	for (alias, qualified_path) in &aliases {
		let referenced = references::symbol_is_referenced_outside_use(ctx, alias);

		if referenced {
			let candidates = collect_alias_reference_rewrites(ctx, alias, qualified_path);

			if candidates.is_empty() {
				fixable = false;
			} else {
				for (start, end, replacement) in candidates {
					if references::alias_is_shadowed_by_local_import(ctx, alias, start) {
						continue;
					}
					if planned_ranges
						.iter()
						.any(|(used_start, used_end)| start < *used_end && end > *used_start)
					{
						continue;
					}

					planned_ranges.push((start, end));
					rewrites.push((start, end, replacement));
				}
			}

			referenced_aliases.push(alias.clone());
		}
	}

	let alias_names = aliases.iter().map(|(alias, _)| alias.as_str()).collect::<Vec<_>>();

	for alias_name in alias_names {
		let message = format!(
			"Import alias `{alias_name}` is not allowed; use a fully qualified path at use sites."
		);

		shared::push_violation(
			violations,
			ctx,
			item.line,
			"RUST-STYLE-IMPORT-003",
			&message,
			fixable,
		);
	}

	if !emit_edits || !fixable {
		return false;
	}

	let aliases_to_remove = aliases.into_iter().map(|(alias, _)| alias).collect::<HashSet<_>>();
	let rewritten_use_path = remove_aliases_from_use_path(path, &aliases_to_remove);
	let Some(use_edit) = paths::build_use_item_rewrite_edit(
		ctx,
		item,
		rewritten_use_path.as_deref(),
		"RUST-STYLE-IMPORT-003",
	) else {
		return false;
	};

	for (start, end, replacement) in rewrites {
		edits.push(Edit { start, end, replacement, rule: "RUST-STYLE-IMPORT-003" });
	}

	edits.push(use_edit);

	!referenced_aliases.is_empty() || rewritten_use_path != Some(path.to_owned())
}

fn import012_package_contexts(ctx: &FileContext) -> Vec<FileContext> {
	workspace::package_rust_files_for_path(&ctx.path)
		.ok()
		.flatten()
		.map(|(current_path, package_files)| {
			package_files
				.into_iter()
				.filter(|path| *path != current_path)
				.filter_map(|path| shared::read_file_context(&path).ok().flatten())
				.collect::<Vec<_>>()
		})
		.unwrap_or_default()
}

fn import012_keep_alive_root(path: &str, local_module_roots: &HashSet<String>) -> Option<String> {
	let trimmed = path.trim();
	let (base, alias) = trimmed.rsplit_once(" as ")?;

	if alias.trim() != "_" {
		return None;
	}

	let base = base.trim();
	let normalized = syntax::normalize_ident(base);

	if base.is_empty()
		|| base.contains("::")
		|| base.contains('{')
		|| base.contains('}')
		|| base.contains('*')
		|| matches!(normalized, "std" | "core" | "alloc" | "crate" | "self" | "super")
		|| local_module_roots.contains(normalized)
		|| WORKSPACE_IMPORT_ROOTS.contains(normalized)
		|| WORKSPACE_IMPORT_ROOTS.contains(&normalized.replace('-', "_"))
	{
		return None;
	}

	Some(base.to_owned())
}

fn import012_package_contexts_mention_root(package_contexts: &[FileContext], root: &str) -> bool {
	package_contexts
		.iter()
		.any(|package_ctx| import012_context_mentions_root(package_ctx, None, root))
}

fn import012_context_mentions_root(
	ctx: &FileContext,
	excluded_item_start_offset: Option<usize>,
	root: &str,
) -> bool {
	let normalized_root = syntax::normalize_ident(root);

	for item in &ctx.top_items {
		if item.kind != TopKind::Use {
			continue;
		}
		if Some(item.start_offset) == excluded_item_start_offset {
			continue;
		}

		let Some(path) = item.use_path.as_deref() else {
			continue;
		};

		if import012_keep_alive_root_only(path, normalized_root) {
			continue;
		}
		if import012_path_mentions_root(path, normalized_root) {
			return true;
		}
	}
	for path in ctx.source_file.syntax().descendants().filter_map(Path::cast) {
		if path.qualifier().is_some() {
			continue;
		}
		if path.syntax().ancestors().any(|node| Use::can_cast(node.kind())) {
			continue;
		}

		let Some(segment) = path.segment() else {
			continue;
		};
		let Some(name_ref) = segment.name_ref() else {
			continue;
		};

		if syntax::is_same_ident(name_ref.text(), normalized_root) {
			return true;
		}
	}

	false
}

fn import012_keep_alive_root_only(path: &str, root: &str) -> bool {
	let Some((base, alias)) = path.trim().rsplit_once(" as ") else {
		return false;
	};

	alias.trim() == "_" && syntax::is_same_ident(base.trim(), root)
}

fn import012_path_mentions_root(path: &str, root: &str) -> bool {
	let compact = path.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
	let normalized_root = syntax::normalize_ident(root);

	syntax::is_same_ident(compact.as_str(), normalized_root)
		|| compact.starts_with(&format!("{normalized_root}::"))
}

fn remove_aliases_from_use_path(path: &str, aliases_to_remove: &HashSet<String>) -> Option<String> {
	rewrite_use_segment_without_aliases(path.trim(), aliases_to_remove)
}

fn rewrite_use_segment_without_aliases(
	segment: &str,
	aliases_to_remove: &HashSet<String>,
) -> Option<String> {
	let trimmed = segment.trim();

	if trimmed.is_empty() {
		return None;
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
				return None;
			}
			if depth == 0 {
				brace_end = Some(idx);
			}
		}
	}

	if depth != 0 {
		return None;
	}

	if let (Some(open), Some(close)) = (brace_start, brace_end) {
		let prefix = trimmed[..open].trim();
		let inner = &trimmed[open + 1..close];
		let suffix = trimmed[close + 1..].trim();

		if !suffix.is_empty() {
			return None;
		}
		if !prefix.is_empty() && !prefix.ends_with("::") {
			return None;
		}

		let mut rewritten_children = Vec::new();

		for child in paths::split_top_level_csv(inner) {
			if let Some(rewritten_child) =
				rewrite_use_segment_without_aliases(child.as_str(), aliases_to_remove)
			{
				rewritten_children.push(rewritten_child);
			}
		}

		if rewritten_children.is_empty() {
			return None;
		}

		let prefix = prefix.strip_suffix("::").unwrap_or(prefix).trim();

		if prefix.is_empty() {
			return Some(format!("{{{}}}", rewritten_children.join(", ")));
		}

		return Some(format!("{prefix}::{{{}}}", rewritten_children.join(", ")));
	}
	if let Some((base, alias)) = trimmed.rsplit_once(" as ") {
		let base = base.trim();
		let alias = alias.trim();

		if aliases_to_remove.contains(alias) {
			return None;
		}
		if base.is_empty() || alias.is_empty() {
			return None;
		}
	}

	Some(trimmed.to_owned())
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

	let (prefix, close, segments) = paths::parse_braced_path_parts_allow_alias(path)?;
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

		if let Some((head, inner)) = paths::parse_single_level_nested_use_segment(trimmed) {
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

	for child in paths::split_top_level_csv(inner) {
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

	let (base, alias) = paths::split_import_leaf_alias(leaf)?;
	let full_path =
		if import_prefix.is_empty() { base.to_owned() } else { format!("{import_prefix}::{base}") };
	let full_path = full_path.replace(' ', "");
	let symbol = paths::symbol_from_full_import_path(&full_path)?;

	if !paths::looks_like_trait_import(&symbol, &full_path) {
		return None;
	}

	if let Some(alias_name) = alias.as_deref().map(str::trim)
		&& alias_name != "_"
		&& references::symbol_is_referenced_outside_use(ctx, alias_name)
	{
		return None;
	}

	let has_dependent_inline_child = ctx
		.source_file
		.syntax()
		.children()
		.filter_map(Module::cast)
		.any(|module| module.item_list().is_some())
		&& scopes::symbol_referenced_by_child_module(ctx, &symbol);
	let symbol_is_ambiguous = imported_symbol_path_count(ctx, &symbol) > 1;
	let should_keep_alive = (!references::symbol_is_referenced_outside_use(ctx, &symbol)
		|| symbol_is_ambiguous)
		&& (!(has_child_module_declarations || has_dependent_inline_child)
			|| alias.as_deref().map(str::trim) == Some("_"));
	let rewritten = if should_keep_alive { format!("{base} as _") } else { base.to_owned() };
	let changed = paths::compact_path_for_match(leaf) != paths::compact_path_for_match(&rewritten);
	let mut symbols = HashSet::new();

	symbols.insert(symbol.clone());

	Some((
		rewritten,
		symbols,
		changed,
		Some(format!(
			"{}|{}",
			paths::compact_path_for_match(&full_path),
			if should_keep_alive { "_" } else { "" }
		)),
	))
}

fn imported_symbol_path_count(ctx: &FileContext, symbol: &str) -> usize {
	let mut paths = HashSet::new();

	for item in &ctx.top_items {
		if item.kind != TopKind::Use {
			continue;
		}

		let Some(path) = paths::extract_use_path(ctx, item) else {
			continue;
		};

		for full_path in paths::imported_full_paths_from_use_path(&path) {
			if paths::symbol_from_full_import_path(&full_path)
				.is_some_and(|name| syntax::is_same_ident(&name, symbol))
			{
				paths.insert(full_path);
			}
		}
	}

	paths.len()
}

fn has_non_inline_child_modules(ctx: &FileContext) -> bool {
	ctx.top_items.iter().any(|item| item.kind == TopKind::Mod && item.raw.trim_end().ends_with(';'))
}

fn collect_alias_reference_rewrites(
	ctx: &FileContext,
	alias: &str,
	qualified_path: &str,
) -> Vec<(usize, usize, String)> {
	let mut type_rewrites = references::unqualified_type_path_rewrites(ctx, alias, qualified_path);

	type_rewrites.extend(references::alias_root_type_path_rewrites(ctx, alias, qualified_path));

	let mut value_rewrites =
		references::unqualified_value_path_rewrites(ctx, alias, qualified_path);

	value_rewrites.extend(references::alias_root_value_path_rewrites(ctx, alias, qualified_path));

	let root_rewrites = references::alias_root_path_name_ref_rewrites(ctx, alias, qualified_path);
	let macro_rewrites = references::alias_macro_token_tree_rewrites(ctx, alias, qualified_path);
	let use_rewrites = references::dependent_use_root_rewrites(ctx, alias, qualified_path);

	type_rewrites
		.into_iter()
		.chain(value_rewrites)
		.chain(root_rewrites)
		.chain(macro_rewrites)
		.chain(use_rewrites)
		.collect()
}
