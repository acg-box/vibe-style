use std::{
	collections::{BTreeSet, HashMap, HashSet},
	sync::LazyLock,
};

use ra_ap_syntax::{
	AstNode as _,
	ast::{Attr, HasAttrs as _, MacroCall, Module, Path, PathType},
};
use regex::Regex;

use crate::style::{
	imports::{
		Import008Candidate, Import008CandidateKind, Import008UseRecoveryCandidate, ImportAnalysis,
		PathQualificationRequirement, UseItemAnalysis, groups, paths, references,
	},
	shared::{self, Edit, FileContext, Violation},
	syntax,
};

static DERIVE_PATH_RE: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(
		r"(?:[A-Za-z_][A-Za-z0-9_]*|r#[A-Za-z_][A-Za-z0-9_]*)\s*(?:::\s*(?:[A-Za-z_][A-Za-z0-9_]*|r#[A-Za-z_][A-Za-z0-9_]*)\s*)+",
	).expect("Compile derive path regex.")
});

#[allow(clippy::too_many_arguments)]
pub(super) fn apply_import008_rules(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	analysis: &ImportAnalysis<'_>,
) -> HashSet<usize> {
	let import008_candidates = collect_import008_candidates(ctx);
	let import008_use_recovery_candidates = collect_import008_use_recovery_candidates(
		&analysis.use_item_analyses,
		&analysis.imported_symbol_maps.full_paths_by_symbol,
	);
	let blocked_symbols = build_import008_blocked_symbols(
		ctx,
		&import008_candidates,
		&analysis.imported_symbol_maps.full_paths_by_symbol,
		&analysis.local_defined_symbols,
		&analysis.qualified_type_paths_by_symbol,
		&analysis.qualified_value_paths_by_symbol,
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
		&analysis.imported_symbol_maps.full_paths_by_symbol,
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
		&analysis.local_defined_symbols,
		&analysis.imported_symbol_maps.full_paths_by_symbol,
		&mut pending_import_paths,
		&mut import008_group_skip_lines,
	);

	if emit_edits {
		import008_group_skip_lines.extend(groups::apply_pending_module_import_path_edits(
			ctx,
			edits,
			&analysis.use_runs,
			&analysis.local_module_roots,
			&pending_import_paths,
			&analysis.imported_symbol_maps.full_paths_by_symbol,
			"RUST-STYLE-IMPORT-008",
		));
	}

	import008_group_skip_lines
}

fn build_import008_blocked_symbols(
	ctx: &FileContext,
	import008_candidates: &[Import008Candidate],
	imported_full_paths_by_symbol: &HashMap<String, HashSet<String>>,
	local_defined_symbols: &HashSet<String>,
	qualified_type_paths_by_symbol: &HashMap<String, HashSet<String>>,
	qualified_value_paths_by_symbol: &HashMap<String, HashSet<String>>,
) -> HashSet<String> {
	let unqualified_type_symbols = collect_unqualified_type_symbols(ctx);
	let mut candidate_paths_by_symbol: HashMap<String, HashSet<String>> = HashMap::new();
	let mut non_value_receiver_candidate_symbols = HashSet::new();
	let mut type_candidate_paths_by_symbol: HashMap<String, HashSet<String>> = HashMap::new();

	for candidate in import008_candidates {
		candidate_paths_by_symbol
			.entry(candidate.symbol.clone())
			.or_default()
			.insert(candidate.import_path.clone());

		if candidate.kind == Import008CandidateKind::TypePath {
			type_candidate_paths_by_symbol
				.entry(candidate.symbol.clone())
				.or_default()
				.insert(candidate.import_path.clone());
		}
		if !matches!(
			candidate.kind,
			Import008CandidateKind::ValueReceiver | Import008CandidateKind::ValuePath
		) {
			non_value_receiver_candidate_symbols.insert(candidate.symbol.clone());
		}
	}

	let mut blocked_symbols = HashSet::new();

	for (symbol, candidate_paths) in &candidate_paths_by_symbol {
		let imported_paths = imported_full_paths_by_symbol.get(symbol).cloned().unwrap_or_default();
		let introduces_short_name =
			candidate_paths.iter().any(|path| !imported_paths.contains(path));
		let mut all_paths = imported_paths;

		all_paths.extend(candidate_paths.iter().cloned());

		if let Some(type_paths) = qualified_type_paths_by_symbol.get(symbol) {
			all_paths.extend(type_paths.iter().cloned());
		}
		if let Some(value_paths) = qualified_value_paths_by_symbol.get(symbol) {
			all_paths.extend(value_paths.iter().cloned());
		}

		let has_non_value_receiver_candidate =
			non_value_receiver_candidate_symbols.contains(symbol);
		let has_explicit_type_value_role_collision =
			type_candidate_paths_by_symbol.get(symbol).is_some_and(|type_paths| {
				qualified_value_paths_by_symbol.get(symbol).is_some_and(|value_paths| {
					type_paths.iter().any(|path| value_paths.contains(path))
				})
			});
		let import009_consistency_conflict = has_explicit_type_value_role_collision
			|| (has_non_value_receiver_candidate
				&& references::is_import009_type_like_symbol(symbol)
				&& qualified_value_paths_by_symbol.get(symbol).is_some_and(|value_paths| {
					candidate_paths.iter().any(|path| value_paths.contains(path))
				}));

		if all_paths.len() > 1
			|| local_defined_symbols.contains(symbol)
			|| (introduces_short_name && unqualified_type_symbols.contains(symbol))
			|| import009_consistency_conflict
		{
			blocked_symbols.insert(symbol.clone());
		}
	}

	blocked_symbols
}

