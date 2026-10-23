use std::collections::HashSet;

use ra_ap_syntax::{
	AstNode as _, SyntaxKind, TextSize,
	ast::{
		Attr, CallExpr, HasName as _, IdentPat, MacroCall, Meta, Module, NameRef, Path, PathExpr,
		PathPat, PathType, RecordExpr, RecordPat, StmtList, TupleStructPat, Use, UseTree,
	},
};
use regex::Regex;

use crate::style::{
	imports::{PathQualificationRequirement, TypePathCandidate, ValuePathCandidate, paths},
	shared::FileContext,
	syntax,
};

pub(super) fn dependent_use_root_rewrites(
	ctx: &FileContext,
	symbol: &str,
	qualified_path: &str,
) -> Vec<(usize, usize, String)> {
	let mut rewrites = Vec::new();

	for tree in ctx.source_file.syntax().descendants().filter_map(UseTree::cast) {
		if tree.syntax().ancestors().any(|node| Module::can_cast(node.kind())) {
			continue;
		}

		let Some(name) = paths::use_tree_root_name_ref(&tree) else {
			continue;
		};
		let range = name.syntax().text_range();
		let start = usize::from(range.start());

		if syntax::is_same_ident(name.text(), symbol)
			&& !alias_is_shadowed_by_local_import(ctx, symbol, start)
		{
			rewrites.push((start, usize::from(range.end()), qualified_path.to_owned()));
		}
	}

	rewrites
}

pub(super) fn symbol_is_referenced_outside_use(ctx: &FileContext, symbol: &str) -> bool {
	for path in ctx.source_file.syntax().descendants().filter_map(Path::cast) {
		if path.syntax().ancestors().any(|node| Use::cast(node).is_some()) {
			continue;
		}

		let Some(segment) = path.segment() else {
			continue;
		};
		let Some(name_ref) = segment.name_ref() else {
			continue;
		};

		if syntax::is_same_ident(name_ref.text(), symbol) {
			return true;
		}
	}

	let derive_symbol_re = Regex::new(&format!(r"\b{}\b", regex::escape(symbol)))
		.expect("Expected operation to succeed.");

	for attr in ctx.source_file.syntax().descendants().filter_map(Attr::cast) {
		let text = attr.syntax().text().to_string();

		if !text.contains("derive") {
			continue;
		}
		if derive_symbol_re.is_match(&text) {
			return true;
		}
	}

	!alias_macro_path_ranges(ctx, symbol).is_empty()
		|| symbol_has_bare_identifier_pattern(ctx, symbol)
		|| !dependent_use_root_rewrites(ctx, symbol, symbol).is_empty()
}

pub(super) fn alias_is_shadowed_by_local_import(
	ctx: &FileContext,
	alias: &str,
	offset: usize,
) -> bool {
	let Some(token) = TextSize::try_from(offset)
		.ok()
		.and_then(|offset| ctx.source_file.syntax().token_at_offset(offset).right_biased())
	else {
		return false;
	};
	let containing_use = token.parent_ancestors().find_map(Use::cast);

	token
		.parent_ancestors()
		.take_while(|ancestor| !Module::can_cast(ancestor.kind()))
		.filter_map(StmtList::cast)
		.any(|scope| {
			scope.syntax().children().filter_map(Use::cast).any(|item| {
				if containing_use.as_ref() == Some(&item) {
					return false;
				}

				item.use_tree().is_some_and(|tree| {
					paths::imported_symbols_from_use_path(&tree.syntax().text().to_string())
						.iter()
						.any(|symbol| syntax::is_same_ident(symbol, alias))
				})
			})
		})
}

pub(super) fn symbol_has_bare_identifier_pattern(ctx: &FileContext, symbol: &str) -> bool {
	// A bare pattern can resolve to an imported constant instead of declaring a binding.
	// Keep its import until name resolution can distinguish those cases.
	ctx.source_file.syntax().descendants().filter_map(IdentPat::cast).any(|pattern| {
		pattern.name().is_some_and(|name| syntax::is_same_ident(name.text(), symbol))
	})
}

