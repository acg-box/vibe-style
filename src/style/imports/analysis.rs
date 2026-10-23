use std::collections::{HashMap, HashSet};

use ra_ap_syntax::{
	AstNode as _,
	ast::{Path, PathType},
};

use crate::style::{
	imports::{
		ImportAnalysis, ImportedSymbolMaps, PathQualificationRequirement, UseItemAnalysis, groups,
		paths, references,
	},
	shared::{FileContext, TopItem, TopKind},
	syntax,
};

pub(super) fn build_import_analysis<'a>(
	ctx: &FileContext,
	use_runs: Vec<Vec<&'a TopItem>>,
	use_items: Vec<&'a TopItem>,
) -> ImportAnalysis<'a> {
	let use_item_analyses = collect_use_item_analyses(ctx, &use_items);
	let all_use_items =
		ctx.top_items.iter().filter(|item| item.kind == TopKind::Use).collect::<Vec<_>>();
	let all_use_item_analyses = collect_use_item_analyses(ctx, &all_use_items);
	let use_item_analysis_by_key = use_item_analyses
		.iter()
		.enumerate()
		.map(|(idx, analysis)| (paths::use_item_lock_key(analysis.item), idx))
		.collect::<HashMap<_, _>>();

	ImportAnalysis {
		// A public re-export also introduces the symbol into the local module namespace. Keep
		// public uses out of non-public grouping edits, but include them when deciding whether a
		// qualified path already has an import.
		imported_symbol_maps: collect_imported_symbol_maps(&all_use_item_analyses),
		local_defined_symbols: collect_local_defined_symbols(ctx),
		local_module_roots: groups::collect_local_module_roots(ctx),
		qualified_type_paths_by_symbol: collect_qualified_type_paths_by_symbol(ctx),
		qualified_value_paths_by_symbol: collect_qualified_value_paths_by_symbol(ctx),
		use_item_analysis_by_key,
		use_item_analyses,
		use_runs,
	}
}

pub(super) fn collect_use_item_analyses<'a>(
	ctx: &FileContext,
	use_items: &[&'a TopItem],
) -> Vec<UseItemAnalysis<'a>> {
	let mut analyses = Vec::with_capacity(use_items.len());

	for item in use_items {
		let Some(path) = paths::extract_use_path(ctx, item) else {
			continue;
		};
		let imported_symbols = paths::imported_symbols_from_use_path(&path);
		let imported_symbol_set = imported_symbols
			.iter()
			.map(|symbol| syntax::normalize_ident(symbol).to_owned())
			.collect::<HashSet<_>>();

		analyses.push(UseItemAnalysis {
			imported_full_paths: paths::imported_full_paths_from_use_path(&path),
			imported_self_full_paths: paths::imported_self_full_paths_from_use_path(&path),
			imported_symbol_set,
			imported_symbols,
			item,
			path,
		});
	}

	analyses
}

pub(super) fn collect_imported_symbol_maps(
	use_item_analyses: &[UseItemAnalysis<'_>],
) -> ImportedSymbolMaps {
	let mut maps = ImportedSymbolMaps::default();

	for use_item_analysis in use_item_analyses {
		for symbol in &use_item_analysis.imported_symbols {
			maps.symbol_paths
				.entry(symbol.clone())
				.or_default()
				.insert(use_item_analysis.path.clone());
			maps.symbol_lines
				.entry(symbol.clone())
				.or_default()
				.insert(use_item_analysis.item.line);
		}
		for full_path in &use_item_analysis.imported_full_paths {
			let Some(symbol) = paths::symbol_from_full_import_path(full_path) else {
				continue;
			};

			if !use_item_analysis.imported_symbol_set.contains(syntax::normalize_ident(&symbol))
				&& !use_item_analysis.imported_self_full_paths.contains(full_path)
			{
				continue;
			}

			maps.full_paths_by_symbol.entry(symbol).or_default().insert(full_path.clone());
		}
	}

	maps
}

pub(super) fn collect_local_defined_symbols(ctx: &FileContext) -> HashSet<String> {
	let mut out = HashSet::new();

	for item in &ctx.top_items {
		let Some(name) = item.name.as_deref() else {
			continue;
		};

		out.insert(syntax::normalize_ident(name).to_owned());
	}

	out
}

pub(super) fn collect_qualified_type_paths_by_symbol(
	ctx: &FileContext,
) -> HashMap<String, HashSet<String>> {
	let mut out: HashMap<String, HashSet<String>> = HashMap::new();

	for path_type in ctx.source_file.syntax().descendants().filter_map(PathType::cast) {
		let Some(candidate) = path_type.path().and_then(|path| {
			references::classify_type_path_candidate(
				ctx,
				path,
				PathQualificationRequirement::Qualified,
			)
		}) else {
			continue;
		};
		let mut segments = Vec::new();

		if !paths::collect_path_segment_texts(&candidate.path, &mut segments) {
			continue;
		}
		if segments.len() < 2 {
			continue;
		}

		let root = segments[0].as_str();

		if paths::is_non_importable_root(root) {
			continue;
		}

		let full_path = segments.join("::");
		let Some(symbol) = paths::symbol_from_full_import_path(&full_path) else {
			continue;
		};

		out.entry(symbol).or_default().insert(full_path);
	}

	out
}

pub(super) fn collect_qualified_value_paths_by_symbol(
	ctx: &FileContext,
) -> HashMap<String, HashSet<String>> {
	let mut out: HashMap<String, HashSet<String>> = HashMap::new();

	for path in ctx.source_file.syntax().descendants().filter_map(Path::cast) {
		let Some(candidate) = references::classify_value_path_candidate(
			ctx,
			path,
			PathQualificationRequirement::Qualified,
			false,
			true,
		) else {
			continue;
		};
		let mut segments = Vec::new();

		if !paths::collect_path_segment_texts(&candidate.path, &mut segments) {
			continue;
		}
		if segments.len() < 2 {
			continue;
		}

		let root = segments[0].as_str();

		if paths::is_non_importable_root(root) {
			continue;
		}

		let full_path = segments.join("::");
		let Some(symbol) = paths::symbol_from_full_import_path(&full_path) else {
			continue;
		};

		out.entry(symbol).or_default().insert(full_path);

		if let Some((receiver_symbol, receiver_path)) =
			references::qualified_associated_receiver_symbol_path(&segments)
		{
			out.entry(receiver_symbol).or_default().insert(receiver_path);
		}
	}

	out
}
