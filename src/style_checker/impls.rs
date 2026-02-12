use std::collections::HashMap;

use ra_ap_syntax::{
	AstNode, TextRange,
	ast::{self, HasModuleItem, HasTypeBounds},
};
use regex::Regex;

use super::shared::{
	Edit, FileContext, INLINE_BOUNDS_RE, TopKind, Violation, extract_impl_target_name,
	line_from_offset, offset_from_line, push_violation, strip_string_and_line_comment,
};

fn is_path_separator_colon(text: &str, colon_offset: usize) -> bool {
	colon_offset > 0 && text.as_bytes().get(colon_offset - 1) == Some(&b':')
}

fn has_param_self_type_match(signature_text: &str, re: &Regex) -> bool {
	re.find_iter(signature_text)
		.any(|matched| !is_path_separator_colon(signature_text, matched.start()))
}

fn replace_param_self_types(signature_text: &str, re: &Regex) -> String {
	let mut out = String::with_capacity(signature_text.len());
	let mut cursor = 0_usize;

	for matched in re.find_iter(signature_text) {
		if is_path_separator_colon(signature_text, matched.start()) {
			continue;
		}
		out.push_str(&signature_text[cursor..matched.start()]);
		out.push_str(": Self");
		cursor = matched.end();
	}

	if cursor == 0 {
		return signature_text.to_owned();
	}

	out.push_str(&signature_text[cursor..]);
	out
}

fn classify_impl_trait_order(raw: &str) -> usize {
	let header = strip_string_and_line_comment(raw, false).0;
	let Some((left, _right)) = header.split_once(" for ") else {
		return 0;
	};
	let mut trait_part =
		left.split_once("impl").map(|(_, right)| right.trim().to_owned()).unwrap_or_default();
	if trait_part.starts_with('<') {
		if let Some((_, after)) = trait_part.split_once('>') {
			trait_part = after.trim().to_owned();
		}
	}
	let trait_name = trait_part.split(['<', ' ', '{']).next().unwrap_or_default().trim();
	if trait_name.starts_with("std::")
		|| trait_name.starts_with("core::")
		|| trait_name.starts_with("alloc::")
	{
		1
	} else if trait_name.starts_with("crate::")
		|| trait_name.starts_with("self::")
		|| trait_name.starts_with("super::")
		|| trait_name.starts_with("elf_")
	{
		3
	} else {
		2
	}
}

pub(crate) fn check_impl_adjacency(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	let mut type_indices: HashMap<String, usize> = HashMap::new();
	for (idx, item) in ctx.top_items.iter().enumerate() {
		if !(item.kind == TopKind::Struct || item.kind == TopKind::Enum) {
			continue;
		}
		if let Some(name) = &item.name {
			type_indices.insert(name.clone(), idx);
		}
	}

	let mut impl_by_target: HashMap<String, Vec<usize>> = HashMap::new();
	for (idx, item) in ctx.top_items.iter().enumerate() {
		if item.kind != TopKind::Impl {
			continue;
		}
		if let Some(target) = &item.impl_target {
			impl_by_target.entry(target.clone()).or_default().push(idx);
		}
	}

	for (target, impl_indices) in &impl_by_target {
		let first_impl = impl_indices[0];
		let last_impl = *impl_indices.last().unwrap_or(&first_impl);

		for idx in first_impl..=last_impl {
			let item = &ctx.top_items[idx];
			if item.kind != TopKind::Impl || item.impl_target.as_deref() != Some(target.as_str()) {
				push_violation(
					violations,
					ctx,
					item.line,
					"RUST-STYLE-IMPL-003",
					&format!("impl blocks for `{target}` must be contiguous."),
					false,
				);
				break;
			}
		}

		let mut order_values = Vec::new();
		for idx in impl_indices {
			order_values.push(classify_impl_trait_order(&ctx.top_items[*idx].raw));
		}
		for pos in 1..order_values.len() {
			if order_values[pos] < order_values[pos - 1] {
				push_violation(
					violations,
					ctx,
					ctx.top_items[impl_indices[pos]].line,
					"RUST-STYLE-IMPL-003",
					&format!(
						"impl block order for `{target}` must be inherent, std traits, third-party traits, then workspace-member traits."
					),
					false,
				);
				break;
			}
		}
	}

	for (type_name, type_idx) in type_indices {
		let Some(impl_indices) = impl_by_target.get(&type_name) else {
			continue;
		};
		if impl_indices.is_empty() {
			continue;
		}
		let first_impl = impl_indices[0];
		if first_impl != type_idx + 1 {
			push_violation(
				violations,
				ctx,
				ctx.top_items[first_impl].line,
				"RUST-STYLE-MOD-005",
				&format!("Keep `{type_name}` definitions and related impl blocks adjacent."),
				false,
			);
			continue;
		}

		let type_end = ctx.top_items[type_idx].end_line;
		let impl_start = ctx.top_items[first_impl].start_line;
		if impl_start > type_end + 1 {
			let between = &ctx.lines[type_end..impl_start.saturating_sub(1)];
			if between.iter().any(|line| line.trim().is_empty()) {
				let can_autofix = between.iter().all(|line| line.trim().is_empty());
				push_violation(
					violations,
					ctx,
					ctx.top_items[first_impl].line,
					"RUST-STYLE-MOD-005",
					&format!(
						"Do not insert blank lines between `{type_name}` and its first impl block."
					),
					can_autofix,
				);
				if emit_edits && can_autofix {
					let start = offset_from_line(&ctx.line_starts, type_end + 1).unwrap_or(0);
					let end = offset_from_line(&ctx.line_starts, impl_start).unwrap_or(start);
					if end > start {
						edits.push(Edit {
							start,
							end,
							replacement: String::new(),
							rule: "RUST-STYLE-MOD-005",
						});
					}
				}
			}
		}
	}
}