pub(super) fn qualified_associated_receiver_symbol_path(
	segments: &[String],
) -> Option<(String, String)> {
	if segments.len() < 3 {
		return None;
	}

	let receiver_path = segments[..segments.len() - 1].join("::");
	let receiver_symbol = paths::symbol_from_full_import_path(&receiver_path)?;

	if !is_import009_type_like_symbol(&receiver_symbol) {
		return None;
	}

	Some((receiver_symbol, receiver_path))
}

pub(super) fn is_import009_type_like_symbol(symbol: &str) -> bool {
	symbol.chars().next().is_some_and(|first| first.is_ascii_uppercase())
}

pub(super) fn classify_value_path_candidate(
	ctx: &FileContext,
	path: Path,
	qualification: PathQualificationRequirement,
	allow_macro_context: bool,
	skip_qualifier_subpath: bool,
) -> Option<ValuePathCandidate> {
	if skip_qualifier_subpath && paths::path_is_qualifier_subpath(&path) {
		return None;
	}

	match qualification {
		PathQualificationRequirement::Qualified if path.qualifier().is_none() => return None,
		PathQualificationRequirement::Unqualified if path.qualifier().is_some() => return None,
		_ => {},
	}

	if paths::is_inside_cfg_test_module(&path) {
		return None;
	}
	if path.syntax().ancestors().any(|node| Use::cast(node).is_some()) {
		return None;
	}
	if path.syntax().ancestors().any(|node| PathType::cast(node).is_some()) {
		return None;
	}

	let macro_context = path.syntax().ancestors().any(|node| MacroCall::cast(node).is_some());

	if macro_context {
		if !allow_macro_context {
			return None;
		}
	} else if !is_value_path_usage(&path) {
		return None;
	}

	let segment = path.segment()?;
	let name_ref = segment.name_ref()?;

	if name_ref_is_method_segment(ctx, &name_ref) {
		return None;
	}

	let is_record_context = path
		.syntax()
		.ancestors()
		.any(|node| RecordExpr::cast(node.clone()).is_some() || RecordPat::cast(node).is_some());

	Some(ValuePathCandidate { path, name_ref, is_record_context })
}

pub(super) fn classify_type_path_candidate(
	ctx: &FileContext,
	path: Path,
	qualification: PathQualificationRequirement,
) -> Option<TypePathCandidate> {
	match qualification {
		PathQualificationRequirement::Qualified if path.qualifier().is_none() => return None,
		PathQualificationRequirement::Unqualified if path.qualifier().is_some() => return None,
		_ => {},
	}

	if paths::is_inside_cfg_test_module(&path) {
		return None;
	}

	let segment = path.segment()?;
	let name_ref = segment.name_ref()?;

	if name_ref_is_method_segment(ctx, &name_ref) {
		return None;
	}

	let segment_text = segment.syntax().text().to_string();
	let name_text = name_ref.text().to_string();
	let suffix = segment_text.strip_prefix(&name_text)?.to_owned();

	Some(TypePathCandidate { path, name_ref, suffix })
}

pub(super) fn unqualified_type_path_rewrites(
	ctx: &FileContext,
	symbol: &str,
	qualified_path: &str,
) -> Vec<(usize, usize, String)> {
	let mut rewrites = Vec::new();

	for path_type in ctx.source_file.syntax().descendants().filter_map(PathType::cast) {
		let Some(candidate) = path_type.path().and_then(|path| {
			classify_type_path_candidate(ctx, path, PathQualificationRequirement::Unqualified)
		}) else {
			continue;
		};

		if !syntax::is_same_ident(candidate.name_ref.text(), symbol) {
			continue;
		}

		rewrites.push((
			usize::from(candidate.name_ref.syntax().text_range().start()),
			usize::from(candidate.name_ref.syntax().text_range().end()),
			qualified_path.to_owned(),
		));
	}

	rewrites
}

pub(super) fn alias_root_type_path_rewrites(
	ctx: &FileContext,
	alias: &str,
	qualified_path: &str,
) -> Vec<(usize, usize, String)> {
	let mut rewrites = Vec::new();

	for path_type in ctx.source_file.syntax().descendants().filter_map(PathType::cast) {
		let Some(candidate) = path_type.path().and_then(|path| {
			classify_type_path_candidate(ctx, path, PathQualificationRequirement::Qualified)
		}) else {
			continue;
		};
		let Some(replacement) = rewrite_alias_root_path(
			candidate.path.syntax().text().to_string().as_str(),
			alias,
			qualified_path,
		) else {
			continue;
		};

		rewrites.push((
			usize::from(candidate.path.syntax().text_range().start()),
			usize::from(candidate.path.syntax().text_range().end()),
			replacement,
		));
	}

	rewrites
}

