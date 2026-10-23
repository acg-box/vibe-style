use std::{collections::HashSet, sync::OnceLock};

use ra_ap_syntax::{
	AstNode as _, SyntaxNode,
	ast::{Attr, HasAttrs as _, Module, NameRef, Path, UseTree},
};
use regex::Regex;

use crate::style::{
	imports::BracedImportSegment,
	shared::{self, Edit, FileContext, TopItem},
	syntax,
};

pub(super) fn collect_non_keep_alive_alias_bindings(path: &str) -> Vec<(String, String)> {
	let mut out = Vec::new();
	let _ok = collect_alias_bindings_from_use_segment(path.trim(), &mut out);

	out.retain(|(alias, _)| alias != "_");

	out
}

pub(super) fn collect_alias_bindings_from_use_segment(
	segment: &str,
	out: &mut Vec<(String, String)>,
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

		let prefix = prefix.strip_suffix("::").unwrap_or(prefix).trim();

		for child in split_top_level_csv(inner) {
			let child = child.trim();

			if child.is_empty() {
				continue;
			}

			let expanded =
				if prefix.is_empty() { child.to_owned() } else { format!("{prefix}::{child}") };

			if !collect_alias_bindings_from_use_segment(expanded.as_str(), out) {
				return false;
			}
		}

		return true;
	}
	if let Some((base, alias)) = trimmed.rsplit_once(" as ") {
		let base = base.trim();
		let alias = alias.trim();

		if base.is_empty() || alias.is_empty() {
			return false;
		}

		out.push((alias.to_owned(), base.to_owned()));
	}

	true
}

pub(super) fn looks_like_trait_import(symbol: &str, full_path: &str) -> bool {
	let symbol_is_common_trait = matches!(
		symbol,
		"Read"
			| "Write"
			| "BufRead"
			| "Seek"
			| "AsyncRead"
			| "AsyncWrite"
			| "AsyncBufRead"
			| "AsyncSeek"
			| "Future"
			| "Stream"
			| "Sink"
			| "Iterator"
			| "IntoIterator"
			| "FromIterator"
			| "ParallelIterator"
			| "IntoParallelIterator"
			| "IntoParallelRefIterator"
			| "Serialize"
			| "Deserialize"
			| "Executor"
	);
	let symbol_is_trait_named =
		symbol.ends_with("Ext") || symbol.ends_with("Trait") || symbol_is_common_trait;
	let path_is_trait_named = full_path.contains("::trait::")
		|| full_path.contains("::traits::")
		|| full_path.contains("::prelude::")
		|| full_path.ends_with("::eyre::Context");

	(symbol_is_trait_named || path_is_trait_named)
		&& symbol.chars().next().is_some_and(char::is_uppercase)
}

pub(super) fn split_import_leaf_alias(leaf: &str) -> Option<(&str, Option<String>)> {
	let trimmed = leaf.trim();

	if trimmed.is_empty() {
		return None;
	}

	if let Some((left, right)) = trimmed.rsplit_once(" as ") {
		let base = left.trim();
		let alias = right.trim();

		if base.is_empty() || alias.is_empty() {
			return None;
		}

		return Some((base, Some(alias.to_owned())));
	}

	Some((trimmed, None))
}

pub(super) fn build_use_item_rewrite_edit(
	ctx: &FileContext,
	item: &TopItem,
	rewritten_use_path: Option<&str>,
	rule: &'static str,
) -> Option<Edit> {
	let (item_start, item_end) = item_syntax_text_range(ctx, item)?;

	if let Some(new_use_path) = rewritten_use_path {
		if let Some(raw) = ctx.text.get(item_start..item_end)
			&& let Some(rewritten) = rewrite_use_item_with_path(raw, new_use_path)
		{
			return Some(Edit { start: item_start, end: item_end, replacement: rewritten, rule });
		}

		return None;
	}
	if let Some((line_start, line_end)) =
		full_line_item_delete_range(ctx, item, item_start, item_end)
	{
		return Some(Edit { start: line_start, end: line_end, replacement: String::new(), rule });
	}

	Some(Edit { start: item_start, end: item_end, replacement: String::new(), rule })
}

pub(super) fn use_item_lock_key(item: &TopItem) -> (usize, usize) {
	(item.start_offset, item.end_offset)
}