fn collect_unqualified_type_symbols(ctx: &FileContext) -> HashSet<String> {
	ctx.source_file
		.syntax()
		.descendants()
		.filter_map(PathType::cast)
		.filter_map(|path_type| {
			path_type.path().and_then(|path| {
				references::classify_type_path_candidate(
					ctx,
					path,
					PathQualificationRequirement::Unqualified,
				)
			})
		})
		.map(|candidate| syntax::normalize_ident(candidate.name_ref.text()).to_owned())
		.collect()
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

			let target_compact = paths::compact_path_for_match(&candidate.import_path);
			let already_imported =
				imported_full_paths_by_symbol.get(&candidate.symbol).is_some_and(|paths| {
					paths.iter().any(|path| paths::compact_path_for_match(path) == target_compact)
				});

			if !already_imported {
				pending_import_paths.insert(candidate.import_path.clone());
			}
		}
	}
}

fn collect_import008_use_recovery_candidates(
	use_item_analyses: &[UseItemAnalysis<'_>],
	imported_full_paths_by_symbol: &HashMap<String, HashSet<String>>,
) -> Vec<Import008UseRecoveryCandidate> {
	let mut candidates = Vec::new();

	for use_item_analysis in use_item_analyses {
		let Some((prefix, symbol)) = paths::simple_import_prefix_symbol(&use_item_analysis.path)
		else {
			continue;
		};

		if prefix.contains("::") {
			continue;
		}
		if matches!(prefix.as_str(), "std" | "core" | "alloc" | "crate" | "self" | "super" | "Self")
		{
			continue;
		}

		let root = syntax::normalize_ident(&prefix).to_owned();
		let symbol_normalized = syntax::normalize_ident(&symbol).to_owned();
		let Some(root_full_paths) = imported_full_paths_by_symbol.get(&root) else {
			continue;
		};

		if root_full_paths.len() != 1 {
			continue;
		}

		let Some(root_full) = root_full_paths.iter().next() else {
			continue;
		};
		let root_full_compact = paths::compact_path_for_match(root_full);
		let prefix_compact = paths::compact_path_for_match(&prefix);

		if root_full_compact == prefix_compact {
			continue;
		}

		let import_path = format!("{root_full}::{symbol}");
		let current_compact = paths::compact_path_for_match(&use_item_analysis.path);
		let import_path_compact = paths::compact_path_for_match(&import_path);

		if let Some(existing_symbol_paths) = imported_full_paths_by_symbol.get(&symbol_normalized)
			&& existing_symbol_paths.iter().any(|existing| {
				let existing_compact = paths::compact_path_for_match(existing);

				existing_compact != current_compact && existing_compact != import_path_compact
			}) {
			continue;
		}

		candidates.push(Import008UseRecoveryCandidate {
			line: use_item_analysis.item.line,
			start_line: use_item_analysis.item.start_line,
			end_line: use_item_analysis.item.end_line,
			symbol: symbol_normalized,
			import_path,
		});
	}

	candidates
}

fn collect_import008_candidates(ctx: &FileContext) -> Vec<Import008Candidate> {
	let mut candidates = Vec::new();
	let mut seen_ranges = HashSet::new();

	collect_import008_from_paths(ctx, &mut candidates, &mut seen_ranges);
	collect_import008_from_type_like_value_paths(ctx, &mut candidates, &mut seen_ranges);
	collect_import008_from_value_receivers(ctx, &mut candidates, &mut seen_ranges);
	collect_import008_from_macro_calls(ctx, &mut candidates, &mut seen_ranges);
	collect_import008_from_derive_attrs(ctx, &DERIVE_PATH_RE, &mut candidates, &mut seen_ranges);

	candidates
}

fn collect_import008_from_paths(
	ctx: &FileContext,
	candidates: &mut Vec<Import008Candidate>,
	seen_ranges: &mut HashSet<(usize, usize)>,
) {
	let mut guarded_candidates = Vec::new();

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
		let guarded = paths::syntax_is_inside_cfg_guarded_scope(candidate.path.syntax());
		let mut segments = Vec::new();

		if !paths::collect_path_segment_texts(&candidate.path, &mut segments) {
			continue;
		}
		if segments.len() < 2 {
			continue;
		}

		let symbol = syntax::normalize_ident(segments[segments.len() - 1].as_str()).to_owned();

		if matches!(symbol.as_str(), "" | "self" | "super" | "crate" | "Self") {
			continue;
		}

		let root = segments[0].as_str();

		if paths::is_non_importable_root(root) {
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

		let replacement = candidate.name_ref.text().to_string();
		let range = candidate.path.syntax().text_range();
		let start = usize::from(range.start());
		let end = usize::from(candidate.name_ref.syntax().text_range().end());

		if start >= end || !seen_ranges.insert((start, end)) {
			continue;
		}

		let line = shared::line_from_offset(&ctx.line_starts, start);
		let candidate = Import008Candidate {
			line,
			start,
			end,
			kind: Import008CandidateKind::TypePath,
			symbol,
			import_path,
			replacement,
		};

		if guarded {
			guarded_candidates.push(candidate);
		} else {
			candidates.push(candidate);
		}
	}

	let unconditional_paths =
		candidates.iter().map(|candidate| candidate.import_path.clone()).collect::<HashSet<_>>();

	// A shared unconditional use already requires this import in every configuration.
	// Shorten its guarded occurrences too, without hoisting guarded-only dependencies.
	candidates.extend(
		guarded_candidates
			.into_iter()
			.filter(|candidate| unconditional_paths.contains(&candidate.import_path)),
	);
}

fn collect_import008_from_type_like_value_paths(
	ctx: &FileContext,
	candidates: &mut Vec<Import008Candidate>,
	seen_ranges: &mut HashSet<(usize, usize)>,
) {
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

		if paths::syntax_is_inside_cfg_guarded_scope(candidate.path.syntax()) {
			continue;
		}

		let mut segments = Vec::new();

		if !paths::collect_path_segment_texts(&candidate.path, &mut segments) {
			continue;
		}
		if segments.len() < 2 {
			continue;
		}

		let symbol = syntax::normalize_ident(segments[segments.len() - 1].as_str()).to_owned();

		if !references::is_import009_type_like_symbol(&symbol) {
			continue;
		}
		if segments.get(segments.len().saturating_sub(2)).is_some_and(|segment| {
			references::is_import009_type_like_symbol(syntax::normalize_ident(segment))
		}) {
			continue;
		}

		let root = segments[0].as_str();

		if paths::is_non_importable_root(root) {
			continue;
		}
		if is_primitive_associated_value_path(&segments) {
			continue;
		}

		let import_path = segments.join("::");
		let range = candidate.path.syntax().text_range();
		let start = usize::from(range.start());
		let end = usize::from(candidate.name_ref.syntax().text_range().end());

		if start >= end || !seen_ranges.insert((start, end)) {
			continue;
		}

		let line = shared::line_from_offset(&ctx.line_starts, start);

		candidates.push(Import008Candidate {
			line,
			start,
			end,
			kind: Import008CandidateKind::ValuePath,
			symbol,
			import_path,
			replacement: candidate.name_ref.text().to_string(),
		});
	}
}

