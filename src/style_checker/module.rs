use std::collections::{HashMap, HashSet};

use ra_ap_syntax::{
	AstNode,
	ast::{self, HasAttrs, HasModuleItem, HasName},
};

use super::shared::{
	Edit, FileContext, TopItem, TopKind, Violation, WORKSPACE_IMPORT_ROOTS, line_from_offset,
	offset_from_line, push_violation,
};

fn order_bucket(kind: TopKind) -> Option<usize> {
	match kind {
		TopKind::Mod => Some(0),
		TopKind::Use => Some(1),
		TopKind::MacroRules => Some(2),
		TopKind::Type => Some(3),
		TopKind::Const => Some(4),
		TopKind::Static => Some(5),
		TopKind::Trait => Some(6),
		TopKind::Enum | TopKind::Struct | TopKind::Impl => Some(8),
		TopKind::Fn => Some(10),
		TopKind::Other => None,
	}
}

fn is_cfg_test_attrs(attrs: &[String]) -> bool {
	attrs.iter().any(|attr| attr.replace(' ', "").contains("#[cfg(test)]"))
}

fn use_origin(path: &str) -> usize {
	let root = path.trim_start_matches(':').split("::").next().unwrap_or_default();
	if matches!(root, "std" | "core" | "alloc") {
		0
	} else if matches!(root, "crate" | "self" | "super")
		|| WORKSPACE_IMPORT_ROOTS.contains(root)
		|| WORKSPACE_IMPORT_ROOTS.contains(&root.replace('-', "_"))
	{
		2
	} else {
		1
	}
}

fn use_item_origin(use_item: &ast::Use) -> usize {
	let path = use_item.use_tree().map(|tree| tree.syntax().text().to_string()).unwrap_or_default();
	use_origin(&path.replace(' ', ""))
}

fn is_cfg_test_mod_item(item: &TopItem) -> bool {
	item.kind == TopKind::Mod && is_cfg_test_attrs(&item.attrs)
}

fn separator_blank_only(ctx: &FileContext, prev: &TopItem, next: &TopItem) -> bool {
	if prev.end_line >= next.start_line.saturating_sub(1) {
		return true;
	}
	ctx.lines[prev.end_line..next.start_line.saturating_sub(1)]
		.iter()
		.all(|line| line.trim().is_empty())
}

fn item_text_range(ctx: &FileContext, item: &TopItem) -> Option<(usize, usize)> {
	let start = offset_from_line(&ctx.line_starts, item.start_line)?;
	let end = offset_from_line(&ctx.line_starts, item.end_line + 1).unwrap_or(ctx.text.len());
	if end < start { None } else { Some((start, end)) }
}

fn is_reorderable_kind(kind: TopKind) -> bool {
	matches!(
		kind,
		TopKind::Use
			| TopKind::Type
			| TopKind::Const
			| TopKind::Static
			| TopKind::Trait
			| TopKind::Enum
			| TopKind::Struct
			| TopKind::Impl
			| TopKind::Fn
	)
}