pub(super) fn item_syntax_text_range(ctx: &FileContext, item: &TopItem) -> Option<(usize, usize)> {
	if item.start_offset > item.end_offset || item.end_offset > ctx.text.len() {
		return None;
	}

	Some((item.start_offset, item.end_offset))
}

pub(super) fn is_rust_primitive_type_ident(segment: &str) -> bool {
	matches!(
		syntax::normalize_ident(segment),
		"bool"
			| "char"
			| "f32"
			| "f64"
			| "i8"
			| "i16"
			| "i32"
			| "i64"
			| "i128"
			| "isize"
			| "str"
			| "u8"
			| "u16"
			| "u32"
			| "u64"
			| "u128"
			| "usize"
	)
}

pub(super) fn qualified_macro_path_regex() -> &'static Regex {
	static QUALIFIED_MACRO_PATH_RE: OnceLock<Regex> = OnceLock::new();

	QUALIFIED_MACRO_PATH_RE.get_or_init(|| {
		Regex::new(
			r"(?:\b(?:crate|self|super|[A-Za-z_][A-Za-z0-9_]*|r#[A-Za-z_][A-Za-z0-9_]*)(?:\s*::\s*(?:[A-Za-z_][A-Za-z0-9_]*|r#[A-Za-z_][A-Za-z0-9_]*)\s*)+)",
		)
		.expect("Compile qualified macro path regex.")
	})
}

pub(super) fn is_inside_cfg_test_module(path: &Path) -> bool {
	syntax_is_inside_cfg_test_module(path.syntax())
}

pub(super) fn path_is_qualifier_subpath(path: &Path) -> bool {
	path.syntax().parent().and_then(Path::cast).is_some()
}

