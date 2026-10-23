use ra_ap_syntax::{
	AstNode,
	ast::{HasAttrs, HasModuleItem, HasName, Item, Module},
};

use crate::style::{
	imports,
	shared::{self, Edit, FileContext},
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
			let Item::Use(use_item) = item else {
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

			let range = use_item.syntax().text_range();
			let Some((start, mut end)) =
				shared::item_text_range(ctx, usize::from(range.start()), usize::from(range.end()))
			else {
				continue;
			};

			for line in ctx.text[end..].split_inclusive('\n') {
				if !line.trim().is_empty() {
					break;
				}

				end += line.len();
			}

			edits.push(Edit { start, end, replacement: String::new(), rule: "RUST-STYLE-MOD-007" });
		}
	}
}

fn is_cfg_test_module(module: &Module) -> bool {
	module
		.attrs()
		.any(|attr| attr.syntax().text().to_string().replace(' ', "").contains("cfg(test)"))
}
