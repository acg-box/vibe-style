use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use ra_ap_syntax::{
	AstNode,
	ast::{self, HasAttrs},
};
use regex::Regex;

use super::shared::{
	Edit, FileContext, TopItem, TopKind, USE_RE, Violation, WORKSPACE_IMPORT_ROOTS,
};

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
	let mut has_prelude_glob = false;
	let mut has_risky_glob_import = false;
	let mut import007_lines = HashSet::new();
	let mut import004_fixed_lines = HashSet::new();
	let mut import009_fixed_lines = HashSet::new();

	for item in &use_items {
		if let Some(path) = extract_use_path(ctx, item) {
			if path.replace(' ', "") == "crate::prelude::*" {
				has_prelude_glob = true;
			}
			let compact_path = path.replace(' ', "");
			if compact_path.ends_with("::*") && compact_path != "crate::prelude::*" {
				has_risky_glob_import = true;
			}
		}
	}
	for item in &use_items {
		let Some(path) = extract_use_path(ctx, item) else {
			continue;
		};

		if let Some(normalized) = normalize_mixed_self_child_use_path(&path) {
			super::shared::push_violation(
				violations,
				ctx,
				item.line,
				"RUST-STYLE-IMPORT-002",
				"Normalize imports like `use a::{b, b::c}` to `use a::{b::{self, c}}`.",
				true,
			);

			if emit_edits {
				if let Some((start, end)) = item_text_range(ctx, item) {
					if let Some(raw) = ctx.text.get(start..end) {
						if let Some(rewritten) = rewrite_use_item_with_path(raw, &normalized) {
							edits.push(Edit {
								start,
								end,
								replacement: rewritten,
								rule: "RUST-STYLE-IMPORT-002",
							});
						}
					}
				}
			}
		}
		if let Some(alias_caps) = Regex::new(r"\bas\s+([A-Za-z_][A-Za-z0-9_]*)\b")
			.expect("Expected operation to succeed.")
			.captures(&path)
		{
			if alias_caps.get(1).map(|m| m.as_str()) != Some("_") {
				super::shared::push_violation(
					violations,
					ctx,
					item.line,
					"RUST-STYLE-IMPORT-003",
					"Import aliases are not allowed except `as _` in test keep-alive modules.",
					false,
				);
			}
		}

		let compact_path = path.replace(' ', "");

		if has_prelude_glob
			&& compact_path.starts_with("crate::")
			&& compact_path != "crate::prelude::*"
		{
			import007_lines.insert(item.line);

			super::shared::push_violation(
				violations,
				ctx,
				item.line,
				"RUST-STYLE-IMPORT-007",
				"Avoid redundant crate imports when crate::prelude::* is imported.",
				true,
			);

			if emit_edits {
				if let (Some(start), Some(next)) = (
					super::shared::offset_from_line(&ctx.line_starts, item.start_line),
					super::shared::offset_from_line(&ctx.line_starts, item.end_line + 1),
				) {
					edits.push(Edit {
						start,
						end: next,
						replacement: String::new(),
						rule: "RUST-STYLE-IMPORT-007",
					});
				}
			}
		}
		if path.contains("::") {
			let imported_symbols = imported_symbols_from_use_path(&path);

			for symbol in imported_symbols {
				if symbol.is_empty() || !symbol.chars().next().is_some_and(char::is_lowercase) {
					continue;
				}

				let local_fn_def_re = Regex::new(&format!(
					r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:const\s+)?(?:unsafe\s+)?fn\s+{}\b",
					regex::escape(&symbol)
				))
				.expect("Expected operation to succeed.");
				let local_macro_def_re = Regex::new(&format!(
					r"^\s*(?:macro_rules!\s*{}\b|macro\s+{}\b)",
					regex::escape(&symbol),
					regex::escape(&symbol),
				))
				.expect("Expected operation to succeed.");
				let local_fn_defined = ctx.lines.iter().any(|line| {
					let code = super::shared::strip_string_and_line_comment(line, false).0;

					local_fn_def_re.is_match(&code)
				});
				let local_macro_defined = ctx.lines.iter().any(|line| {
					let code = super::shared::strip_string_and_line_comment(line, false).0;

					local_macro_def_re.is_match(&code)
				});
				let fn_ranges = unqualified_function_call_ranges(ctx, &symbol);
				let macro_ranges = unqualified_macro_call_ranges(ctx, &symbol);
				let called_fn_unqualified = !fn_ranges.is_empty();
				let called_macro_unqualified = !macro_ranges.is_empty();
				let needs_fn_fix = called_fn_unqualified && !local_fn_defined;
				let needs_macro_fix = called_macro_unqualified && !local_macro_defined;

				if needs_fn_fix || needs_macro_fix {
					let mut fixable = false;

					if let Some((qualified_symbol_path, rewritten_use_path)) =
						import004_fix_plan(&path, &symbol)
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

							if let Some(new_use_path) = rewritten_use_path {
								if let Some((start, end)) = item_text_range(ctx, item) {
									if let Some(raw) = ctx.text.get(start..end) {
										if let Some(rewritten) =
											rewrite_use_item_with_path(raw, &new_use_path)
										{
											edits.push(Edit {
												start,
												end,
												replacement: rewritten,
												rule: "RUST-STYLE-IMPORT-004",
											});
											import004_fixed_lines.insert(item.line);
										}
									}
								}
							} else if let (Some(start), Some(next)) = (
								super::shared::offset_from_line(&ctx.line_starts, item.start_line),
								super::shared::offset_from_line(
									&ctx.line_starts,
									item.end_line + 1,
								),
							) {
								edits.push(Edit {
									start,
									end: next,
									replacement: String::new(),
									rule: "RUST-STYLE-IMPORT-004",
								});
								import004_fixed_lines.insert(item.line);
							}
						}
					}

					super::shared::push_violation(
						violations,
						ctx,
						item.line,
						"RUST-STYLE-IMPORT-004",
						"Do not import free functions or macros into scope; prefer qualified module paths.",
						fixable,
					);

					break;
				}
			}
		}
	}

	let mut imported_symbol_paths: HashMap<String, HashSet<String>> = HashMap::new();
	let mut imported_symbol_lines: HashMap<String, HashSet<usize>> = HashMap::new();
	let mut imported_full_paths_by_symbol: HashMap<String, HashSet<String>> = HashMap::new();

	for item in &use_items {
		let Some(path) = extract_use_path(ctx, item) else {
			continue;
		};

		for symbol in imported_symbols_from_use_path(&path) {
			imported_symbol_paths.entry(symbol.clone()).or_default().insert(path.clone());
			imported_symbol_lines.entry(symbol).or_default().insert(item.line);
		}
		for full_path in imported_full_paths_from_use_path(&path) {
			let Some(symbol) = symbol_from_full_import_path(&full_path) else {
				continue;
			};

			imported_full_paths_by_symbol.entry(symbol).or_default().insert(full_path);
		}
	}
	for (symbol, paths) in &imported_symbol_paths {
		if paths.len() <= 1 {
			continue;
		}

		for line in imported_symbol_lines.get(symbol).into_iter().flat_map(|lines| lines.iter()) {
			super::shared::push_violation(
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
	let mut local_defined_symbols = HashSet::new();

	for item in &ctx.top_items {
		let Some(name) = item.name.as_deref() else {
			continue;
		};

		local_defined_symbols.insert(normalize_ident(name).to_owned());
	}
	let qualified_type_paths_by_symbol = collect_qualified_type_paths_by_symbol(ctx);

	for (symbol, imported_paths) in &imported_full_paths_by_symbol {
		if imported_paths.len() != 1 || local_defined_symbols.contains(symbol) {
			continue;
		}

		let Some(imported_path) = imported_paths.iter().next().cloned() else {
			continue;
		};
		let type_rewrites = unqualified_type_path_rewrites(ctx, symbol, &imported_path);
		let value_rewrites = unqualified_value_path_rewrites(ctx, symbol, &imported_path);
		let has_other_qualified_path = qualified_type_paths_by_symbol
			.get(symbol)
			.is_some_and(|paths| paths.iter().any(|path| path != &imported_path));
		let has_glob_ambiguity =
			has_risky_glob_import && has_root_unqualified_path_use(ctx, symbol);

		if !has_other_qualified_path && !has_glob_ambiguity {
			continue;
		}

		let mut use_item_plans = Vec::new();
		let mut fixable = true;

		for item in &use_items {
			let Some(path) = extract_use_path(ctx, item) else {
				continue;
			};

			if !use_item_imports_symbol_path(&path, symbol, &imported_path) {
				continue;
			}

			let Some((qualified_symbol_path, rewritten_use_path)) =
				import004_fix_plan(&path, symbol)
			else {
				fixable = false;
				break;
			};

			use_item_plans.push((item, qualified_symbol_path, rewritten_use_path));
		}

		if use_item_plans.is_empty() {
			fixable = false;
		}

		for line in imported_symbol_lines.get(symbol).into_iter().flat_map(|lines| lines.iter()) {
			super::shared::push_violation(
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
			if let Some(new_use_path) = rewritten_use_path {
				if let Some((start, end)) = item_text_range(ctx, item) {
					if let Some(raw) = ctx.text.get(start..end) {
						if let Some(rewritten) = rewrite_use_item_with_path(raw, &new_use_path) {
							edits.push(Edit {
								start,
								end,
								replacement: rewritten,
								rule: "RUST-STYLE-IMPORT-009",
							});
							import009_fixed_lines.insert(item.line);
						}
					}
				}
			} else if let (Some(start), Some(next)) = (
				super::shared::offset_from_line(&ctx.line_starts, item.start_line),
				super::shared::offset_from_line(&ctx.line_starts, item.end_line + 1),
			) {
				edits.push(Edit {
					start,
					end: next,
					replacement: String::new(),
					rule: "RUST-STYLE-IMPORT-009",
				});
				import009_fixed_lines.insert(item.line);
			}
		}
	}
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
	}
	if has_risky_glob_import {
		for symbol in candidate_paths_by_symbol.keys() {
			blocked_symbols.insert(symbol.clone());
		}
	}

	let mut pending_import_paths = BTreeSet::new();
	let mut import008_group_skip_lines = HashSet::new();

	for candidate in import008_candidates {
		if blocked_symbols.contains(&candidate.symbol) {
			continue;
		}

		super::shared::push_violation(
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

		if already_imported {
			edits.push(Edit {
				start: candidate.start,
				end: candidate.end,
				replacement: candidate.replacement,
				rule: "RUST-STYLE-IMPORT-008",
			});
			import008_group_skip_lines.insert(candidate.line);
		} else {
			pending_import_paths.insert(candidate.import_path);
		}
	}

	if emit_edits {
		if let Some((edit, touched_lines)) =
			build_import008_insert_edit(ctx, &use_runs, &local_module_roots, &pending_import_paths)
		{
			edits.push(edit);
			import008_group_skip_lines.extend(touched_lines);
		}
	}

	let mut import_group_skip_lines = import007_lines.clone();

	import_group_skip_lines.extend(import004_fixed_lines.iter().copied());
	import_group_skip_lines.extend(import009_fixed_lines.iter().copied());
	import_group_skip_lines.extend(import008_group_skip_lines.iter().copied());

	let (planned_import_group_edits, fixable_import_group_lines) =
		build_import_group_fix_plans(ctx, &use_runs, &import_group_skip_lines, &local_module_roots);

	if emit_edits {
		edits.extend(planned_import_group_edits);
	}

	for run in &use_runs {
		for pair in run.windows(2) {
			let prev = pair[0];
			let curr = pair[1];
			let Some(prev_path) = extract_use_path(ctx, prev) else {
				continue;
			};
			let Some(curr_path) = extract_use_path(ctx, curr) else {
				continue;
			};
			let prev_origin = use_origin(&prev_path, &local_module_roots);
			let curr_origin = use_origin(&curr_path, &local_module_roots);
			let is_fixable = fixable_import_group_lines.contains(&curr.line);

			if curr_origin < prev_origin {
				super::shared::push_violation(
					violations,
					ctx,
					curr.line,
					"RUST-STYLE-IMPORT-001",
					"Import groups must be ordered: std, third-party, self/workspace.",
					is_fixable,
				);
			}

			let between = separator_lines(ctx, prev, curr);
			let has_blank = between.iter().any(|line| line.trim().is_empty());
			let has_header_comment = between.iter().any(|line| line.trim_start().starts_with("//"));

			if curr_origin != prev_origin && !has_blank {
				super::shared::push_violation(
					violations,
					ctx,
					curr.line,
					"RUST-STYLE-IMPORT-002",
					"Separate import groups with one blank line.",
					is_fixable,
				);
			}
			if curr_origin == prev_origin && has_blank {
				super::shared::push_violation(
					violations,
					ctx,
					curr.line,
					"RUST-STYLE-IMPORT-002",
					"Do not place blank lines inside an import group.",
					is_fixable,
				);
			}
			if has_header_comment {
				super::shared::push_violation(
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

#[derive(Debug, Clone)]
struct Import008Candidate {
	line: usize,
	start: usize,
	end: usize,
	symbol: String,
	import_path: String,
	replacement: String,
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

		let import_path = segments.join("::");
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

		let line = super::shared::line_from_offset(&ctx.line_starts, start);

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

fn has_root_unqualified_path_use(ctx: &FileContext, symbol: &str) -> bool {
	for path in ctx.source_file.syntax().descendants().filter_map(ast::Path::cast) {
		if path.qualifier().is_some() || is_inside_cfg_test_module(&path) {
			continue;
		}
		if path.syntax().ancestors().any(|node| ast::Use::cast(node).is_some()) {
			continue;
		}
		if path.syntax().ancestors().any(|node| ast::MacroCall::cast(node).is_some()) {
			continue;
		}
		if path.syntax().ancestors().skip(1).any(|node| ast::Path::cast(node).is_some()) {
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

		let in_type_context = path.syntax().ancestors().any(|node| ast::PathType::cast(node).is_some());
		let in_value_context = path
			.syntax()
			.ancestors()
			.any(|node| ast::PathExpr::cast(node.clone()).is_some() || ast::PathPat::cast(node).is_some());

		if in_type_context || in_value_context {
			return true;
		}
	}

	false
}

fn use_item_imports_symbol_path(path: &str, symbol: &str, import_path: &str) -> bool {
	if !imported_symbols_from_use_path(path).iter().any(|item_symbol| item_symbol == symbol) {
		return false;
	}

	imported_full_paths_from_use_path(path).into_iter().any(|path| path == import_path)
}

fn is_inside_cfg_test_module(path: &ast::Path) -> bool {
	path.syntax().ancestors().filter_map(ast::Module::cast).any(|module| {
		module
			.attrs()
			.any(|attr| attr.syntax().text().to_string().replace(' ', "").contains("cfg(test)"))
	})
}

fn collect_path_segment_texts(path: &ast::Path, out: &mut Vec<String>) -> bool {
	if let Some(qualifier) = path.qualifier() {
		if !collect_path_segment_texts(&qualifier, out) {
			return false;
		}
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

		if let (Some(last_origin), Some(first_new_origin)) = (last_origin, first_new_origin) {
			if last_origin != first_new_origin {
				replacement.push('\n');
			}
		}

		replacement.push_str(&block);

		return Some((
			Edit { start: run_end, end: run_end, replacement, rule: "RUST-STYLE-IMPORT-008" },
			run.iter().map(|item| item.line).collect::<HashSet<_>>(),
		));
	}

	let insert_line = import008_insert_line(ctx);
	let insert_pos =
		super::shared::offset_from_line(&ctx.line_starts, insert_line).unwrap_or(ctx.text.len());
	let mut replacement = block;

	if !ctx.top_items.is_empty() {
		replacement.push('\n');
	}

	Some((
		Edit { start: insert_pos, end: insert_pos, replacement, rule: "RUST-STYLE-IMPORT-008" },
		HashSet::new(),
	))
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
	let open = path.find('{')?;
	let mut depth = 0_i32;
	let mut close = None;

	for (idx, ch) in path.char_indices().skip(open) {
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

	let close = close?;

	if !path[close + 1..].trim().is_empty() {
		return None;
	}

	let mut prefix = path[..open].trim().to_owned();

	if prefix.is_empty() {
		return None;
	}
	if !prefix.ends_with("::") {
		prefix.push_str("::");
	}

	let inside = &path[open + 1..close];
	let segments = split_top_level_csv(inside);

	if segments.is_empty() {
		return None;
	}

	#[derive(Clone)]
	enum Segment {
		Simple(String),
		Nested { head: String, children: Vec<String> },
	}

	let mut parsed_segments = Vec::new();

	for segment in segments {
		let trimmed = segment.trim();

		if trimmed.is_empty() {
			continue;
		}
		if trimmed.contains(" as ") || trimmed == "*" {
			return None;
		}

		if let Some((head, rest)) = trimmed.split_once("::{") {
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
					|| child == "*" || child.contains(" as ")
					|| child.contains('{')
					|| child.contains('}')
			}) {
				return None;
			}

			parsed_segments.push(Segment::Nested {
				head: head.trim().to_owned(),
				children: nested_children
					.into_iter()
					.map(|child| child.trim().to_owned())
					.collect(),
			});

			continue;
		}

		if trimmed.contains('{') || trimmed.contains('}') {
			return None;
		}

		parsed_segments.push(Segment::Simple(trimmed.to_owned()));
	}

	if parsed_segments.is_empty() {
		return None;
	}

	let mut qualified_symbol_path = None::<String>;
	let mut kept = Vec::new();

	for segment in parsed_segments {
		match segment {
			Segment::Simple(name) => {
				if qualified_symbol_path.is_none() && is_same_ident(&name, symbol) {
					qualified_symbol_path = Some(format!("{prefix}{name}"));

					continue;
				}

				kept.push(name);
			},
			Segment::Nested { head, children } => {
				let mut child_kept = Vec::new();

				for child in children {
					if qualified_symbol_path.is_none() && is_same_ident(&child, symbol) {
						qualified_symbol_path = Some(format!("{prefix}{head}::{child}"));

						continue;
					}

					child_kept.push(child);
				}

				if !child_kept.is_empty() {
					kept.push(format!("{head}::{{{}}}", child_kept.join(", ")));
				}
			},
		}
	}

	let qualified_symbol_path = qualified_symbol_path?;

	if kept.is_empty() {
		return Some((qualified_symbol_path, None));
	}

	let rewritten_use_path = format!("{prefix}{{{}}}", kept.join(", "));

	Some((qualified_symbol_path, Some(rewritten_use_path)))
}

fn import004_fix_plan(path: &str, symbol: &str) -> Option<(String, Option<String>)> {
	if let Some((prefix, imported_symbol)) = simple_import_prefix_symbol(path) {
		if is_same_ident(&imported_symbol, symbol) {
			return Some((format!("{prefix}::{imported_symbol}"), None));
		}
	}

	braced_import_fix_plan(path, symbol)
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
	let open = path.find('{')?;
	let mut depth = 0_i32;
	let mut close = None;

	for (idx, ch) in path.char_indices().skip(open) {
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

	let close = close?;

	if !path[close + 1..].trim().is_empty() {
		return None;
	}

	let prefix = &path[..open + 1];
	let inner = &path[open + 1..close];
	let segments = split_top_level_csv(inner);

	if segments.is_empty() {
		return None;
	}
	if segments.iter().any(|segment| segment.contains(" as ")) {
		return None;
	}

	#[derive(Default)]
	struct Group {
		indices: Vec<usize>,
		has_self: bool,
		children: Vec<String>,
	}

	let mut groups: HashMap<String, Group> = HashMap::new();
	let mut parsed = Vec::new();

	for (idx, segment) in segments.iter().enumerate() {
		if let Some((head, rest)) = segment.split_once("::") {
			let head = head.trim();

			if head.is_empty() || head.contains('{') || head.contains('}') {
				parsed.push(None);

				continue;
			}
			if rest.starts_with('{') && rest.ends_with('}') {
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

				parsed.push(Some((head.to_owned(), true)));

				continue;
			}

			let child = rest.trim();

			if child.is_empty() || child.contains("::") {
				parsed.push(None);

				continue;
			}

			let group = groups.entry(head.to_owned()).or_default();

			group.indices.push(idx);
			group.children.push(child.to_owned());
			parsed.push(Some((head.to_owned(), false)));

			continue;
		}

		let head = segment.trim();

		if head.is_empty() || head.contains('{') || head.contains('}') {
			parsed.push(None);

			continue;
		}

		let group = groups.entry(head.to_owned()).or_default();

		group.indices.push(idx);

		group.has_self = true;

		parsed.push(Some((head.to_owned(), false)));
	}

	let mut emit = vec![true; segments.len()];
	let mut merged = false;
	let mut rewritten = Vec::new();

	for (idx, segment) in segments.iter().enumerate() {
		if !emit[idx] {
			continue;
		}

		let Some((head, _)) = parsed.get(idx).cloned().flatten() else {
			rewritten.push(segment.to_owned());

			continue;
		};
		let Some(group) = groups.get(&head) else {
			rewritten.push(segment.to_owned());

			continue;
		};

		if !group.has_self || group.children.is_empty() {
			rewritten.push(segment.to_owned());

			continue;
		}
		if group.indices.len() == 1 {
			rewritten.push(segment.to_owned());

			continue;
		}
		if group.indices.first().copied() != Some(idx) {
			emit[idx] = false;

			continue;
		}

		let mut seen = HashSet::new();
		let mut children = Vec::new();

		for child in &group.children {
			if seen.insert(child.clone()) {
				children.push(child.clone());
			}
		}

		let combined = format!("{head}::{{self, {}}}", children.join(", "));

		rewritten.push(combined);

		merged = true;

		for original_idx in group.indices.iter().skip(1) {
			emit[*original_idx] = false;
		}
	}

	if !merged {
		return None;
	}

	let rewritten_path = format!("{prefix}{}{}", rewritten.join(", "), &path[close..=close]);

	if rewritten_path == path { None } else { Some(rewritten_path) }
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
	let start = super::shared::offset_from_line(&ctx.line_starts, item.start_line)?;
	let end = super::shared::offset_from_line(&ctx.line_starts, item.end_line + 1)
		.unwrap_or(ctx.text.len());

	if end < start { None } else { Some((start, end)) }
}

fn run_text_range(ctx: &FileContext, first: &TopItem, last: &TopItem) -> Option<(usize, usize)> {
	let start = super::shared::offset_from_line(&ctx.line_starts, first.start_line)?;
	let end = super::shared::offset_from_line(&ctx.line_starts, last.end_line + 1)
		.unwrap_or(ctx.text.len());

	if end < start { None } else { Some((start, end)) }
}

fn build_import_group_fix_plans(
	ctx: &FileContext,
	use_runs: &[Vec<&TopItem>],
	skip_lines: &HashSet<usize>,
	local_module_roots: &HashSet<String>,
) -> (Vec<Edit>, HashSet<usize>) {
	struct UseEntry<'a> {
		item: &'a TopItem,
		origin: usize,
		order: usize,
		block: String,
	}

	let mut planned_edits = Vec::new();
	let mut fixable_lines = HashSet::new();

	for run in use_runs {
		if run.len() < 2 {
			continue;
		}
		if run.iter().any(|item| skip_lines.contains(&item.line)) {
			continue;
		}

		let mut safe_to_rewrite = true;

		for pair in run.windows(2) {
			for line in separator_lines(ctx, pair[0], pair[1]) {
				let trimmed = line.trim();

				if trimmed.is_empty() {
					continue;
				}

				safe_to_rewrite = false;

				break;
			}

			if !safe_to_rewrite {
				break;
			}
		}

		if !safe_to_rewrite {
			continue;
		}

		let mut entries = Vec::with_capacity(run.len());

		for (order, item) in run.iter().enumerate() {
			let Some(path) = extract_use_path(ctx, item) else {
				safe_to_rewrite = false;

				break;
			};
			let Some((start, end)) = item_text_range(ctx, item) else {
				safe_to_rewrite = false;

				break;
			};
			let Some(block) = ctx.text.get(start..end) else {
				safe_to_rewrite = false;

				break;
			};
			let mut block_lines = Vec::new();
			let mut has_item_start = false;

			for line in block.lines() {
				let trimmed = line.trim();

				if !has_item_start {
					if trimmed.is_empty() {
						continue;
					}
					if line.trim_start().starts_with("//") {
						safe_to_rewrite = false;

						break;
					}

					has_item_start = true;
				}

				block_lines.push(line);
			}

			if !safe_to_rewrite || block_lines.is_empty() {
				safe_to_rewrite = false;

				break;
			}

			let mut normalized_block = block_lines.join("\n");

			if block.ends_with('\n') {
				normalized_block.push('\n');
			}

			entries.push(UseEntry {
				item,
				origin: use_origin(&path, local_module_roots),
				order,
				block: normalized_block,
			});
		}

		if !safe_to_rewrite {
			continue;
		}

		let Some((run_start, run_end)) = run_text_range(ctx, run[0], run[run.len() - 1]) else {
			continue;
		};
		let Some(original) = ctx.text.get(run_start..run_end) else {
			continue;
		};
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