pub(super) fn collect_path_segment_texts(path: &Path, out: &mut Vec<String>) -> bool {
	if let Some(qualifier) = path.qualifier()
		&& !collect_path_segment_texts(&qualifier, out)
	{
		return false;
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

pub(super) fn symbol_from_full_import_path(path: &str) -> Option<String> {
	let symbol = path.rsplit("::").next()?.trim();
	let symbol = syntax::normalize_ident(symbol);

	if matches!(symbol, "" | "*" | "self" | "super" | "crate") {
		None
	} else {
		Some(symbol.to_owned())
	}
}

pub(super) fn imported_full_paths_from_use_path(path: &str) -> Vec<String> {
	imported_reference_paths_from_use_path(path)
		.into_iter()
		.filter(|path| !path.ends_with("::*"))
		.collect()
}

pub(super) fn imported_reference_paths_from_use_path(path: &str) -> Vec<String> {
	let mut paths = Vec::new();

	if !collect_full_paths_from_use_segment(path.trim(), &mut paths) {
		return Vec::new();
	}

	paths.into_iter().map(|path| path.replace(' ', "")).filter(|path| !path.is_empty()).collect()
}

pub(super) fn imported_self_full_paths_from_use_path(path: &str) -> HashSet<String> {
	let mut paths = HashSet::new();

	if !collect_self_full_paths_from_use_segment(path.trim(), &mut paths) {
		return HashSet::new();
	}

	paths.into_iter().map(|path| path.replace(' ', "")).collect()
}

pub(super) fn syntax_is_inside_cfg_test_module(syntax: &SyntaxNode) -> bool {
	syntax.ancestors().filter_map(Module::cast).any(|module| {
		module
			.attrs()
			.any(|attr| attr.syntax().text().to_string().replace(' ', "").contains("cfg(test)"))
	})
}

pub(super) fn syntax_is_inside_cfg_guarded_scope(syntax: &SyntaxNode) -> bool {
	syntax.ancestors().skip(1).any(|ancestor| syntax_has_cfg_attributes(&ancestor))
}

pub(super) fn syntax_has_cfg_attributes(syntax: &SyntaxNode) -> bool {
	syntax.children().filter_map(Attr::cast).any(|attr| {
		let compact = attr
			.syntax()
			.text()
			.to_string()
			.chars()
			.filter(|ch| !ch.is_whitespace())
			.collect::<String>();

		compact.starts_with("#[cfg(") || compact.starts_with("#[cfg_attr(")
	})
}

pub(super) fn compact_path_for_match(path: &str) -> String {
	path.chars().filter(|ch| !ch.is_whitespace()).collect()
}

pub(super) fn normalize_use_path_for_equivalence(path: &str) -> String {
	let mut normalized = compact_path_for_match(path);

	// Treat trailing commas in braced `use` groups as formatting-only.
	while normalized.contains(",}") {
		normalized = normalized.replace(",}", "}");
	}

	normalized
}

pub(super) fn is_cfg_test_attrs(attrs: &[String]) -> bool {
	attrs.iter().any(|attr| attr.replace(' ', "").contains("#[cfg(test)]"))
}

pub(super) fn extract_use_path_from_text(text: &str) -> Option<String> {
	syntax::find_use_path_range(text)
		.and_then(|(start, end)| text.get(start..end).map(|s| s.trim().to_owned()))
}

pub(super) fn extract_use_path(ctx: &FileContext, item: &TopItem) -> Option<String> {
	item.use_path.clone().or_else(|| extract_use_path_from_text(&item.raw)).or_else(|| {
		ctx.lines.get(item.line.saturating_sub(1)).and_then(|line| extract_use_path_from_text(line))
	})
}

pub(super) fn imported_symbols_from_use_path(path: &str) -> Vec<String> {
	let compact = path.replace(' ', "");

	if compact.ends_with("::*") {
		return Vec::new();
	}

	fn normalize_symbol(segment: &str) -> Option<String> {
		let mut symbol = segment.trim().to_owned();

		if symbol.is_empty() {
			return None;
		}

		if let Some((_left, right)) = symbol.split_once(" as ") {
			if right.trim() == "_" {
				return None;
			}

			symbol = right.trim().to_owned();
		}

		if matches!(symbol.as_str(), "*" | "self" | "super" | "crate") {
			return None;
		}

		if let Some(parent) = symbol.strip_suffix("::self") {
			symbol = parent.to_owned();
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

pub(super) fn is_non_importable_root(root: &str) -> bool {
	if matches!(root, "Self" | "self" | "super" | "crate") {
		return root == "Self";
	}

	root.chars().next().is_some_and(char::is_uppercase)
}

pub(super) fn is_non_importable_use_root(root: &str) -> bool {
	root == "Self" || (root.len() == 1 && root.chars().next().is_some_and(char::is_uppercase))
}

pub(super) fn simple_import_prefix_symbol(path: &str) -> Option<(String, String)> {
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

pub(super) fn braced_import_fix_plan(path: &str, symbol: &str) -> Option<(String, Option<String>)> {
	let (open, close) = top_level_brace_range(path)?;

	if !path[close + 1..].trim().is_empty() {
		return None;
	}

	let prefix = braced_import_prefix(path, open)?;
	let inside = &path[open + 1..close];
	let parsed_segments = parse_braced_import_segments(inside)?;
	let (qualified_symbol_path, mut kept) =
		extract_braced_import_target_and_kept(&prefix, &parsed_segments, symbol)?;
	let module_path = prefix.trim_end_matches("::");
	let use_module_alias = module_alias_from_parent_path(module_path);
	let module_leaf = module_path.rsplit("::").next().unwrap_or_default();
	let mut qualified_symbol_path = apply_parent_alias_to_qualified_path(
		module_path,
		use_module_alias.as_deref(),
		qualified_symbol_path,
	);

	if syntax::is_same_ident(module_leaf, symbol) {
		let module_access = use_module_alias.as_deref().unwrap_or(module_leaf);

		if !kept.iter().any(|segment| segment == "self") {
			kept.insert(0, "self".to_owned());
		}

		qualified_symbol_path = format!("{module_access}::{symbol}");
	}

	build_braced_import_rewritten_path(
		&prefix,
		module_path,
		use_module_alias.as_deref(),
		qualified_symbol_path,
		kept.clone(),
		false,
	)
}

pub(super) fn parse_braced_path_parts_allow_alias(
	path: &str,
) -> Option<(String, usize, Vec<String>)> {
	let (open, close) = top_level_brace_range(path)?;

	if !path[close + 1..].trim().is_empty() {
		return None;
	}

	let prefix = path[..open + 1].to_owned();
	let inner = &path[open + 1..close];
	let segments = split_top_level_csv(inner);

	if segments.is_empty() {
		return None;
	}

	Some((prefix, close, segments))
}

pub(super) fn import004_fix_plan(path: &str, symbol: &str) -> Option<(String, Option<String>)> {
	if let Some((prefix, imported_symbol)) = simple_import_prefix_symbol(path)
		&& syntax::is_same_ident(&imported_symbol, symbol)
	{
		if let Some(alias) = module_alias_from_parent_path(&prefix) {
			return Some((format!("{alias}::{imported_symbol}"), Some(prefix)));
		}

		return Some((format!("{prefix}::{imported_symbol}"), None));
	}

	braced_import_fix_plan(path, symbol)
}

pub(super) fn split_top_level_csv(text: &str) -> Vec<String> {
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

pub(super) fn rewrite_use_item_with_path(raw: &str, new_path: &str) -> Option<String> {
	let (start, end) = syntax::find_use_path_range(raw)?;
	let original_path = raw.get(start..end)?;

	// Do not rewrite `use` paths when the change is formatting-only. This avoids
	// collapsing already-correct multi-line imports into a single line.
	if normalize_use_path_for_equivalence(original_path)
		== normalize_use_path_for_equivalence(new_path)
	{
		return None;
	}

	let mut out = String::new();

	out.push_str(&raw[..start]);
	out.push_str(new_path);
	out.push_str(&raw[end..]);

	Some(out)
}

pub(super) fn module_alias_from_parent_path(path: &str) -> Option<String> {
	if !path.starts_with("super::") {
		return None;
	}

	path.rsplit("::").next().filter(|segment| !segment.is_empty()).map(str::to_owned)
}

pub(super) fn use_tree_root_name_ref(tree: &UseTree) -> Option<NameRef> {
	if tree
		.syntax()
		.ancestors()
		.skip(1)
		.filter_map(UseTree::cast)
		.any(|parent| parent.path().is_some())
	{
		return None;
	}

	let mut path = tree.path()?;

	while let Some(qualifier) = path.qualifier() {
		path = qualifier;
	}

	if path.syntax().text().to_string().starts_with("::") {
		return None;
	}

	path.segment()?.name_ref()
}

pub(super) fn parse_single_level_nested_use_segment(segment: &str) -> Option<(&str, &str)> {
	let (head, rest) = segment.split_once("::{")?;

	if !rest.ends_with('}') {
		return None;
	}

	let inner = &rest[..rest.len().saturating_sub(1)];

	if inner.contains('{') || inner.contains('}') {
		return None;
	}

	Some((head.trim(), inner))
}

fn full_line_item_delete_range(
	ctx: &FileContext,
	item: &TopItem,
	item_start: usize,
	item_end: usize,
) -> Option<(usize, usize)> {
	let line_start = shared::offset_from_line(&ctx.line_starts, item.start_line)?;
	let line_end =
		shared::offset_from_line(&ctx.line_starts, item.end_line + 1).unwrap_or(ctx.text.len());
	let prefix = ctx.text.get(line_start..item_start)?;
	let suffix = ctx.text.get(item_end..line_end)?;

	if prefix.trim().is_empty() && suffix.trim().is_empty() {
		Some((line_start, line_end))
	} else {
		None
	}
}

fn collect_self_full_paths_from_use_segment(segment: &str, out: &mut HashSet<String>) -> bool {
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
					out.insert(prefix.to_owned());
				}

				continue;
			}

			let expanded =
				if prefix.is_empty() { child.to_owned() } else { format!("{prefix}::{child}") };

			if !collect_self_full_paths_from_use_segment(&expanded, out) {
				return false;
			}
		}
	}

	true
}

fn collect_full_paths_from_use_segment(segment: &str, out: &mut Vec<String>) -> bool {
	let trimmed = segment.trim();

	if trimmed.is_empty() {
		return true;
	}
	if trimmed.ends_with("::*") {
		out.push(trimmed.to_owned());

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

fn top_level_brace_range(text: &str) -> Option<(usize, usize)> {
	let open = text.find('{')?;
	let mut depth = 0_i32;
	let mut close = None;

	for (idx, ch) in text.char_indices().skip_while(|(idx, _)| *idx < open) {
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

	if depth != 0 {
		return None;
	}

	Some((open, close?))
}

fn braced_import_prefix(path: &str, open: usize) -> Option<String> {
	let mut prefix = path[..open].trim().to_owned();

	if prefix.is_empty() {
		return None;
	}
	if !prefix.ends_with("::") {
		prefix.push_str("::");
	}

	Some(prefix)
}

fn parse_braced_import_segments(inside: &str) -> Option<Vec<BracedImportSegment>> {
	let segments = split_top_level_csv(inside);

	if segments.is_empty() {
		return None;
	}

	let mut out = Vec::new();

	for segment in segments {
		let trimmed = segment.trim();

		if trimmed.is_empty() {
			continue;
		}
		if trimmed == "*" {
			return None;
		}
		if trimmed.contains(" as ") {
			out.push(BracedImportSegment::Simple(trimmed.to_owned()));

			continue;
		}

		if let Some((head, rest)) = trimmed.split_once("::{") {
			let nested = parse_nested_braced_segment(head, rest)?;

			out.push(nested);

			continue;
		}

		if trimmed.contains('{') || trimmed.contains('}') {
			return None;
		}

		out.push(BracedImportSegment::Simple(trimmed.to_owned()));
	}

	if out.is_empty() { None } else { Some(out) }
}

fn parse_nested_braced_segment(head: &str, rest: &str) -> Option<BracedImportSegment> {
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

		child.is_empty() || child == "*" || child.contains('{') || child.contains('}')
	}) {
		return None;
	}

	Some(BracedImportSegment::Nested {
		head: head.trim().to_owned(),
		children: nested_children.into_iter().map(|child| child.trim().to_owned()).collect(),
	})
}

fn extract_braced_import_target_and_kept(
	prefix: &str,
	segments: &[BracedImportSegment],
	symbol: &str,
) -> Option<(String, Vec<String>)> {
	let mut qualified_symbol_path = None::<String>;
	let mut kept = Vec::new();

	for segment in segments {
		match segment {
			BracedImportSegment::Simple(name) => {
				if qualified_symbol_path.is_none()
					&& braced_import_segment_matches_symbol(name, symbol)
				{
					qualified_symbol_path = Some(format!("{prefix}{name}"));
				} else {
					kept.push(name.clone());
				}
			},
			BracedImportSegment::Nested { head, children } => {
				let mut child_kept = Vec::new();

				for child in children {
					if qualified_symbol_path.is_none()
						&& braced_import_segment_matches_symbol(child, symbol)
					{
						qualified_symbol_path = Some(format!("{prefix}{head}::{child}"));
					} else {
						child_kept.push(child.clone());
					}
				}

				if !child_kept.is_empty() {
					kept.push(format!("{head}::{{{}}}", child_kept.join(", ")));
				}
			},
		}
	}

	Some((qualified_symbol_path?, kept))
}

fn braced_import_segment_matches_symbol(segment: &str, symbol: &str) -> bool {
	if syntax::is_same_ident(segment, symbol) {
		return true;
	}

	symbol_from_full_import_path(segment)
		.as_deref()
		.is_some_and(|leaf| syntax::is_same_ident(leaf, symbol))
}

fn apply_parent_alias_to_qualified_path(
	module_path: &str,
	use_module_alias: Option<&str>,
	qualified_symbol_path: String,
) -> String {
	let Some(alias) = use_module_alias else {
		return qualified_symbol_path;
	};

	if let Some(tail) =
		qualified_symbol_path.strip_prefix(module_path).and_then(|rest| rest.strip_prefix("::"))
	{
		format!("{alias}::{tail}")
	} else {
		qualified_symbol_path
	}
}

fn build_braced_import_rewritten_path(
	prefix: &str,
	module_path: &str,
	use_module_alias: Option<&str>,
	qualified_symbol_path: String,
	kept: Vec<String>,
	keep_module_access: bool,
) -> Option<(String, Option<String>)> {
	if kept.is_empty() {
		if use_module_alias.is_some() || keep_module_access {
			return Some((qualified_symbol_path, Some(module_path.to_owned())));
		}

		return Some((qualified_symbol_path, None));
	}

	let rewritten_use_path = if use_module_alias.is_some() || keep_module_access {
		let mut kept_no_self = kept;

		kept_no_self.retain(|segment| segment != "self");

		if kept_no_self.is_empty() {
			format!("{prefix}{{self}}")
		} else {
			format!("{prefix}{{self, {}}}", kept_no_self.join(", "))
		}
	} else {
		format!("{prefix}{{{}}}", kept.join(", "))
	};

	Some((qualified_symbol_path, Some(rewritten_use_path)))
}
