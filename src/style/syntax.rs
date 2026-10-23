use ra_ap_syntax::{
	AstNode, Edition, SourceFile, SyntaxKind,
	ast::{Path, PathSegment},
};

pub(crate) fn normalize_ident(name: &str) -> &str {
	name.strip_prefix("r#").unwrap_or(name)
}

pub(crate) fn is_same_ident(left: &str, right: &str) -> bool {
	normalize_ident(left) == normalize_ident(right)
}

pub(crate) fn find_use_path_range(text: &str) -> Option<(usize, usize)> {
	let source = SourceFile::parse(text, Edition::CURRENT).tree();
	let mut tokens = source.syntax().descendants_with_tokens().filter_map(|part| part.into_token());

	tokens.find(|token| token.kind() == SyntaxKind::USE_KW)?;

	let first = tokens.by_ref().find(|token| !token.kind().is_trivia())?;
	let end = tokens.find(|token| token.kind() == SyntaxKind::SEMICOLON)?;

	Some((usize::from(first.text_range().start()), usize::from(end.text_range().start())))
}

pub(crate) fn collect_simple_path_segments(path: &Path, out: &mut Vec<PathSegment>) -> bool {
	if let Some(qualifier) = path.qualifier()
		&& !collect_simple_path_segments(&qualifier, out)
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

	out.push(segment);

	true
}