fn build_module_reorder_plans(ctx: &FileContext) -> (Vec<Edit>, HashSet<usize>, HashSet<usize>) {
	#[derive(Clone)]
	struct Entry {
		order: usize,
		line: usize,
		is_pub: bool,
		is_async: bool,
		text: String,
	}

	let mut edits = Vec::new();
	let mut mod002_fixable_lines = HashSet::new();
	let mut mod003_fixable_lines = HashSet::new();

	let mut idx = 0_usize;
	while idx < ctx.top_items.len() {
		let first = &ctx.top_items[idx];
		if is_cfg_test_mod_item(first) || !is_reorderable_kind(first.kind) {
			idx += 1;
			continue;
		}

		let kind = first.kind;
		let mut end = idx + 1;
		while end < ctx.top_items.len() {
			let candidate = &ctx.top_items[end];
			if candidate.kind != kind || is_cfg_test_mod_item(candidate) {
				break;
			}
			end += 1;
		}

		let run = &ctx.top_items[idx..end];
		if run.len() < 2 {
			idx = end;
			continue;
		}
		if run.windows(2).any(|pair| !separator_blank_only(ctx, &pair[0], &pair[1])) {
			idx = end;
			continue;
		}

		let mut entries = Vec::with_capacity(run.len());
		let mut collect_failed = false;
		for (offset, item) in run.iter().enumerate() {
			let Some((start, end_offset)) = item_text_range(ctx, item) else {
				collect_failed = true;
				break;
			};
			let Some(slice) = ctx.text.get(start..end_offset) else {
				collect_failed = true;
				break;
			};
			entries.push(Entry {
				order: offset,
				line: item.line,
				is_pub: item.is_pub,
				is_async: item.is_async,
				text: slice.trim_end_matches('\n').to_owned(),
			});
		}
		if collect_failed {
			idx = end;
			continue;
		}

		let mut run_mod002_lines = Vec::new();
		let mut seen_non_pub = false;
		for entry in &entries {
			if entry.is_pub {
				if seen_non_pub {
					run_mod002_lines.push(entry.line);
				}
			} else {
				seen_non_pub = true;
			}
		}

		let mut run_mod003_lines = Vec::new();
		if kind == TopKind::Fn {
			let mut seen_async_by_visibility = HashMap::new();
			seen_async_by_visibility.insert(true, false);
			seen_async_by_visibility.insert(false, false);
			for entry in &entries {
				if entry.is_async {
					seen_async_by_visibility.insert(entry.is_pub, true);
				} else if seen_async_by_visibility.get(&entry.is_pub).copied().unwrap_or(false) {
					run_mod003_lines.push(entry.line);
				}
			}
		}
		if run_mod002_lines.is_empty() && run_mod003_lines.is_empty() {
			idx = end;
			continue;
		}

		let mut ordered = entries.clone();
		if kind == TopKind::Fn {
			ordered.sort_by_key(|entry| {
				(if entry.is_pub { 0 } else { 1 }, if entry.is_async { 1 } else { 0 }, entry.order)
			});
		} else {
			ordered.sort_by_key(|entry| (if entry.is_pub { 0 } else { 1 }, entry.order));
		}
		if entries.iter().map(|entry| entry.order).collect::<Vec<_>>()
			== ordered.iter().map(|entry| entry.order).collect::<Vec<_>>()
		{
			idx = end;
			continue;
		}

		let Some((run_start, _)) = item_text_range(ctx, &run[0]) else {
			idx = end;
			continue;
		};
		let run_end = item_text_range(ctx, &run[run.len() - 1])
			.map(|(_, end_offset)| end_offset)
			.unwrap_or(run_start);
		let original = ctx.text.get(run_start..run_end).unwrap_or_default();

		let mut replacement = String::new();
		for (position, entry) in ordered.iter().enumerate() {
			if position > 0 {
				replacement.push_str("\n\n");
			}
			replacement.push_str(&entry.text);
		}
		if original.ends_with('\n') {
			replacement.push('\n');
		}

		let has_mod002_violation = !run_mod002_lines.is_empty();
		let has_mod003_violation = !run_mod003_lines.is_empty();
		for line in run_mod002_lines {
			mod002_fixable_lines.insert(line);
		}
		for line in run_mod003_lines {
			mod003_fixable_lines.insert(line);
		}
		edits.push(Edit {
			start: run_start,
			end: run_end,
			replacement,
			rule: if kind == TopKind::Fn && has_mod003_violation && !has_mod002_violation {
				"RUST-STYLE-MOD-003"
			} else {
				"RUST-STYLE-MOD-002"
			},
		});

		idx = end;
	}

	(edits, mod002_fixable_lines, mod003_fixable_lines)
}

