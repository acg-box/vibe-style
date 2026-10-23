use std::collections::{BTreeSet, HashMap, HashSet};

use ra_ap_syntax::{
	AstNode as _, SourceFile,
	ast::{self, HasAttrs as _, Module, Use},
};

use crate::style::{
	fixes,
	imports::{
		Import006TargetScope, UseItemAnalysis, analysis, groups, paths, planning, qualification,
	},
	shared::{self, Edit, FileContext, TopKind, Violation},
	syntax,
};

pub(super) fn apply_import006_local_use_scope_rule(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	let mut pending_paths_by_scope = HashMap::<usize, HashSet<String>>::new();

	for use_item in ctx.source_file.syntax().descendants().filter_map(Use::cast) {
		if import006_is_allowed_use_item(&use_item) {
			continue;
		}

		let range = use_item.syntax().text_range();
		let line = shared::line_from_offset(&ctx.line_starts, usize::from(range.start()));
		let fix_plan = build_import006_fix_plan(ctx, &use_item, &mut pending_paths_by_scope);
		let fixable = fix_plan.is_some();

		shared::push_violation(
			violations,
			ctx,
			line,
			"RUST-STYLE-IMPORT-006",
			"Use items must appear only at file top level or module top level.",
			fixable,
		);

		if emit_edits && let Some((delete_edit, insert_edit)) = fix_plan {
			edits.push(delete_edit);

			if let Some(insert_edit) = insert_edit {
				edits.push(insert_edit);
			}
		}
	}
}

pub(super) fn check_cfg_test_module_import_rules(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	for module in ctx.source_file.syntax().descendants().filter_map(Module::cast) {
		if !module_has_cfg_test_attr(&module) {
			continue;
		}

		let Some(item_list) = module.item_list() else {
			continue;
		};
		let Some((module_body_start, module_body_end)) = item_list_body_text_range(&item_list)
		else {
			continue;
		};
		let Some(module_body_text) = ctx.text.get(module_body_start..module_body_end) else {
			continue;
		};
		// Re-plan the module after outer rules have expanded or resolved its imports.
		let emit_edits = emit_edits
			&& !edits.iter().any(|edit| {
				matches!(edit.rule, "RUST-STYLE-IMPORT-007" | "RUST-STYLE-IMPORT-010")
					&& edit.start >= module_body_start
					&& edit.end <= module_body_end
			});
		let Ok(Some(module_ctx)) =
			shared::read_file_context_from_text(&ctx.path, module_body_text.to_owned())
		else {
			continue;
		};
		let has_follow_up_group_fix = cfg_test_module_has_follow_up_group_fix(&module_ctx);
		let mut module_violations = Vec::new();
		let mut module_edits = Vec::new();

		groups::apply_pub_use_group_rules(
			&module_ctx,
			&mut module_violations,
			&mut module_edits,
			emit_edits,
		);

		let use_runs = groups::collect_non_pub_use_runs(&module_ctx);
		let use_items = use_runs.iter().flat_map(|run| run.iter().copied()).collect::<Vec<_>>();
		let analysis = analysis::build_import_analysis(&module_ctx, use_runs, use_items);
		let use_item_skip_lines = collect_cfg_test_module_skip_lines(&analysis.use_item_analyses);
		let import004_fixed_lines = planning::apply_use_item_rules(
			&module_ctx,
			&mut module_violations,
			&mut module_edits,
			emit_edits,
			&analysis.use_item_analyses,
			&use_item_skip_lines,
			false,
		);

		qualification::push_import004_ambiguous_symbol_violations(
			&module_ctx,
			&mut module_violations,
			&analysis.imported_symbol_maps,
		);

		let import004_qualified_path_fixed_lines =
			qualification::apply_import004_qualified_function_path_rule(
				&module_ctx,
				&mut module_violations,
				&mut module_edits,
				emit_edits,
				&analysis.use_runs,
				&analysis.local_module_roots,
				&analysis.imported_symbol_maps.full_paths_by_symbol,
				false,
			);
		let mut import_group_skip_lines = use_item_skip_lines;

		import_group_skip_lines.extend(import004_fixed_lines);
		import_group_skip_lines.extend(import004_qualified_path_fixed_lines);

		let (planned_import_group_edits, fixable_import_group_lines) =
			groups::build_import_group_fix_plans(&module_ctx, &analysis, &import_group_skip_lines);

		if emit_edits {
			module_edits.extend(planned_import_group_edits);
		}

		groups::push_import_group_order_spacing_violations(
			&module_ctx,
			&mut module_violations,
			&analysis.use_runs,
			&fixable_import_group_lines,
			&analysis.local_module_roots,
		);

		violations.extend(module_violations.into_iter().filter_map(|violation| {
			translate_cfg_test_violation(
				ctx,
				&module_ctx,
				module_body_start,
				violation,
				has_follow_up_group_fix,
			)
		}));

		if !emit_edits {
			continue;
		}

		edits.extend(
			module_edits
				.into_iter()
				.filter_map(|edit| translate_cfg_test_edit(ctx, module_body_start, edit)),
		);
	}
}

