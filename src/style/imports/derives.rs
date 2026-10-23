use std::collections::{HashMap, HashSet};

use ra_ap_syntax::{
	AstNode as _,
	ast::{Attr, Meta},
};

use crate::style::{
	imports::{groups, paths},
	shared::{self, Edit, FileContext, Violation},
	syntax,
};

struct DeriveOrderEntry {
	display_text: String,
	origin: usize,
	original_index: usize,
}

struct DeriveOrderCandidate {
	line: usize,
	start: usize,
	end: usize,
	is_inner: bool,
	entries: Vec<DeriveOrderEntry>,
}

pub(super) fn apply_import011_derive_order_rule(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	local_module_roots: &HashSet<String>,
	local_defined_symbols: &HashSet<String>,
	imported_full_paths_by_symbol: &HashMap<String, HashSet<String>>,
) {
	let locked_ranges = collect_locked_derive_attr_ranges(edits);

	for candidate in collect_import011_candidates(
		ctx,
		local_module_roots,
		local_defined_symbols,
		imported_full_paths_by_symbol,
	) {
		if candidate.entries.len() < 2
			|| locked_ranges
				.iter()
				.any(|(start, end)| ranges_overlap(candidate.start, candidate.end, *start, *end))
		{
			continue;
		}

		let mut ordered = candidate.entries.iter().collect::<Vec<_>>();

		ordered.sort_by(|left, right| {
			left.origin
				.cmp(&right.origin)
				.then(left.display_text.cmp(&right.display_text))
				.then(left.original_index.cmp(&right.original_index))
		});

		if ordered.iter().enumerate().all(|(idx, entry)| entry.original_index == idx) {
			continue;
		}

		shared::push_violation(
			violations,
			ctx,
			candidate.line,
			"RUST-STYLE-IMPORT-011",
			"Order `#[derive(...)]` entries like imports: std/core/alloc, third-party, then workspace; alphabetize within each group.",
			true,
		);

		if !emit_edits {
			continue;
		}

		let prefix = if candidate.is_inner { "#![derive(" } else { "#[derive(" };
		let replacement = format!(
			"{prefix}{})]",
			ordered.iter().map(|entry| entry.display_text.as_str()).collect::<Vec<_>>().join(", ")
		);

		edits.push(Edit {
			start: candidate.start,
			end: candidate.end,
			replacement,
			rule: "RUST-STYLE-IMPORT-011",
		});
	}
}

fn collect_import011_candidates(
	ctx: &FileContext,
	local_module_roots: &HashSet<String>,
	local_defined_symbols: &HashSet<String>,
	imported_full_paths_by_symbol: &HashMap<String, HashSet<String>>,
) -> Vec<DeriveOrderCandidate> {
	let mut candidates = Vec::new();

	for attr in ctx.source_file.syntax().descendants().filter_map(Attr::cast) {
		let Some(meta) = attr.meta() else {
			continue;
		};
		let Some(meta_path) = meta.path() else {
			continue;
		};
		let Some(meta_name) = meta_path.segment().and_then(|segment| segment.name_ref()) else {
			continue;
		};

		if meta_name.text() != "derive" {
			continue;
		}

		let Some(token_tree) = (match meta {
			Meta::TokenTreeMeta(meta) => meta.token_tree(),
			_ => None,
		}) else {
			continue;
		};
		let tree_text = token_tree.syntax().text().to_string();
		let body = tree_text
			.strip_prefix('(')
			.and_then(|value| value.strip_suffix(')'))
			.unwrap_or_default();

		if body.trim().is_empty() {
			continue;
		}

		let mut entries = Vec::new();
		let mut resolvable = true;

		for (idx, segment) in paths::split_top_level_csv(body).into_iter().enumerate() {
			let display_text = paths::compact_path_for_match(segment.trim());

			if display_text.is_empty() {
				resolvable = false;

				break;
			}

			let Some(origin) = derive_entry_origin(
				&display_text,
				local_module_roots,
				local_defined_symbols,
				imported_full_paths_by_symbol,
			) else {
				resolvable = false;

				break;
			};

			entries.push(DeriveOrderEntry { display_text, origin, original_index: idx });
		}

		if !resolvable || entries.len() < 2 {
			continue;
		}

		let range = attr.syntax().text_range();
		let start = usize::from(range.start());
		let end = usize::from(range.end());

		if start >= end {
			continue;
		}

		candidates.push(DeriveOrderCandidate {
			line: shared::line_from_offset(&ctx.line_starts, start),
			start,
			end,
			is_inner: attr.syntax().text().to_string().starts_with("#!["),
			entries,
		});
	}

	candidates
}

fn derive_entry_origin(
	display_text: &str,
	local_module_roots: &HashSet<String>,
	local_defined_symbols: &HashSet<String>,
	imported_full_paths_by_symbol: &HashMap<String, HashSet<String>>,
) -> Option<usize> {
	if display_text.contains("::") {
		let root = display_text.split("::").next().unwrap_or_default();

		if paths::is_non_importable_root(root) {
			return None;
		}

		return Some(groups::use_origin(display_text, local_module_roots));
	}

	let symbol = syntax::normalize_ident(display_text);

	if is_std_builtin_derive(symbol) {
		return Some(0);
	}
	if local_defined_symbols.contains(symbol) {
		return None;
	}

	let imported_paths = imported_full_paths_by_symbol.get(symbol)?;

	if imported_paths.len() != 1 {
		return None;
	}

	imported_paths.iter().next().map(|path| groups::use_origin(path, local_module_roots))
}

fn is_std_builtin_derive(symbol: &str) -> bool {
	matches!(
		syntax::normalize_ident(symbol),
		"Clone" | "Copy" | "Debug" | "Default" | "Eq" | "Hash" | "Ord" | "PartialEq" | "PartialOrd"
	)
}

fn collect_locked_derive_attr_ranges(edits: &[Edit]) -> Vec<(usize, usize)> {
	edits
		.iter()
		.filter(|edit| matches!(edit.rule, "RUST-STYLE-IMPORT-008" | "RUST-STYLE-IMPORT-009"))
		.map(|edit| (edit.start, edit.end))
		.collect()
}

fn ranges_overlap(start: usize, end: usize, other_start: usize, other_end: usize) -> bool {
	start < other_end && other_start < end
}