pub(crate) fn check_module_order(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	let items_for_order = ctx
		.top_items
		.iter()
		.filter(|item| !(item.kind == TopKind::Mod && is_cfg_test_attrs(&item.attrs)))
		.collect::<Vec<_>>();
	let (planned_reorder_edits, mod002_fixable_lines, mod003_fixable_lines) =
		build_module_reorder_plans(ctx);
	if emit_edits {
		edits.extend(planned_reorder_edits);
	}

	let mut order_seen: Vec<usize> = Vec::new();
	for item in &items_for_order {
		let Some(order) = order_bucket(item.kind) else {
			continue;
		};
		if let Some(last) = order_seen.last().copied() {
			if order < last {
				push_violation(
					violations,
					ctx,
					item.line,
					"RUST-STYLE-MOD-001",
					"Top-level module item order does not match rust.md order.",
					false,
				);
			}
		}
		order_seen.push(order);
	}

	let mut non_pub_seen: HashMap<TopKind, bool> = HashMap::new();
	for item in &items_for_order {
		let seen_non_pub = non_pub_seen.get(&item.kind).copied().unwrap_or(false);
		if item.is_pub {
			if seen_non_pub {
				push_violation(
					violations,
					ctx,
					item.line,
					"RUST-STYLE-MOD-002",
					"Place pub items before non-pub items within the same group.",
					mod002_fixable_lines.contains(&item.line),
				);
			}
		} else {
			non_pub_seen.insert(item.kind, true);
		}
	}

	let mut async_seen = HashMap::new();
	async_seen.insert(true, false);
	async_seen.insert(false, false);
	for item in &items_for_order {
		if item.kind != TopKind::Fn {
			continue;
		}
		let key = item.is_pub;
		if item.is_async {
			async_seen.insert(key, true);
		} else if async_seen.get(&key).copied().unwrap_or(false) {
			push_violation(
				violations,
				ctx,
				item.line,
				"RUST-STYLE-MOD-003",
				"Place non-async functions before async functions at the same visibility.",
				mod003_fixable_lines.contains(&item.line),
			);
		}
	}

	let mut last_non_test_idx: Option<usize> = None;
	for (idx, item) in ctx.top_items.iter().enumerate() {
		if !(item.kind == TopKind::Mod && is_cfg_test_attrs(&item.attrs)) {
			last_non_test_idx = Some(idx);
		}
	}

	if let Some(last_non_test_idx) = last_non_test_idx {
		for (idx, item) in ctx.top_items.iter().enumerate() {
			if !(item.kind == TopKind::Mod && is_cfg_test_attrs(&item.attrs)) {
				continue;
			}
			if idx < last_non_test_idx {
				push_violation(
					violations,
					ctx,
					item.line,
					"RUST-STYLE-MOD-001",
					"Place #[cfg(test)] modules after all non-test items.",
					false,
				);
			}
		}
	}
}