fn import006_is_allowed_use_item(use_item: &Use) -> bool {
	let Some(parent) = use_item.syntax().parent() else {
		return false;
	};

	if SourceFile::cast(parent.clone()).is_some() {
		return true;
	}

	let Some(item_list) = ast::ItemList::cast(parent) else {
		return false;
	};

	item_list.syntax().parent().and_then(Module::cast).is_some()
}

fn build_import006_fix_plan(
	ctx: &FileContext,
	use_item: &Use,
	pending_paths_by_scope: &mut HashMap<usize, HashSet<String>>,
) -> Option<(Edit, Option<Edit>)> {
	let raw_item = use_item.syntax().text().to_string();
	let path = paths::extract_use_path_from_text(&raw_item)?;

	if raw_item.contains(" as ")
		|| path.contains('*')
		|| path.contains(" as ")
		|| import006_use_item_has_comments(ctx, use_item)
		|| paths::syntax_has_cfg_attributes(use_item.syntax())
		|| import006_has_cfg_ancestor(use_item)
	{
		return None;
	}

	let target_scope = import006_target_scope(ctx, use_item)?;
	let normalized_path = paths::normalize_use_path_for_equivalence(&path);
	let known_paths = pending_paths_by_scope.entry(target_scope.key).or_default();
	let path_already_pending = known_paths.contains(&normalized_path);
	let equivalent_exists = path_already_pending
		|| import006_target_scope_has_equivalent_path(&target_scope.ctx, &normalized_path);

	if !equivalent_exists {
		let symbols = paths::imported_symbols_from_use_path(&path);
		let pending_conflict = known_paths.iter().any(|pending| {
			paths::imported_symbols_from_use_path(pending)
				.iter()
				.any(|symbol| symbols.contains(symbol))
		});

		if pending_conflict || import006_path_conflicts_in_target_scope(&target_scope.ctx, &path) {
			return None;
		}
	}

	let delete_edit = import006_delete_local_use_edit(ctx, use_item)?;

	if equivalent_exists {
		return Some((delete_edit, None));
	}

	let insert_edit = import006_insert_scope_use_edit(&target_scope.ctx, &path)
		.and_then(|edit| import006_translate_scope_edit(ctx, target_scope.offset, edit))?;

	known_paths.insert(normalized_path);

	Some((delete_edit, Some(insert_edit)))
}

fn import006_has_cfg_ancestor(use_item: &Use) -> bool {
	use_item
		.syntax()
		.ancestors()
		.skip(1)
		.take_while(|ancestor| !Module::can_cast(ancestor.kind()))
		.any(|ancestor| paths::syntax_has_cfg_attributes(&ancestor))
}

fn import006_use_item_has_comments(ctx: &FileContext, use_item: &Use) -> bool {
	let range = use_item.syntax().text_range();
	let start = usize::from(range.start());
	let end = usize::from(range.end());
	let start_line = shared::line_from_offset(&ctx.line_starts, start);
	let end_line = if end == 0 {
		start_line
	} else {
		shared::line_from_offset(&ctx.line_starts, end.saturating_sub(1))
	};
	let line_start = shared::offset_from_line(&ctx.line_starts, start_line).unwrap_or(start);
	let line_end =
		shared::offset_from_line(&ctx.line_starts, end_line + 1).unwrap_or(ctx.text.len());
	let Some(snippet) = ctx.text.get(line_start..line_end) else {
		return true;
	};

	if snippet.contains("//") || snippet.contains("/*") || snippet.contains("*/") {
		return true;
	}

	let line_idx = start_line.saturating_sub(1);
	let Some(line) = (line_idx > 0).then(|| ctx.lines.get(line_idx - 1)).flatten() else {
		return false;
	};
	let trimmed = line.trim();

	!trimmed.is_empty() && import006_is_comment_line(trimmed)
}

fn import006_is_comment_line(trimmed: &str) -> bool {
	trimmed.starts_with("//")
		|| trimmed.starts_with("/*")
		|| trimmed.starts_with('*')
		|| trimmed.starts_with("*/")
}