fn is_primitive_associated_value_path(segments: &[String]) -> bool {
	match segments {
		[receiver, _] => paths::is_rust_primitive_type_ident(receiver),
		[root, primitive_module, receiver, _]
			if matches!(root.as_str(), "core" | "std") && primitive_module == "primitive" =>
			paths::is_rust_primitive_type_ident(receiver),
		_ => false,
	}
}

fn collect_import008_from_value_receivers(
	ctx: &FileContext,
	candidates: &mut Vec<Import008Candidate>,
	seen_ranges: &mut HashSet<(usize, usize)>,
) {
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

		if paths::syntax_is_inside_cfg_guarded_scope(candidate.path.syntax()) {
			continue;
		}

		let mut segments = Vec::new();

		if !paths::collect_path_segment_texts(&candidate.path, &mut segments) {
			continue;
		}

		let Some((symbol, import_path)) =
			references::qualified_associated_receiver_symbol_path(&segments)
		else {
			continue;
		};
		let Some(receiver_leaf) = segments.get(segments.len().saturating_sub(2)) else {
			continue;
		};
		let Some(receiver_name) = candidate
			.path
			.qualifier()
			.and_then(|receiver| receiver.segment())
			.and_then(|segment| segment.name_ref())
		else {
			continue;
		};

		if matches!(symbol.as_str(), "" | "self" | "super" | "crate" | "Self") {
			continue;
		}

		let root = segments[0].as_str();

		if paths::is_non_importable_root(root) {
			continue;
		}

		let replacement = receiver_leaf.clone();
		let range = candidate.path.syntax().text_range();
		let start = usize::from(range.start());
		let end = usize::from(receiver_name.syntax().text_range().end());

		if start >= end || !seen_ranges.insert((start, end)) {
			continue;
		}

		let line = shared::line_from_offset(&ctx.line_starts, start);

		candidates.push(Import008Candidate {
			line,
			start,
			end,
			kind: Import008CandidateKind::ValueReceiver,
			symbol,
			import_path,
			replacement,
		});
	}
}

