use std::collections::{HashMap, HashSet};

use ra_ap_syntax::{
	AstNode, TextRange,
	ast::{self, HasModuleItem, HasTypeBounds},
};
use regex::Regex;

use crate::style::shared::{
	self, Edit, FileContext, TopItem, TopKind, Violation, WORKSPACE_IMPORT_ROOTS,
};

pub(crate) fn check_impl_adjacency(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	let type_indices = collect_type_indices(ctx);
	let impl_by_target = collect_impl_indices_by_target(ctx);

	check_impl_contiguity_and_order(ctx, violations, edits, emit_edits, &impl_by_target);

	for (type_name, type_idx) in type_indices {
		let Some(impl_indices) = impl_by_target.get(&type_name) else {
			continue;
		};

		if impl_indices.is_empty() {
			continue;
		}

		check_type_impl_adjacency(
			ctx,
			violations,
			edits,
			emit_edits,
			&type_name,
			type_idx,
			impl_indices,
		);
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

		if impl_item.trait_().is_some() {
			continue;
		}

		let Some(self_ty) = impl_item.self_ty() else {
			continue;
		};
		let Some(target) = shared::extract_impl_target_name(&self_ty.syntax().text().to_string())
		else {
			continue;
		};
		let qualified_target = format!(
			r"(?:{}\b|(?:crate|self|super)::(?:[A-Za-z_][A-Za-z0-9_]*::)*{}\b)",
			regex::escape(&target),
			regex::escape(&target)
		);
		let return_self_type_re = Regex::new(&format!(r"->\s*{qualified_target}"))
			.expect("Compile impl return-type self-reference regex.");
		let param_self_type_re = Regex::new(&format!(r":\s*{qualified_target}"))
			.expect("Compile impl parameter self-reference regex.");
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
			let has_return_match =
				has_return_self_type_match(&signature_text, &return_self_type_re);
			let has_param_match = has_param_self_type_match(&signature_text, &param_self_type_re);

			if has_return_match || has_param_match {
				let line = shared::line_from_offset(
					&ctx.line_starts,
					usize::from(function.syntax().text_range().start()),
				);

				shared::push_violation(
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
					let replaced = replace_return_self_types(&signature_text, &return_self_type_re);
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
			if let ast::GenericParam::TypeParam(type_param) = param
				&& type_param.type_bound_list().is_some()
			{
				let line = shared::line_from_offset(
					&ctx.line_starts,
					usize::from(type_param.syntax().text_range().start()),
				);

				shared::push_violation(
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

fn collect_type_indices(ctx: &FileContext) -> HashMap<String, usize> {
	let mut type_indices = HashMap::new();

	for (idx, item) in ctx.top_items.iter().enumerate() {
		if !(item.kind == TopKind::Struct || item.kind == TopKind::Enum) {
			continue;
		}

		if let Some(name) = &item.name {
			type_indices.insert(name.clone(), idx);
		}
	}

	type_indices
}

fn collect_impl_indices_by_target(ctx: &FileContext) -> HashMap<String, Vec<usize>> {
	let mut impl_by_target: HashMap<String, Vec<usize>> = HashMap::new();

	for (idx, item) in ctx.top_items.iter().enumerate() {
		if item.kind != TopKind::Impl {
			continue;
		}

		if let Some(target) = &item.impl_target {
			impl_by_target.entry(target.clone()).or_default().push(idx);
		}
	}

	impl_by_target
}

fn check_impl_contiguity_and_order(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	impl_by_target: &HashMap<String, Vec<usize>>,
) {
	let trait_order_hints = collect_impl_trait_order_hints(ctx);
	let local_trait_names = collect_local_trait_names(ctx);
	let trait_ctx = (&trait_order_hints, &local_trait_names);

	for (target, impl_indices) in impl_by_target {
		check_target_impl_contiguous(ctx, violations, target, impl_indices);
		check_target_impl_order(
			ctx,
			violations,
			edits,
			emit_edits,
			target,
			impl_indices,
			trait_ctx,
		);
	}
}

fn check_target_impl_contiguous(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	target: &str,
	impl_indices: &[usize],
) {
	let first_impl = impl_indices[0];
	let last_impl = *impl_indices.last().unwrap_or(&first_impl);

	for idx in first_impl..=last_impl {
		let item = &ctx.top_items[idx];

		if item.kind != TopKind::Impl || item.impl_target.as_deref() != Some(target) {
			shared::push_violation(
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
}

fn check_target_impl_order(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	target: &str,
	impl_indices: &[usize],
	trait_ctx: (&HashMap<String, HashSet<usize>>, &HashSet<String>),
) {
	let mut order_values = Vec::with_capacity(impl_indices.len());
	let mut order_by_impl_index = HashMap::new();

	for idx in impl_indices {
		let order = classify_impl_trait_order(&ctx.top_items[*idx].raw, trait_ctx.0, trait_ctx.1);

		order_values.push(order);
		order_by_impl_index.insert(*idx, order);
	}

	let mut desired_indices = impl_indices.to_vec();

	desired_indices.sort_by_key(|idx| (order_by_impl_index.get(idx).copied().unwrap_or(0), *idx));

	if desired_indices == impl_indices {
		return;
	}

	let line =
		impl_indices
			.iter()
			.enumerate()
			.find_map(|(pos, idx)| {
				if *idx != desired_indices[pos] { Some(ctx.top_items[*idx].line) } else { None }
			})
			.unwrap_or_else(|| ctx.top_items[impl_indices[0]].line);
	let can_autofix = impl_indices_are_contiguous(impl_indices)
		&& impl_run_has_blank_only_separators(ctx, impl_indices);

	shared::push_violation(
		violations,
		ctx,
		line,
		"RUST-STYLE-IMPL-003",
		&format!(
			"impl block order for `{target}` must be inherent, std traits, third-party traits, then workspace-member traits."
		),
		can_autofix,
	);

	if !emit_edits || !can_autofix {
		return;
	}

	if let Some(edit) = build_impl_order_rewrite_edit(ctx, impl_indices, &desired_indices) {
		edits.push(edit);
	}
}

fn impl_indices_are_contiguous(impl_indices: &[usize]) -> bool {
	impl_indices.windows(2).all(|pair| pair[1] == pair[0].saturating_add(1))
}

fn impl_run_has_blank_only_separators(ctx: &FileContext, impl_indices: &[usize]) -> bool {
	impl_indices.windows(2).all(|pair| {
		let prev = &ctx.top_items[pair[0]];
		let curr = &ctx.top_items[pair[1]];
		let start = prev.end_line;
		let end = curr.start_line.saturating_sub(1);

		if start >= end || end > ctx.lines.len() {
			return true;
		}

		ctx.lines[start..end].iter().all(|line| line.trim().is_empty())
	})
}

fn build_impl_order_rewrite_edit(
	ctx: &FileContext,
	impl_indices: &[usize],
	desired_indices: &[usize],
) -> Option<Edit> {
	let first_item = &ctx.top_items[*impl_indices.first()?];
	let last_item = &ctx.top_items[*impl_indices.last()?];
	let (start, _) = top_item_text_range(ctx, first_item)?;
	let (_, end) = top_item_text_range(ctx, last_item)?;
	let mut blocks_by_index = HashMap::new();

	for idx in impl_indices {
		let item = &ctx.top_items[*idx];
		let (item_start, item_end) = top_item_text_range(ctx, item)?;
		let block = ctx.text.get(item_start..item_end)?.trim_end_matches('\n').to_owned();

		if block.is_empty() {
			return None;
		}

		blocks_by_index.insert(*idx, block);
	}

	let mut replacement = String::new();

	for (pos, idx) in desired_indices.iter().enumerate() {
		if pos > 0 {
			replacement.push_str("\n\n");
		}

		replacement.push_str(blocks_by_index.get(idx)?);
	}

	replacement.push('\n');

	Some(Edit { start, end, replacement, rule: "RUST-STYLE-IMPL-003" })
}

fn check_type_impl_adjacency(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	type_name: &str,
	type_idx: usize,
	impl_indices: &[usize],
) {
	let first_impl = impl_indices[0];

	if first_impl != type_idx + 1 {
		let can_relocate = impl_indices.windows(2).all(|pair| pair[1] == pair[0].saturating_add(1));

		shared::push_violation(
			violations,
			ctx,
			ctx.top_items[first_impl].line,
			"RUST-STYLE-MOD-005",
			&format!("Keep `{type_name}` definitions and related impl blocks adjacent."),
			can_relocate,
		);

		if emit_edits && can_relocate {
			push_impl_relocation_edits(ctx, edits, type_idx, impl_indices);
		}

		return;
	}

	let type_end = ctx.top_items[type_idx].end_line;
	let impl_start = ctx.top_items[first_impl].start_line;

	if impl_start > type_end + 1 {
		push_blank_between_type_and_impl_violation(
			ctx, violations, edits, emit_edits, type_name, type_idx, first_impl,
		);
	}
}

fn push_impl_relocation_edits(
	ctx: &FileContext,
	edits: &mut Vec<Edit>,
	type_idx: usize,
	impl_indices: &[usize],
) {
	let first_impl_idx = *impl_indices.first().unwrap_or(&impl_indices[0]);
	let last_impl_idx = *impl_indices.last().unwrap_or(&first_impl_idx);
	let first_impl_item = &ctx.top_items[first_impl_idx];
	let last_impl_item = &ctx.top_items[last_impl_idx];
	let Some((block_start, _)) = top_item_text_range(ctx, first_impl_item) else {
		return;
	};
	let Some((_, cluster_end)) = top_item_text_range(ctx, last_impl_item) else {
		return;
	};
	let insert_start_line = ctx.top_items[type_idx].end_line + 1;
	let insert_start = line_start_offset_or_eof(ctx, insert_start_line);
	let insert_end = line_start_offset_or_eof(ctx, next_non_blank_line(ctx, insert_start_line));
	let delete_end =
		line_start_offset_or_eof(ctx, next_non_blank_line(ctx, last_impl_item.end_line + 1));

	if insert_start >= block_start && insert_start <= delete_end {
		return;
	}

	let block = ctx
		.text
		.get(block_start..cluster_end)
		.unwrap_or_default()
		.trim_end_matches('\n')
		.to_owned();

	if block.is_empty() {
		return;
	}

	edits.push(Edit {
		start: block_start,
		end: delete_end.max(cluster_end),
		replacement: String::new(),
		rule: "RUST-STYLE-MOD-005",
	});
	edits.push(Edit {
		start: insert_start,
		end: insert_end,
		replacement: if insert_end < ctx.text.len() {
			format!("{block}\n\n")
		} else {
			format!("{block}\n")
		},
		rule: "RUST-STYLE-MOD-005",
	});
}

fn next_non_blank_line(ctx: &FileContext, mut line_one_based: usize) -> usize {
	while line_one_based <= ctx.lines.len() && ctx.lines[line_one_based - 1].trim().is_empty() {
		line_one_based += 1;
	}

	line_one_based
}

fn push_blank_between_type_and_impl_violation(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	type_name: &str,
	type_idx: usize,
	first_impl: usize,
) {
	let type_end = ctx.top_items[type_idx].end_line;
	let impl_start = ctx.top_items[first_impl].start_line;
	let between = &ctx.lines[type_end..impl_start.saturating_sub(1)];

	if !between.iter().any(|line| line.trim().is_empty()) {
		return;
	}

	let can_autofix = between.iter().all(|line| line.trim().is_empty());

	shared::push_violation(
		violations,
		ctx,
		ctx.top_items[first_impl].line,
		"RUST-STYLE-MOD-005",
		&format!("Do not insert blank lines between `{type_name}` and its first impl block."),
		can_autofix,
	);

	if emit_edits && can_autofix {
		let start = shared::offset_from_line(&ctx.line_starts, type_end + 1).unwrap_or(0);
		let end = shared::offset_from_line(&ctx.line_starts, impl_start).unwrap_or(start);

		if end > start {
			edits.push(Edit { start, end, replacement: String::new(), rule: "RUST-STYLE-MOD-005" });
		}
	}
}

fn line_start_offset_or_eof(ctx: &FileContext, line_one_based: usize) -> usize {
	shared::offset_from_line(&ctx.line_starts, line_one_based).unwrap_or(ctx.text.len())
}

fn is_path_separator_colon(text: &str, colon_offset: usize) -> bool {
	colon_offset > 0 && text.as_bytes().get(colon_offset - 1) == Some(&b':')
}

fn has_immediate_type_args(text: &str, mut end_offset: usize) -> bool {
	let bytes = text.as_bytes();

	while end_offset < bytes.len() && bytes[end_offset].is_ascii_whitespace() {
		end_offset += 1;
	}

	bytes.get(end_offset) == Some(&b'<')
}

fn has_return_self_type_match(signature_text: &str, re: &Regex) -> bool {
	re.find_iter(signature_text)
		.any(|matched| !has_immediate_type_args(signature_text, matched.end()))
}

fn has_param_self_type_match(signature_text: &str, re: &Regex) -> bool {
	re.find_iter(signature_text).any(|matched| {
		!is_path_separator_colon(signature_text, matched.start())
			&& !has_immediate_type_args(signature_text, matched.end())
	})
}

fn replace_return_self_types(signature_text: &str, re: &Regex) -> String {
	let mut out = String::with_capacity(signature_text.len());
	let mut cursor = 0_usize;

	for matched in re.find_iter(signature_text) {
		if has_immediate_type_args(signature_text, matched.end()) {
			continue;
		}

		out.push_str(&signature_text[cursor..matched.start()]);
		out.push_str("-> Self");

		cursor = matched.end();
	}

	if cursor == 0 {
		return signature_text.to_owned();
	}

	out.push_str(&signature_text[cursor..]);

	out
}

fn replace_param_self_types(signature_text: &str, re: &Regex) -> String {
	let mut out = String::with_capacity(signature_text.len());
	let mut cursor = 0_usize;

	for matched in re.find_iter(signature_text) {
		if is_path_separator_colon(signature_text, matched.start())
			|| has_immediate_type_args(signature_text, matched.end())
		{
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

fn classify_impl_trait_order(
	raw: &str,
	trait_order_hints: &HashMap<String, HashSet<usize>>,
	local_trait_names: &HashSet<String>,
) -> usize {
	let header = shared::strip_string_and_line_comment(raw, false).0;
	let Some((left, _right)) = header.split_once(" for ") else {
		return 0;
	};
	let mut trait_part =
		left.split_once("impl").map(|(_, right)| right.trim().to_owned()).unwrap_or_default();

	if trait_part.starts_with('<')
		&& let Some((_, after)) = trait_part.split_once('>')
	{
		trait_part = after.trim().to_owned();
	}

	let trait_name = trait_part.split(['<', ' ', '{']).next().unwrap_or_default().trim();

	if trait_name.starts_with("std::")
		|| trait_name.starts_with("core::")
		|| trait_name.starts_with("alloc::")
	{
		1
	} else if !trait_name.contains("::") && local_trait_names.contains(trait_name) {
		3
	} else if !trait_name.contains("::")
		&& let Some(hints) = trait_order_hints.get(trait_name)
		&& hints.len() == 1
	{
		*hints.iter().next().unwrap_or(&2)
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

fn collect_local_trait_names(ctx: &FileContext) -> HashSet<String> {
	let mut names = HashSet::new();

	for item in &ctx.top_items {
		if item.kind != TopKind::Trait {
			continue;
		}

		if let Some(name) = &item.name {
			names.insert(name.clone());
		}
	}

	names
}

fn collect_impl_trait_order_hints(ctx: &FileContext) -> HashMap<String, HashSet<usize>> {
	let mut hints: HashMap<String, HashSet<usize>> = HashMap::new();

	for item in &ctx.top_items {
		if item.kind != TopKind::Use {
			continue;
		}

		let Some(path) =
			item.use_path.clone().or_else(|| extract_use_path_from_item_text(&item.raw))
		else {
			continue;
		};
		let Some(root) = path.split("::").next() else {
			continue;
		};
		let root = normalize_ident(root.trim());
		let order = if matches!(root, "std" | "core" | "alloc") {
			1
		} else if matches!(root, "crate" | "self" | "super")
			|| WORKSPACE_IMPORT_ROOTS.contains(root)
			|| WORKSPACE_IMPORT_ROOTS.contains(&root.replace('-', "_"))
		{
			3
		} else {
			2
		};

		for symbol in imported_symbols_from_use_path(&path) {
			hints.entry(symbol).or_default().insert(order);
		}
	}

	hints
}

fn extract_use_path_from_item_text(text: &str) -> Option<String> {
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

		let bytes = text.as_bytes();
		let mut start = idx + 3;

		while start < bytes.len() && bytes[start].is_ascii_whitespace() {
			start += 1;
		}

		let tail = text.get(start..)?;
		let semi = tail.find(';')?;

		return Some((start, start + semi));
	}

	None
}

fn normalize_ident(name: &str) -> &str {
	name.strip_prefix("r#").unwrap_or(name)
}

fn imported_symbols_from_use_path(path: &str) -> Vec<String> {
	fn collect_symbols_from_segment(segment: &str, out: &mut Vec<String>) -> bool {
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

				if !collect_symbols_from_segment(&expanded, out) {
					return false;
				}
			}

			return true;
		}

		let mut symbol = trimmed.to_owned();

		if let Some((_left, right)) = symbol.split_once(" as ") {
			if right.trim() == "_" {
				return true;
			}

			symbol = right.trim().to_owned();
		}

		if matches!(symbol.as_str(), "*" | "self" | "super" | "crate") {
			return true;
		}

		if let Some((_, right)) = symbol.rsplit_once("::") {
			symbol = right.to_owned();
		}
		if let Some(stripped) = symbol.strip_prefix("r#") {
			symbol = stripped.to_owned();
		}

		if matches!(symbol.as_str(), "*" | "self" | "super" | "crate") || symbol.is_empty() {
			return true;
		}

		out.push(symbol);

		true
	}

	let mut out = Vec::new();

	if !collect_symbols_from_segment(path, &mut out) {
		return Vec::new();
	}

	out
}

fn split_top_level_csv(text: &str) -> Vec<String> {
	let chars = text.char_indices().collect::<Vec<_>>();
	let mut out = Vec::new();
	let mut start = 0_usize;
	let mut depth_brace = 0_i32;
	let mut depth_angle = 0_i32;

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

fn top_item_text_range(ctx: &FileContext, item: &TopItem) -> Option<(usize, usize)> {
	let start = shared::offset_from_line(&ctx.line_starts, item.start_line)?;
	let end =
		shared::offset_from_line(&ctx.line_starts, item.end_line + 1).unwrap_or(ctx.text.len());

	if end < start { None } else { Some((start, end)) }
}