fn import006_target_scope(ctx: &FileContext, use_item: &Use) -> Option<Import006TargetScope> {
	for item_list in use_item.syntax().ancestors().skip(1).filter_map(ast::ItemList::cast) {
		if item_list.syntax().parent().and_then(Module::cast).is_none() {
			continue;
		}

		let (body_start, body_end) = item_list_body_text_range(&item_list)?;
		let body_text = ctx.text.get(body_start..body_end)?.to_owned();
		let scope_ctx = shared::read_file_context_from_text(&ctx.path, body_text).ok()??;

		return Some(Import006TargetScope { ctx: scope_ctx, offset: body_start, key: body_start });
	}

	Some(Import006TargetScope {
		ctx: shared::read_file_context_from_text(&ctx.path, ctx.text.clone()).ok()??,
		offset: 0,
		key: 0,
	})
}

fn import006_target_scope_has_equivalent_path(ctx: &FileContext, normalized_path: &str) -> bool {
	ctx.top_items.iter().any(|item| {
		item.kind == TopKind::Use
			&& paths::extract_use_path(ctx, item).is_some_and(|path| {
				paths::normalize_use_path_for_equivalence(&path) == normalized_path
			})
	})
}

fn import006_path_conflicts_in_target_scope(ctx: &FileContext, path: &str) -> bool {
	let imported_symbols = paths::imported_symbols_from_use_path(path);

	if imported_symbols.is_empty() {
		return true;
	}

	let mut target_use_items = groups::collect_non_pub_use_runs(ctx)
		.into_iter()
		.flat_map(|run| run.into_iter())
		.collect::<Vec<_>>();

	target_use_items
		.extend(groups::collect_pub_use_runs(ctx).into_iter().flat_map(|run| run.into_iter()));

	let target_use_item_analyses = analysis::collect_use_item_analyses(ctx, &target_use_items);
	let target_maps = analysis::collect_imported_symbol_maps(&target_use_item_analyses);
	let local_defined_symbols = analysis::collect_local_defined_symbols(ctx);
	let target_path = paths::normalize_use_path_for_equivalence(path);

	imported_symbols.into_iter().any(|symbol| {
		let normalized_symbol = syntax::normalize_ident(&symbol);

		if local_defined_symbols.contains(normalized_symbol) {
			return true;
		}

		target_maps.symbol_paths.iter().any(|(existing_symbol, paths)| {
			syntax::normalize_ident(existing_symbol) == normalized_symbol
				&& paths.iter().any(|existing_path| {
					paths::normalize_use_path_for_equivalence(existing_path) != target_path
				})
		})
	})
}

fn import006_delete_local_use_edit(ctx: &FileContext, use_item: &Use) -> Option<Edit> {
	let range = use_item.syntax().text_range();
	let item_start = usize::from(range.start());
	let item_end = usize::from(range.end());
	let start_line = shared::line_from_offset(&ctx.line_starts, item_start);
	let end_line = if item_end == 0 {
		start_line
	} else {
		shared::line_from_offset(&ctx.line_starts, item_end.saturating_sub(1))
	};

	if let Some((line_start, line_end)) =
		import006_full_line_delete_range(ctx, start_line, end_line, item_start, item_end)
	{
		return Some(Edit {
			start: line_start,
			end: line_end,
			replacement: String::new(),
			rule: "RUST-STYLE-IMPORT-006",
		});
	}

	Some(Edit {
		start: item_start,
		end: item_end,
		replacement: String::new(),
		rule: "RUST-STYLE-IMPORT-006",
	})
}

fn import006_full_line_delete_range(
	ctx: &FileContext,
	start_line: usize,
	end_line: usize,
	item_start: usize,
	item_end: usize,
) -> Option<(usize, usize)> {
	let line_start = shared::offset_from_line(&ctx.line_starts, start_line)?;
	let line_end =
		shared::offset_from_line(&ctx.line_starts, end_line + 1).unwrap_or(ctx.text.len());
	let prefix = ctx.text.get(line_start..item_start)?;
	let suffix = ctx.text.get(item_end..line_end)?;

	if prefix.trim().is_empty() && suffix.trim().is_empty() {
		Some((line_start, line_end))
	} else {
		None
	}
}

fn import006_insert_scope_use_edit(ctx: &FileContext, path: &str) -> Option<Edit> {
	let mut pending_import_paths = BTreeSet::new();

	pending_import_paths.insert(path.to_owned());

	let use_runs = groups::collect_non_pub_use_runs(ctx);
	let local_module_roots = groups::collect_local_module_roots(ctx);
	let (edit, _touched_lines) = groups::build_import008_insert_edit(
		ctx,
		&use_runs,
		&local_module_roots,
		&pending_import_paths,
		"RUST-STYLE-IMPORT-006",
	)?;

	Some(Edit { rule: "RUST-STYLE-IMPORT-006", ..edit })
}

fn import006_translate_scope_edit(ctx: &FileContext, offset: usize, edit: Edit) -> Option<Edit> {
	let start = offset.checked_add(edit.start)?;
	let end = offset.checked_add(edit.end)?;

	if end > ctx.text.len() || start > end {
		return None;
	}

	Some(Edit { start, end, replacement: edit.replacement, rule: edit.rule })
}

