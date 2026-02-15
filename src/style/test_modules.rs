use crate::style::{
	imports,
	shared::{self, Edit, FileContext},
};

use ra_ap_syntax::{
	AstNode,
	ast::{self, HasAttrs, HasModuleItem, HasName, Module},
};

pub(crate) fn check_test_module_super_glob(
	ctx: &FileContext,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	if !emit_edits {
		return;
	}

	for module in ctx.source_file.syntax().descendants().filter_map(Module::cast) {
		let Some(name) = module.name() else {
			continue;
		};
		if name.text() != "tests" {
			continue;
		}
		if !is_cfg_test_module(&module) {
			continue;
		}

		let Some(item_list) = module.item_list() else {
			continue;
		};

		for item in item_list.items() {
			let ast::Item::Use(use_item) = item else {
				continue;
			};
			let Some(use_tree) = use_item.use_tree() else {
				continue;
			};
			if use_tree.syntax().text().to_string().trim() != "super::*" {
				continue;
			}

			// If we can resolve what the tests module uses from `super` and it is non-empty,
			// let the existing IMPORT-007 logic expand the glob to explicit imports.
			let Some(used) = imports::exported_symbols_from_super_scope(&use_item) else {
				continue;
			};
			if !used.is_empty() {
				continue;
			}

			let start_offset = usize::from(use_item.syntax().text_range().start());
			let start_line = shared::line_from_offset(&ctx.line_starts, start_offset);
			let start_idx = start_line.saturating_sub(1);
			let mut end_idx = start_idx;

			while end_idx + 1 < ctx.lines.len() && ctx.lines[end_idx + 1].trim().is_empty() {
				end_idx += 1;
			}

			let start =
				shared::offset_from_line(&ctx.line_starts, start_line).unwrap_or(start_offset);
			let end =
				shared::offset_from_line(&ctx.line_starts, end_idx + 2).unwrap_or(ctx.text.len());

			edits.push(Edit { start, end, replacement: String::new(), rule: "RUST-STYLE-MOD-007" });
		}
	}
}

fn is_cfg_test_module(module: &Module) -> bool {
	module
		.attrs()
		.any(|attr| attr.syntax().text().to_string().replace(' ', "").contains("cfg(test)"))
}