pub(super) fn alias_root_path_name_ref_rewrites(
	ctx: &FileContext,
	alias: &str,
	qualified_path: &str,
) -> Vec<(usize, usize, String)> {
	let mut rewrites = Vec::new();
	let mut seen = HashSet::new();

	for path in ctx.source_file.syntax().descendants().filter_map(Path::cast) {
		let Some(candidate) = classify_value_path_candidate(
			ctx,
			path,
			PathQualificationRequirement::Unqualified,
			false,
			false,
		) else {
			continue;
		};

		if !syntax::is_same_ident(candidate.name_ref.text(), alias) {
			continue;
		}

		let range = candidate.name_ref.syntax().text_range();
		let start = usize::from(range.start());
		let end = usize::from(range.end());

		if !seen.insert((start, end)) {
			continue;
		}

		rewrites.push((start, end, qualified_path.to_owned()));
	}

	rewrites
}

pub(super) fn alias_macro_token_tree_rewrites(
	ctx: &FileContext,
	alias: &str,
	qualified_path: &str,
) -> Vec<(usize, usize, String)> {
	alias_macro_path_ranges(ctx, alias)
		.into_iter()
		.map(|(start, end)| (start, end, qualified_path.to_owned()))
		.collect()
}

pub(super) fn alias_macro_path_ranges(ctx: &FileContext, alias: &str) -> Vec<(usize, usize)> {
	let pattern = format!(r"\b{}\b::", regex::escape(alias));
	let re = Regex::new(&pattern).expect("Compile alias macro path regex.");
	let mut rewrites = Vec::new();

	for macro_call in ctx.source_file.syntax().descendants().filter_map(MacroCall::cast) {
		let Some(token_tree) = macro_call.token_tree() else {
			continue;
		};
		let original = token_tree.syntax().text().to_string();
		let tree_start = usize::from(token_tree.syntax().text_range().start());

		for found in re.find_iter(&original) {
			if !macro_symbol_is_import009_unqualified(ctx, &original, tree_start, found.start()) {
				continue;
			}

			let start = tree_start + found.start();

			rewrites.push((start, start + alias.len()));
		}
	}

	rewrites
}

pub(super) fn unqualified_value_path_rewrites(
	ctx: &FileContext,
	symbol: &str,
	qualified_path: &str,
) -> Vec<(usize, usize, String)> {
	let mut rewrites = Vec::new();
	let mut seen_ranges = HashSet::new();

	for path in ctx.source_file.syntax().descendants().filter_map(Path::cast) {
		let Some(candidate) = classify_value_path_candidate(
			ctx,
			path,
			PathQualificationRequirement::Unqualified,
			false,
			false,
		) else {
			continue;
		};

		if !syntax::is_same_ident(candidate.name_ref.text(), symbol) {
			continue;
		}

		let start = usize::from(candidate.name_ref.syntax().text_range().start());
		let end = usize::from(candidate.name_ref.syntax().text_range().end());

		if seen_ranges.insert((start, end)) {
			rewrites.push((start, end, qualified_path.to_owned()));
		}
	}
	for (start, end, replacement) in
		unqualified_macro_token_symbol_rewrites(ctx, symbol, qualified_path)
	{
		if seen_ranges.insert((start, end)) {
			rewrites.push((start, end, replacement));
		}
	}

	rewrites
}

pub(super) fn macro_symbol_has_record_follow(text: &str, symbol_end: usize) -> bool {
	let bytes = text.as_bytes();
	let mut idx = symbol_end;

	while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
		idx += 1;
	}

	idx < bytes.len() && bytes[idx] == b'{'
}

