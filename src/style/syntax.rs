use ra_ap_syntax::{
	AstNode, Edition, SourceFile, SyntaxKind, SyntaxNode,
	ast::{ConstParam, HasName, Path, PathSegment, TypeParam, Use, UseTree},
};

pub(crate) struct UseBinding {
	pub(crate) start: usize,
	pub(crate) path: Vec<String>,
	pub(crate) name: Option<String>,
	pub(crate) alias: Option<String>,
	pub(crate) is_glob: bool,
	pub(crate) imports_self: bool,
}

pub(crate) fn generic_parameter_names(node: &SyntaxNode) -> impl Iterator<Item = String> {
	node.descendants().filter_map(|node| {
		TypeParam::cast(node.clone())
			.and_then(|param| param.name())
			.or_else(|| ConstParam::cast(node).and_then(|param| param.name()))
			.map(|name| name.text().to_string())
	})
}

pub(crate) fn parse_use_bindings(path: &str) -> Option<Vec<UseBinding>> {
	let parsed = SourceFile::parse(&format!("use {path};"), Edition::CURRENT);

	if !parsed.errors().is_empty() {
		return None;
	}

	use_bindings(&parsed.tree().syntax().children().find_map(Use::cast)?)
}

pub(crate) fn use_bindings(item: &Use) -> Option<Vec<UseBinding>> {
	let mut bindings = Vec::new();

	collect_use_bindings(&item.use_tree()?, &[], &mut bindings)?;

	Some(bindings)
}

pub(crate) fn path_segments(path: &Path) -> Option<Vec<String>> {
	let mut nodes = Vec::new();

	if !collect_simple_path_segments(path, &mut nodes) {
		return None;
	}

	nodes
		.into_iter()
		.map(|segment| {
			if let Some(name) = segment.name_ref() {
				return Some(name.text().to_string());
			}

			let token = segment.syntax().first_token()?;

			matches!(token.text(), "crate" | "self" | "super" | "Self")
				.then(|| token.text().to_owned())
		})
		.collect()
}

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

fn collect_use_bindings(
	tree: &UseTree,
	prefix: &[String],
	out: &mut Vec<UseBinding>,
) -> Option<()> {
	let own_path = tree.path();
	let segments = if let Some(path) = &own_path { Some(path_segments(path)?) } else { None };
	let imports_self = !prefix.is_empty() && segments.as_deref() == Some(&["self".to_owned()]);
	let mut full =
		if own_path.as_ref().is_some_and(|path| path.syntax().text().to_string().starts_with("::"))
		{
			vec![String::new()]
		} else {
			prefix.to_vec()
		};

	if !imports_self {
		full.extend(segments.unwrap_or_default());
	}

	if let Some(children) = tree.use_tree_list() {
		for child in children.syntax().children().filter_map(UseTree::cast) {
			collect_use_bindings(&child, &full, out)?;
		}

		return Some(());
	}

	let is_glob = tree.star_token().is_some();
	let alias = tree.rename().and_then(|rename| {
		rename
			.name()
			.map(|name| name.text().to_string())
			.or_else(|| rename.underscore_token().map(|_| "_".to_owned()))
	});
	let name = if is_glob || alias.as_deref() == Some("_") {
		None
	} else if let Some(alias) = &alias {
		Some(alias.clone())
	} else {
		full.last().cloned()
	};

	out.push(UseBinding {
		start: usize::from(tree.syntax().text_range().start()),
		path: full,
		name,
		alias,
		is_glob,
		imports_self,
	});

	Some(())
}
