use std::collections::HashSet;

use crate::style::{
	imports::{UseItemAnalysis, aliases, groups, qualification},
	shared::{Edit, FileContext, Violation},
};

pub(super) fn apply_use_item_rules(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	use_item_analyses: &[UseItemAnalysis<'_>],
	skip_lines: &HashSet<usize>,
	with_import_shortening: bool,
) -> HashSet<usize> {
	let mut import004_fixed_lines = HashSet::new();
	let mut planned_import_paths = Vec::new();

	for use_item_analysis in use_item_analyses {
		let item = use_item_analysis.item;

		if skip_lines.contains(&item.line) {
			continue;
		}

		let path = &use_item_analysis.path;

		if with_import_shortening {
			if qualification::apply_import009_std_fmt_result_rule(
				ctx, violations, edits, emit_edits, item, path,
			) {
				import004_fixed_lines.insert(item.line);

				continue;
			}
			if qualification::apply_import009_non_importable_root_use_rule(
				ctx, violations, edits, emit_edits, item, path,
			) {
				import004_fixed_lines.insert(item.line);

				continue;
			}
		}
		if groups::apply_import002_normalization_rule(
			ctx, violations, edits, emit_edits, item, path,
		) {
			import004_fixed_lines.insert(item.line);
		}

		let mut alias_rule_applied = false;

		if aliases::apply_import003_trait_keep_alive_rule(
			ctx, violations, edits, emit_edits, item, path,
		) {
			import004_fixed_lines.insert(item.line);

			alias_rule_applied = true;
		}
		if aliases::apply_import003_non_keep_alive_alias_rule(
			ctx, violations, edits, emit_edits, item, path,
		) {
			import004_fixed_lines.insert(item.line);

			alias_rule_applied = true;
		}
		if !alias_rule_applied {
			aliases::push_alias_violation_if_needed(ctx, violations, item, path);
		}
		if qualification::apply_import004_free_fn_macro_rule(
			ctx,
			violations,
			edits,
			emit_edits,
			item,
			path,
			&mut planned_import_paths,
		) {
			import004_fixed_lines.insert(item.line);
		}
	}

	import004_fixed_lines
}