pub(super) fn macro_symbol_is_import009_unqualified(
	ctx: &FileContext,
	text: &str,
	tree_start: usize,
	symbol_start: usize,
) -> bool {
	let Some(offset) =
		tree_start.checked_add(symbol_start).and_then(|offset| TextSize::try_from(offset).ok())
	else {
		return false;
	};
	let Some(token) = ctx.source_file.syntax().token_at_offset(offset).right_biased() else {
		return false;
	};

	if token.text_range().start() != offset
		|| !matches!(
			token.kind(),
			SyntaxKind::IDENT | SyntaxKind::CRATE_KW | SyntaxKind::SELF_KW | SyntaxKind::SUPER_KW
		) {
		return false;
	}
	if symbol_start == 0 {
		return true;
	}

	let bytes = text.as_bytes();
	let mut idx = symbol_start;

	while idx > 0 {
		idx -= 1;

		if bytes[idx].is_ascii_whitespace() {
			continue;
		}

		return !(bytes[idx] == b':' && idx > 0 && bytes[idx - 1] == b':')
			&& bytes[idx] != b'.'
			&& !bytes[idx].is_ascii_alphanumeric()
			&& bytes[idx] != b'_';
	}

	true
}

pub(super) fn alias_root_value_path_rewrites(
	ctx: &FileContext,
	alias: &str,
	qualified_path: &str,
) -> Vec<(usize, usize, String)> {
	let mut rewrites = Vec::new();

	for path in ctx.source_file.syntax().descendants().filter_map(Path::cast) {
		let Some(candidate) = classify_value_path_candidate(
			ctx,
			path,
			PathQualificationRequirement::Qualified,
			true,
			true,
		) else {
			continue;
		};
		let Some(replacement) = rewrite_alias_root_path(
			candidate.path.syntax().text().to_string().as_str(),
			alias,
			qualified_path,
		) else {
			continue;
		};

		rewrites.push((
			usize::from(candidate.path.syntax().text_range().start()),
			usize::from(candidate.path.syntax().text_range().end()),
			replacement,
		));
	}

	rewrites
}

pub(super) fn unqualified_derive_attr_symbol_rewrites(
	ctx: &FileContext,
	symbol: &str,
	qualified_path: &str,
) -> Vec<(usize, usize, String)> {
	let symbol_re = Regex::new(&format!(r"\b{}\b", regex::escape(symbol)))
		.expect("Compile import009 derive symbol regex.");
	let mut rewrites = Vec::new();

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
		let tree_start = usize::from(token_tree.syntax().text_range().start());

		for found in symbol_re.find_iter(&tree_text) {
			let symbol_start = found.start();
			let symbol_end = found.end();

			if !derive_symbol_is_unqualified(&tree_text, symbol_start)
				|| !derive_symbol_has_import009_follow(&tree_text, symbol_end)
			{
				continue;
			}

			rewrites.push((
				tree_start + symbol_start,
				tree_start + symbol_end,
				qualified_path.to_owned(),
			));
		}
	}

	rewrites
}

pub(super) fn unqualified_nongeneric_type_path_rewrites(
	ctx: &FileContext,
	symbol: &str,
	qualified_path: &str,
) -> Vec<(usize, usize, String)> {
	let mut rewrites = Vec::new();

	for path_type in ctx.source_file.syntax().descendants().filter_map(PathType::cast) {
		let Some(candidate) = path_type.path().and_then(|path| {
			classify_type_path_candidate(ctx, path, PathQualificationRequirement::Unqualified)
		}) else {
			continue;
		};

		if !syntax::is_same_ident(candidate.name_ref.text(), symbol) {
			continue;
		}
		if !candidate.suffix.is_empty() {
			continue;
		}

		rewrites.push((
			usize::from(candidate.path.syntax().text_range().start()),
			usize::from(candidate.path.syntax().text_range().end()),
			qualified_path.to_owned(),
		));
	}

	rewrites
}

pub(super) fn has_unqualified_generic_type_path_use(ctx: &FileContext, symbol: &str) -> bool {
	for path_type in ctx.source_file.syntax().descendants().filter_map(PathType::cast) {
		let Some(candidate) = path_type.path().and_then(|path| {
			classify_type_path_candidate(ctx, path, PathQualificationRequirement::Unqualified)
		}) else {
			continue;
		};

		if !syntax::is_same_ident(candidate.name_ref.text(), symbol) {
			continue;
		}
		if !candidate.suffix.is_empty() {
			return true;
		}
	}

	false
}