fn collect_import008_from_macro_record_receivers(
	ctx: &FileContext,
	candidates: &mut Vec<Import008Candidate>,
	seen_ranges: &mut HashSet<(usize, usize)>,
	tree_text: &str,
	tree_start: usize,
) {
	if !tree_text.contains("::") || !tree_text.contains('{') {
		return;
	}

	for found in paths::qualified_macro_path_regex().find_iter(tree_text) {
		if !references::macro_symbol_has_record_follow(tree_text, found.end())
			|| !references::macro_symbol_is_import009_unqualified(
				ctx,
				tree_text,
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

		if segments.len() < 3 {
			continue;
		}

		let root = segments[0].as_str();

		if paths::is_non_importable_root(root) {
			continue;
		}

		let receiver_leaf = segments[segments.len() - 2].clone();
		let symbol = syntax::normalize_ident(&receiver_leaf).to_owned();

		if !references::is_import009_type_like_symbol(&symbol) {
			continue;
		}

		let Some(leaf) = segments.last().cloned() else {
			continue;
		};
		let import_path = segments[..segments.len() - 1].join("::");
		let replacement = format!("{receiver_leaf}::{leaf}");
		let start = tree_start + found.start();
		let end = tree_start + found.end();

		if start >= end || !seen_ranges.insert((start, end)) {
			continue;
		}

		let line = shared::line_from_offset(&ctx.line_starts, start);

		candidates.push(Import008Candidate {
			line,
			start,
			end,
			kind: Import008CandidateKind::ValueReceiver,
			symbol,
			import_path,
			replacement,
		});
	}
}

fn collect_import008_from_macro_calls(
	ctx: &FileContext,
	candidates: &mut Vec<Import008Candidate>,
	seen_ranges: &mut HashSet<(usize, usize)>,
) {
	for macro_call in ctx.source_file.syntax().descendants().filter_map(MacroCall::cast) {
		if paths::syntax_is_inside_cfg_guarded_scope(macro_call.syntax())
			|| paths::syntax_is_inside_cfg_test_module(macro_call.syntax())
		{
			continue;
		}

		if let Some(token_tree) = macro_call.token_tree() {
			let tree_text = token_tree.syntax().text().to_string();
			let tree_start = usize::from(token_tree.syntax().text_range().start());

			collect_import008_from_macro_record_receivers(
				ctx,
				candidates,
				seen_ranges,
				&tree_text,
				tree_start,
			);
		}

		let Some(path) = macro_call.path() else {
			continue;
		};

		if path.qualifier().is_none() {
			continue;
		}

		let mut segments = Vec::new();

		if !paths::collect_path_segment_texts(&path, &mut segments) {
			continue;
		}
		if segments.len() < 3 {
			continue;
		}

		let module_name = segments[segments.len() - 2].clone();
		let macro_name = segments[segments.len() - 1].clone();

		if !syntax::is_same_ident(&module_name, &macro_name) {
			continue;
		}

		let symbol = syntax::normalize_ident(&module_name).to_owned();

		if matches!(symbol.as_str(), "" | "self" | "super" | "crate" | "Self") {
			continue;
		}

		let root = segments[0].as_str();

		if paths::is_non_importable_root(root) {
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

		candidates.push(Import008Candidate {
			line,
			start,
			end,
			kind: Import008CandidateKind::MacroModule,
			symbol,
			import_path,
			replacement,
		});
	}
}

fn collect_import008_from_derive_attrs(
	ctx: &FileContext,
	derive_path_re: &Regex,
	candidates: &mut Vec<Import008Candidate>,
	seen_ranges: &mut HashSet<(usize, usize)>,
) {
	for attr in ctx.source_file.syntax().descendants().filter_map(Attr::cast) {
		let Some(attr_path) = attr.path() else {
			continue;
		};
		let Some(attr_name) = attr_path.segment().and_then(|segment| segment.name_ref()) else {
			continue;
		};

		if attr_name.text() != "derive" {
			continue;
		}
		if paths::syntax_is_inside_cfg_guarded_scope(attr.syntax()) {
			continue;
		}

		let is_cfg_test = attr.syntax().ancestors().filter_map(Module::cast).any(|module| {
			module
				.attrs()
				.any(|attr| attr.syntax().text().to_string().replace(' ', "").contains("cfg(test)"))
		});

		if is_cfg_test {
			continue;
		}

		let attr_text = attr.syntax().text().to_string();
		let attr_start = usize::from(attr.syntax().text_range().start());
		let mut derive_body = None;

		if let Some(open_paren_offset) = attr_text.find('(') {
			let body_start = open_paren_offset + 1;
			let mut depth = 1_usize;
			let mut close_offset = None;

			for (idx, ch) in attr_text.as_bytes().iter().enumerate().skip(body_start) {
				match ch {
					b'(' => depth += 1,
					b')' => {
						depth -= 1;

						if depth == 0 {
							close_offset = Some(idx);

							break;
						}
					},
					_ => {},
				}
			}

			if let Some(close_offset) = close_offset
				&& close_offset >= body_start
			{
				derive_body = Some((body_start, close_offset));
			}
		}

		let Some((body_start, body_end)) = derive_body else {
			continue;
		};
		let body = &attr_text[body_start..body_end];
		let body_offset = body_start;

		for derive_match in derive_path_re.find_iter(body) {
			let compact_match =
				derive_match.as_str().chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
			let segments = compact_match.split("::").map(ToString::to_string).collect::<Vec<_>>();

			if segments.len() < 2 {
				continue;
			}

			let symbol = syntax::normalize_ident(segments[segments.len() - 1].as_str()).to_owned();

			if matches!(symbol.as_str(), "" | "self" | "super" | "crate" | "Self") {
				continue;
			}

			let root = syntax::normalize_ident(segments[0].as_str());

			if paths::is_non_importable_root(root) {
				continue;
			}

			let import_path = compact_match;

			if matches!(
				import_path.as_str(),
				"std::fmt::Result"
					| "core::fmt::Result"
					| "std::result::Result"
					| "core::result::Result"
			) {
				continue;
			}

			let replacement = segments[segments.len() - 1].clone();
			let start = attr_start + body_offset + derive_match.start();
			let end = attr_start + body_offset + derive_match.end();

			if start >= end || !seen_ranges.insert((start, end)) {
				continue;
			}

			let line = shared::line_from_offset(&ctx.line_starts, start);

			candidates.push(Import008Candidate {
				line,
				start,
				end,
				kind: Import008CandidateKind::Derive,
				symbol,
				import_path,
				replacement,
			});
		}
	}
}