pub(crate) fn check_cfg_test_mod_tests_use_super(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	fn leading_whitespace(line: &str) -> String {
		line.chars().take_while(|ch| ch.is_whitespace()).collect()
	}
	#[derive(Clone)]
	struct LeadingUseEntry {
		order: usize,
		start: usize,
		end: usize,
		origin: usize,
		is_super_glob: bool,
		is_super_specific: bool,
		block: String,
	}

	for item in ctx.source_file.items() {
		let ast::Item::Module(module) = item else {
			continue;
		};
		if !module
			.attrs()
			.any(|attr| attr.syntax().text().to_string().replace(' ', "").contains("cfg(test)"))
		{
			continue;
		}

		let name = module.name().map(|name| name.text().to_string()).unwrap_or_default();
		if name == "_test" {
			continue;
		}
		if name != "tests" {
			continue;
		}
		let Some(item_list) = module.item_list() else {
			continue;
		};

		let mut found_super_use = false;
		for nested in item_list.items() {
			let ast::Item::Use(use_item) = nested else {
				continue;
			};
			if let Some(path) =
				use_item.use_tree().map(|tree| tree.syntax().text().to_string().replace(' ', ""))
			{
				if path == "super::*" {
					found_super_use = true;
					break;
				}
			}
		}

		if !found_super_use {
			let line = line_from_offset(
				&ctx.line_starts,
				usize::from(module.syntax().text_range().start()),
			);
			push_violation(
				violations,
				ctx,
				line,
				"RUST-STYLE-MOD-007",
				"#[cfg(test)] mod tests should include `#[allow(unused_imports)] use super::*;` unless it is a keep-alive module.",
				true,
			);

			if emit_edits {
				let items = item_list.items().collect::<Vec<_>>();
				let indent = items
					.first()
					.and_then(|nested_item| {
						let nested_line = line_from_offset(
							&ctx.line_starts,
							usize::from(nested_item.syntax().text_range().start()),
						);
						ctx.lines
							.get(nested_line.saturating_sub(1))
							.map(|line_text| leading_whitespace(line_text))
					})
					.unwrap_or_else(|| "\t".to_owned());
				let super_block =
					format!("{indent}#[allow(unused_imports)]\n{indent}use super::*;");

				let mut leading_use_end = None;
				let mut leading_use_origin = None;
				let mut first_non_use_start = None;
				for nested in &items {
					match nested {
						ast::Item::Use(use_item) if first_non_use_start.is_none() => {
							leading_use_end = Some(usize::from(nested.syntax().text_range().end()));
							leading_use_origin = Some(use_item_origin(use_item));
						},
						_ => {
							first_non_use_start =
								Some(usize::from(nested.syntax().text_range().start()));
							break;
						},
					}
				}

				match leading_use_end {
					Some(gap_start) => {
						let gap_end = first_non_use_start.unwrap_or(gap_start);
						let before_sep = if leading_use_origin == Some(2) { "\n" } else { "\n\n" };
						let gap = ctx.text.get(gap_start..gap_end).unwrap_or_default();

						if gap.chars().all(char::is_whitespace) {
							let after_sep = if let Some(next_start) = first_non_use_start {
								let next_line = line_from_offset(&ctx.line_starts, next_start);
								let next_indent = ctx
									.lines
									.get(next_line.saturating_sub(1))
									.map(|line_text| leading_whitespace(line_text))
									.unwrap_or_else(|| indent.clone());
								format!("\n\n{next_indent}")
							} else {
								"\n".to_owned()
							};
							edits.push(Edit {
								start: gap_start,
								end: gap_end,
								replacement: format!("{before_sep}{super_block}{after_sep}"),
								rule: "RUST-STYLE-MOD-007",
							});
						} else {
							edits.push(Edit {
								start: gap_start,
								end: gap_start,
								replacement: format!("{before_sep}{super_block}"),
								rule: "RUST-STYLE-MOD-007",
							});
						}
					},
					None => {
						let list_start = usize::from(item_list.syntax().text_range().start());
						let insert_pos = ctx.text[list_start..]
							.find('\n')
							.map(|offset| list_start + offset + 1)
							.unwrap_or(list_start + 1);
						let trailing_newlines = if items.is_empty() { "\n" } else { "\n\n" };
						edits.push(Edit {
							start: insert_pos,
							end: insert_pos,
							replacement: format!("{super_block}{trailing_newlines}"),
							rule: "RUST-STYLE-MOD-007",
						});
					},
				}
			}
		} else {
			let items = item_list.items().collect::<Vec<_>>();
			let indent = items
				.first()
				.and_then(|nested_item| {
					let nested_line = line_from_offset(
						&ctx.line_starts,
						usize::from(nested_item.syntax().text_range().start()),
					);
					ctx.lines
						.get(nested_line.saturating_sub(1))
						.map(|line_text| leading_whitespace(line_text))
				})
				.unwrap_or_else(|| "\t".to_owned());

			let mut leading_uses = Vec::new();
			let mut first_non_use_start = None;
			let mut can_rewrite = true;
			for nested in &items {
				match nested {
					ast::Item::Use(use_item) if first_non_use_start.is_none() => {
						let start = usize::from(nested.syntax().text_range().start());
						let end = usize::from(nested.syntax().text_range().end());
						let Some(raw_block) = ctx.text.get(start..end) else {
							can_rewrite = false;
							break;
						};
						let trimmed_block = raw_block.trim_end_matches('\n');
						let block = if trimmed_block.chars().next().is_some_and(char::is_whitespace)
						{
							trimmed_block.to_owned()
						} else {
							format!("{indent}{trimmed_block}")
						};
						let compact_path = use_item
							.use_tree()
							.map(|tree| tree.syntax().text().to_string().replace(' ', ""))
							.unwrap_or_default();
						let is_super_glob = compact_path == "super::*";
						let is_super_specific =
							compact_path.starts_with("super::") && !is_super_glob;
						leading_uses.push(LeadingUseEntry {
							order: leading_uses.len(),
							start,
							end,
							origin: use_item_origin(use_item),
							is_super_glob,
							is_super_specific,
							block,
						});
					},
					_ => {
						first_non_use_start =
							Some(usize::from(nested.syntax().text_range().start()));
						break;
					},
				}
			}
			if !can_rewrite || leading_uses.len() < 2 {
				continue;
			}
			let has_super_glob = leading_uses.iter().any(|entry| entry.is_super_glob);
			let has_super_specific = leading_uses.iter().any(|entry| entry.is_super_specific);

			let out_of_order =
				leading_uses.windows(2).position(|pair| pair[1].origin < pair[0].origin);
			let Some(bad_pair_idx) = out_of_order else {
				if !(has_super_glob && has_super_specific) {
					continue;
				}
				push_violation(
					violations,
					ctx,
					leading_uses
						.iter()
						.find(|entry| entry.is_super_specific)
						.map(|entry| line_from_offset(&ctx.line_starts, entry.start))
						.unwrap_or_else(|| {
							line_from_offset(&ctx.line_starts, leading_uses[0].start)
						}),
					"RUST-STYLE-MOD-007",
					"In #[cfg(test)] mod tests, prefer `use super::*;` and remove specific super imports.",
					true,
				);
				if !emit_edits {
					continue;
				}

				let mut filtered = Vec::new();
				let mut kept_super_glob = false;
				for entry in &leading_uses {
					if entry.is_super_specific {
						continue;
					}
					if entry.is_super_glob {
						if kept_super_glob {
							continue;
						}
						kept_super_glob = true;
					}
					filtered.push(entry.clone());
				}
				if filtered.is_empty() {
					continue;
				}

				filtered.sort_by_key(|entry| (entry.origin, entry.order));
				let mut replacement = String::new();
				for (idx, entry) in filtered.iter().enumerate() {
					if idx > 0 {
						if filtered[idx - 1].origin == entry.origin {
							replacement.push('\n');
						} else {
							replacement.push_str("\n\n");
						}
					}
					replacement.push_str(&entry.block);
				}

				let edit_start = leading_uses
					.first()
					.and_then(|entry| {
						let line = line_from_offset(&ctx.line_starts, entry.start);
						offset_from_line(&ctx.line_starts, line)
					})
					.unwrap_or_default();
				let last_use_end = leading_uses.last().map(|entry| entry.end).unwrap_or(edit_start);
				let mut edit_end = last_use_end;
				if let Some(next_start) = first_non_use_start {
					let gap = ctx.text.get(last_use_end..next_start).unwrap_or_default();
					if gap.chars().all(char::is_whitespace) {
						replacement.push_str("\n\n");
						let next_line = line_from_offset(&ctx.line_starts, next_start);
						edit_end =
							offset_from_line(&ctx.line_starts, next_line).unwrap_or(next_start);
					}
				}

				edits.push(Edit {
					start: edit_start,
					end: edit_end,
					replacement,
					rule: "RUST-STYLE-MOD-007",
				});
				continue;
			};
			push_violation(
				violations,
				ctx,
				line_from_offset(&ctx.line_starts, leading_uses[bad_pair_idx + 1].start),
				"RUST-STYLE-MOD-007",
				"In #[cfg(test)] mod tests, order imports as std, third-party, self/workspace.",
				true,
			);

			if emit_edits {
				let mut ordered = Vec::new();
				let mut kept_super_glob = false;
				for entry in &leading_uses {
					if has_super_glob && entry.is_super_specific {
						continue;
					}
					if entry.is_super_glob {
						if kept_super_glob {
							continue;
						}
						kept_super_glob = true;
					}
					ordered.push(entry.clone());
				}
				ordered.sort_by_key(|entry| (entry.origin, entry.order));

				let mut replacement = String::new();
				for (idx, entry) in ordered.iter().enumerate() {
					if idx > 0 {
						if ordered[idx - 1].origin == entry.origin {
							replacement.push('\n');
						} else {
							replacement.push_str("\n\n");
						}
					}
					replacement.push_str(&entry.block);
				}

				let edit_start = leading_uses
					.first()
					.and_then(|entry| {
						let line = line_from_offset(&ctx.line_starts, entry.start);
						offset_from_line(&ctx.line_starts, line)
					})
					.unwrap_or_default();
				let last_use_end = leading_uses.last().map(|entry| entry.end).unwrap_or(edit_start);
				let mut edit_end = last_use_end;
				if let Some(next_start) = first_non_use_start {
					let gap = ctx.text.get(last_use_end..next_start).unwrap_or_default();
					if gap.chars().all(char::is_whitespace) {
						replacement.push_str("\n\n");
						let next_line = line_from_offset(&ctx.line_starts, next_start);
						edit_end =
							offset_from_line(&ctx.line_starts, next_line).unwrap_or(next_start);
					}
				}

				edits.push(Edit {
					start: edit_start,
					end: edit_end,
					replacement,
					rule: "RUST-STYLE-MOD-007",
				});
			}
		}
	}
}