pub(super) fn unqualified_value_paths_are_record_only(ctx: &FileContext, symbol: &str) -> bool {
	let mut saw_matching_path = false;

	for path in ctx.source_file.syntax().descendants().filter_map(Path::cast) {
		let Some(candidate) = classify_value_path_candidate(
			ctx,
			path,
			PathQualificationRequirement::Unqualified,
			false,
			false,
		) else {
			continue;
		};

		if !syntax::is_same_ident(candidate.name_ref.text(), symbol) {
			continue;
		}

		saw_matching_path = true;

		if !candidate.is_record_context {
			return false;
		}
	}

	saw_matching_path
}

pub(super) fn unqualified_function_call_ranges(
	ctx: &FileContext,
	symbol: &str,
) -> Vec<(usize, usize)> {
	let mut ranges = Vec::new();
	let mut seen_ranges = HashSet::new();

	for call_expr in ctx.source_file.syntax().descendants().filter_map(CallExpr::cast) {
		let Some(expr) = call_expr.expr() else {
			continue;
		};
		let Some(path_expr) = PathExpr::cast(expr.syntax().clone()) else {
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

		if !syntax::is_same_ident(name_ref.text(), symbol) {
			continue;
		}

		let range = (
			usize::from(name_ref.syntax().text_range().start()),
			usize::from(name_ref.syntax().text_range().end()),
		);

		if seen_ranges.insert(range) {
			ranges.push(range);
		}
	}
	for range in unqualified_macro_token_function_call_ranges(ctx, symbol) {
		if seen_ranges.insert(range) {
			ranges.push(range);
		}
	}

	ranges
}

pub(super) fn macro_symbol_has_import004_function_follow(text: &str, symbol_end: usize) -> bool {
	let bytes = text.as_bytes();
	let mut idx = symbol_end;

	while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
		idx += 1;
	}

	if idx >= bytes.len() {
		return false;
	}
	if bytes[idx] == b'(' {
		return true;
	}
	if idx + 1 >= bytes.len() || bytes[idx] != b':' || bytes[idx + 1] != b':' {
		return false;
	}

	idx += 2;

	while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
		idx += 1;
	}

	idx < bytes.len() && bytes[idx] == b'<'
}

pub(super) fn unqualified_macro_call_ranges(
	ctx: &FileContext,
	symbol: &str,
) -> Vec<(usize, usize)> {
	let mut ranges = Vec::new();

	for macro_call in ctx.source_file.syntax().descendants().filter_map(MacroCall::cast) {
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

		if !syntax::is_same_ident(name_ref.text(), symbol) {
			continue;
		}

		ranges.push((
			usize::from(path.syntax().text_range().start()),
			usize::from(path.syntax().text_range().end()),
		));
	}
	for macro_call in ctx.source_file.syntax().descendants().filter_map(MacroCall::cast) {
		let Some(tree) = macro_call.token_tree() else {
			continue;
		};
		let tokens = tree
			.syntax()
			.descendants_with_tokens()
			.filter_map(|part| part.into_token())
			.filter(|token| !token.kind().is_trivia())
			.collect::<Vec<_>>();

		for (index, token) in tokens.iter().enumerate() {
			if token.kind() != SyntaxKind::IDENT
				|| !syntax::is_same_ident(token.text(), symbol)
				|| !tokens.get(index + 1).is_some_and(|next| next.text() == "!")
				|| index.checked_sub(1).and_then(|previous| tokens.get(previous)).is_some_and(
					|previous| match previous.text() {
						"::" | "$" => true,
						":" => index >= 2 && tokens[index - 2].text() == ":",
						_ => false,
					},
				) {
				continue;
			}

			ranges.push((
				usize::from(token.text_range().start()),
				usize::from(token.text_range().end()),
			));
		}
	}

	ranges
}

fn is_value_path_usage(path: &Path) -> bool {
	path.syntax().ancestors().any(|node| {
		PathExpr::cast(node.clone()).is_some()
			|| PathPat::cast(node.clone()).is_some()
			|| TupleStructPat::cast(node.clone()).is_some()
			|| RecordExpr::cast(node.clone()).is_some()
			|| RecordPat::cast(node).is_some()
	})
}

// Method names are not module/value paths and must never be qualified.
fn name_ref_is_method_segment(ctx: &FileContext, name_ref: &NameRef) -> bool {
	let bytes = ctx.text.as_bytes();
	let mut idx = usize::from(name_ref.syntax().text_range().start());

	while idx > 0 && bytes[idx - 1].is_ascii_whitespace() {
		idx -= 1;
	}

	idx > 0 && bytes[idx - 1] == b'.'
}

