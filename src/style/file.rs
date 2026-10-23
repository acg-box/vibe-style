use std::collections::{HashMap, HashSet};

use ra_ap_syntax::{
	AstNode,
	ast::{MacroCall, Path, Use},
};

use crate::style::{
	shared::{self, Edit, FileContext, TopItem, TopKind, Violation},
	syntax,
};

pub(crate) fn check_mod_rs(ctx: &FileContext, violations: &mut Vec<Violation>) {
	if ctx.path.file_name().is_some_and(|name| name == "mod.rs") {
		shared::push_violation(
			violations,
			ctx,
			1,
			"RUST-STYLE-FILE-001",
			"Do not use mod.rs. Use flat module files instead.",
			false,
		);
	}
}

pub(crate) fn check_error_rs_no_use(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	if ctx.path.file_name().is_none_or(|name| name != "error.rs") {
		return;
	}

	let use_items =
		ctx.top_items.iter().filter(|item| item.kind == TopKind::Use).collect::<Vec<_>>();

	if use_items.is_empty() {
		return;
	}

	let local_defined_symbols = ctx
		.top_items
		.iter()
		.filter_map(|item| item.name.as_deref())
		.map(|name| syntax::normalize_ident(name).to_owned())
		.chain(
			syntax::generic_parameter_names(ctx.source_file.syntax())
				.map(|name| syntax::normalize_ident(&name).to_owned()),
		)
		.collect::<HashSet<_>>();
	let mut symbol_paths: HashMap<String, HashSet<String>> = HashMap::new();
	let mut symbol_ranges: HashMap<String, Vec<(usize, usize, String)>> = HashMap::new();
	let mut parse_failed = false;

	for item in &use_items {
		let Some(path) = extract_use_path(item) else {
			parse_failed = true;

			continue;
		};
		let Some(bindings) = collect_import_bindings_from_use_path(&path) else {
			parse_failed = true;

			continue;
		};

		if bindings.is_empty() {
			parse_failed = true;

			continue;
		}

		for (symbol, full_path) in bindings {
			symbol_paths.entry(symbol).or_default().insert(full_path);
		}
	}

	let mut fixable = !parse_failed;

	for symbol in symbol_paths.keys() {
		let Some(full_path) =
			symbol_paths.get(symbol).and_then(|paths| paths.iter().next()).cloned()
		else {
			fixable = false;

			continue;
		};
		let mut ranges = unqualified_path_rewrites(ctx, symbol, &full_path);

		ranges.extend(unqualified_macro_call_rewrites(ctx, symbol, &full_path));
		ranges.sort_by_key(|(start, end, _)| (*start, *end));
		ranges.dedup();

		if !ranges.is_empty()
			&& (local_defined_symbols.contains(symbol)
				|| symbol_paths.get(symbol).is_none_or(|paths| paths.len() != 1))
		{
			fixable = false;
		}

		symbol_ranges.insert(symbol.clone(), ranges);
	}
	for item in &use_items {
		shared::push_violation(
			violations,
			ctx,
			item.line,
			"RUST-STYLE-IMPORT-005",
			"Do not add use imports in error.rs; use fully qualified paths.",
			fixable,
		);
	}

	if !emit_edits || !fixable {
		return;
	}

	for (_symbol, ranges) in symbol_ranges {
		if ranges.is_empty() {
			continue;
		}

		for (start, end, replacement) in ranges {
			edits.push(Edit { start, end, replacement, rule: "RUST-STYLE-IMPORT-005" });
		}
	}
	for item in use_items {
		let Some((start, end)) = shared::item_text_range(ctx, item.start_offset, item.end_offset)
		else {
			continue;
		};

		edits.push(Edit { start, end, replacement: String::new(), rule: "RUST-STYLE-IMPORT-005" });
	}
}

fn extract_use_path(item: &TopItem) -> Option<String> {
	syntax::find_use_path_range(&item.raw)
		.and_then(|(start, end)| item.raw.get(start..end).map(|s| s.trim().to_owned()))
}

fn collect_import_bindings_from_use_path(path: &str) -> Option<Vec<(String, String)>> {
	let bindings = syntax::parse_use_bindings(path)?;

	if bindings.iter().any(|binding| binding.is_glob) {
		return None;
	}

	Some(
		bindings
			.into_iter()
			.filter_map(|binding| {
				let symbol = binding.name?;

				Some((syntax::normalize_ident(&symbol).to_owned(), binding.path.join("::")))
			})
			.collect(),
	)
}

fn unqualified_path_rewrites(
	ctx: &FileContext,
	symbol: &str,
	qualified_path: &str,
) -> Vec<(usize, usize, String)> {
	let mut rewrites = Vec::new();

	for path in ctx.source_file.syntax().descendants().filter_map(Path::cast) {
		if path.qualifier().is_some() {
			continue;
		}
		if path.syntax().ancestors().any(|node| Use::cast(node).is_some()) {
			continue;
		}

		let Some(segment) = path.segment() else {
			continue;
		};
		let Some(name_ref) = segment.name_ref() else {
			continue;
		};

		if !syntax::is_same_ident(name_ref.text(), symbol) {
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

fn unqualified_macro_call_rewrites(
	ctx: &FileContext,
	symbol: &str,
	qualified_path: &str,
) -> Vec<(usize, usize, String)> {
	let mut rewrites = Vec::new();

	for macro_call in ctx.source_file.syntax().descendants().filter_map(MacroCall::cast) {
		let Some(path) = macro_call.path() else {
			continue;
		};

		if path.qualifier().is_some() {
			continue;
		}

		let Some(segment) = path.segment() else {
			continue;
		};
		let Some(name_ref) = segment.name_ref() else {
			continue;
		};

		if !syntax::is_same_ident(name_ref.text(), symbol) {
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