pub(crate) fn check_impl_rules(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	for item in ctx.source_file.items() {
		let ast::Item::Impl(impl_item) = item else {
			continue;
		};
		let Some(self_ty) = impl_item.self_ty() else {
			continue;
		};
		let Some(target) = extract_impl_target_name(&self_ty.syntax().text().to_string()) else {
			continue;
		};

		let qualified_target = format!(
			r"(?:{}\b|(?:crate|self|super)::(?:[A-Za-z_][A-Za-z0-9_]*::)*{}\b)",
			regex::escape(&target),
			regex::escape(&target)
		);
		let return_self_type_re = Regex::new(&format!(r"->\s*{qualified_target}")).unwrap();
		let param_self_type_re = Regex::new(&format!(r":\s*{qualified_target}")).unwrap();

		let Some(items) = impl_item.assoc_item_list() else {
			continue;
		};
		for assoc in items.assoc_items() {
			let ast::AssocItem::Fn(function) = assoc else {
				continue;
			};
			let signature_text = if let Some(body) = function.body() {
				let sig_range = TextRange::new(
					function.syntax().text_range().start(),
					body.syntax().text_range().start(),
				);
				ctx.text[sig_range].to_owned()
			} else {
				function.syntax().text().to_string()
			};

			let has_return_match = return_self_type_re.is_match(&signature_text);
			let has_param_match = has_param_self_type_match(&signature_text, &param_self_type_re);
			if has_return_match || has_param_match {
				let line = line_from_offset(
					&ctx.line_starts,
					usize::from(function.syntax().text_range().start()),
				);
				push_violation(
					violations,
					ctx,
					line,
					"RUST-STYLE-IMPL-001",
					&format!(
						"Use Self instead of concrete type `{target}` in impl method signatures."
					),
					true,
				);

				if emit_edits {
					let replaced =
						return_self_type_re.replace_all(&signature_text, "-> Self").to_string();
					let replaced = replace_param_self_types(&replaced, &param_self_type_re);
					let start = usize::from(function.syntax().text_range().start());
					let end = start + signature_text.len();
					edits.push(Edit {
						start,
						end,
						replacement: replaced,
						rule: "RUST-STYLE-IMPL-001",
					});
				}
			}
		}
	}
}

pub(crate) fn check_inline_trait_bounds(ctx: &FileContext, violations: &mut Vec<Violation>) {
	for item in ctx.source_file.syntax().descendants().filter_map(ast::GenericParamList::cast) {
		for param in item.generic_params() {
			if let ast::GenericParam::TypeParam(type_param) = param {
				if type_param.type_bound_list().is_some() {
					let line = line_from_offset(
						&ctx.line_starts,
						usize::from(type_param.syntax().text_range().start()),
					);
					push_violation(
						violations,
						ctx,
						line,
						"RUST-STYLE-GENERICS-001",
						"Inline trait bounds are not allowed. Move bounds into a where clause.",
						false,
					);
				}
			}
		}
	}

	for (idx, line) in ctx.lines.iter().enumerate() {
		let code = strip_string_and_line_comment(line, false).0;
		if INLINE_BOUNDS_RE.is_match(&code) {
			push_violation(
				violations,
				ctx,
				idx + 1,
				"RUST-STYLE-GENERICS-001",
				"Inline trait bounds are not allowed. Move bounds into a where clause.",
				false,
			);
		}
	}
}