fn rewrite_alias_root_path(path_text: &str, alias: &str, qualified_path: &str) -> Option<String> {
	let compact = paths::compact_path_for_match(path_text);
	let (head, tail) = compact
		.split_once("::")
		.map_or((compact.as_str(), None), |(left, right)| (left, Some(right)));

	if !syntax::is_same_ident(head, alias) {
		return None;
	}

	if let Some(tail) = tail {
		return Some(format!("{qualified_path}::{tail}"));
	}

	Some(qualified_path.to_owned())
}

fn derive_symbol_is_unqualified(text: &str, symbol_start: usize) -> bool {
	if symbol_start == 0 {
		return true;
	}

	let bytes = text.as_bytes();
	let mut idx = symbol_start;

	while idx > 0 && bytes[idx - 1].is_ascii_whitespace() {
		idx -= 1;
	}

	if idx == 0 {
		return true;
	}

	let prev = bytes[idx - 1];

	if prev.is_ascii_alphanumeric() || prev == b'_' || prev == b':' || prev == b'#' {
		return false;
	}

	true
}

fn derive_symbol_has_import009_follow(text: &str, symbol_end: usize) -> bool {
	let bytes = text.as_bytes();
	let mut idx = symbol_end;

	while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
		idx += 1;
	}

	if idx >= bytes.len() {
		return true;
	}

	matches!(bytes[idx], b',' | b')')
}

fn unqualified_macro_token_symbol_rewrites(
	ctx: &FileContext,
	symbol: &str,
	qualified_path: &str,
) -> Vec<(usize, usize, String)> {
	let symbol_re = Regex::new(&format!(r"\b{}\b", regex::escape(symbol)))
		.expect("Compile import009 macro symbol regex.");
	let mut rewrites = Vec::new();

	for macro_call in ctx.source_file.syntax().descendants().filter_map(MacroCall::cast) {
		if paths::syntax_is_inside_cfg_test_module(macro_call.syntax()) {
			continue;
		}

		let Some(token_tree) = macro_call.token_tree() else {
			continue;
		};
		let tree_text = token_tree.syntax().text().to_string();
		let tree_start = usize::from(token_tree.syntax().text_range().start());

		for found in symbol_re.find_iter(&tree_text) {
			let symbol_start = found.start();
			let symbol_end = found.end();

			if !macro_symbol_has_import009_follow(&tree_text, symbol_end)
				|| !macro_symbol_is_import009_unqualified(ctx, &tree_text, tree_start, symbol_start)
			{
				continue;
			}

			rewrites.push((
				tree_start + symbol_start,
				tree_start + symbol_end,
				qualified_path.to_owned(),
			));
		}
	}

	rewrites
}

fn macro_symbol_has_import009_follow(text: &str, symbol_end: usize) -> bool {
	let bytes = text.as_bytes();
	let mut idx = symbol_end;

	while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
		idx += 1;
	}

	if idx >= bytes.len() {
		return false;
	}
	if bytes[idx] == b'{' {
		return true;
	}

	idx + 1 < bytes.len() && bytes[idx] == b':' && bytes[idx + 1] == b':'
}

fn unqualified_macro_token_function_call_ranges(
	ctx: &FileContext,
	symbol: &str,
) -> Vec<(usize, usize)> {
	let symbol_re = Regex::new(&format!(r"\b{}\b", regex::escape(symbol)))
		.expect("Compile import004 macro function regex.");
	let mut ranges = Vec::new();

	for macro_call in ctx.source_file.syntax().descendants().filter_map(MacroCall::cast) {
		let Some(token_tree) = macro_call.token_tree() else {
			continue;
		};
		let tree_text = token_tree.syntax().text().to_string();
		let tree_start = usize::from(token_tree.syntax().text_range().start());

		for found in symbol_re.find_iter(&tree_text) {
			let symbol_start = found.start();
			let symbol_end = found.end();

			if !macro_symbol_has_import004_function_follow(&tree_text, symbol_end)
				|| !macro_symbol_is_import009_unqualified(ctx, &tree_text, tree_start, symbol_start)
			{
				continue;
			}

			ranges.push((tree_start + symbol_start, tree_start + symbol_end));
		}
	}

	ranges
}