fn module_has_cfg_test_attr(module: &Module) -> bool {
	module
		.attrs()
		.any(|attr| attr.syntax().text().to_string().replace(' ', "").contains("cfg(test)"))
}

fn item_list_body_text_range(item_list: &ast::ItemList) -> Option<(usize, usize)> {
	let range = item_list.syntax().text_range();
	let start = usize::from(range.start());
	let end = usize::from(range.end());

	if end <= start + 1 { None } else { Some((start + 1, end - 1)) }
}

fn collect_cfg_test_module_skip_lines(use_item_analyses: &[UseItemAnalysis<'_>]) -> HashSet<usize> {
	let mut skip_lines = HashSet::new();

	for use_item_analysis in use_item_analyses {
		let compact_path = paths::compact_path_for_match(&use_item_analysis.path);

		if compact_path.contains('*')
			|| compact_path == "super"
			|| compact_path.starts_with("super::")
		{
			skip_lines.insert(use_item_analysis.item.line);
		}
	}

	skip_lines
}

fn translate_cfg_test_violation(
	ctx: &FileContext,
	module_ctx: &FileContext,
	module_body_start: usize,
	mut violation: Violation,
	has_follow_up_group_fix: bool,
) -> Option<Violation> {
	let local_line_offset = shared::offset_from_line(&module_ctx.line_starts, violation.line)?;
	let line = shared::line_from_offset(&ctx.line_starts, module_body_start + local_line_offset);

	if has_follow_up_group_fix
		&& !violation.fixable
		&& matches!(violation.rule, "RUST-STYLE-IMPORT-001" | "RUST-STYLE-IMPORT-002")
	{
		violation.fixable = true;
	}

	Some(Violation {
		file: ctx.path.clone(),
		line,
		rule: violation.rule,
		message: violation.message,
		fixable: violation.fixable,
	})
}

fn translate_cfg_test_edit(
	ctx: &FileContext,
	module_body_start: usize,
	edit: Edit,
) -> Option<Edit> {
	let start = module_body_start.checked_add(edit.start)?;
	let end = module_body_start.checked_add(edit.end)?;

	if end > ctx.text.len() || start > end {
		return None;
	}

	Some(Edit { start, end, replacement: edit.replacement, rule: edit.rule })
}

fn cfg_test_module_has_follow_up_group_fix(ctx: &FileContext) -> bool {
	let use_runs = groups::collect_non_pub_use_runs(ctx);

	if use_runs.is_empty() {
		return false;
	}

	let use_items = use_runs.iter().flat_map(|run| run.iter().copied()).collect::<Vec<_>>();
	let analysis = analysis::build_import_analysis(ctx, use_runs, use_items);
	let use_item_skip_lines = collect_cfg_test_module_skip_lines(&analysis.use_item_analyses);
	let mut probe_violations = Vec::new();
	let mut probe_edits = Vec::new();

	planning::apply_use_item_rules(
		ctx,
		&mut probe_violations,
		&mut probe_edits,
		true,
		&analysis.use_item_analyses,
		&use_item_skip_lines,
		false,
	);

	if probe_edits.is_empty() {
		return false;
	}

	let mut rewritten = ctx.text.clone();
	let Ok(applied) = fixes::apply_edits(&mut rewritten, probe_edits) else {
		return false;
	};

	if applied == 0 {
		return false;
	}

	let Ok(Some(next_ctx)) = shared::read_file_context_from_text(&ctx.path, rewritten) else {
		return false;
	};
	let next_use_runs = groups::collect_non_pub_use_runs(&next_ctx);

	if next_use_runs.is_empty() {
		return false;
	}

	let next_use_items =
		next_use_runs.iter().flat_map(|run| run.iter().copied()).collect::<Vec<_>>();
	let next_analysis = analysis::build_import_analysis(&next_ctx, next_use_runs, next_use_items);
	let next_use_item_skip_lines =
		collect_cfg_test_module_skip_lines(&next_analysis.use_item_analyses);
	let mut next_probe_violations = Vec::new();
	let mut next_probe_edits = Vec::new();
	let next_import004_fixed_lines = planning::apply_use_item_rules(
		&next_ctx,
		&mut next_probe_violations,
		&mut next_probe_edits,
		false,
		&next_analysis.use_item_analyses,
		&next_use_item_skip_lines,
		false,
	);
	let mut next_import_group_skip_lines = next_use_item_skip_lines;

	next_import_group_skip_lines.extend(next_import004_fixed_lines);

	let (_planned_import_group_edits, fixable_import_group_lines) =
		groups::build_import_group_fix_plans(
			&next_ctx,
			&next_analysis,
			&next_import_group_skip_lines,
		);

	!fixable_import_group_lines.is_empty()
}
