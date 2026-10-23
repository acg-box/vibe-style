use std::{
	collections::{BTreeMap, BTreeSet},
	path::PathBuf,
};

use color_eyre::Result;
use ra_ap_syntax::{
	AstNode, SyntaxKind, SyntaxNode,
	ast::{self, GenericParam, GenericParamList, HasName, Item, Module, Use, UseTree},
};

use crate::style::{
	fixes, module_tree,
	shared::{self, Edit, FileContext},
	syntax,
	types::{self, RULE_ID, TypeAliasRenameFix},
};

enum NameResolution {
	Found(Vec<String>),
	Unbound,
	Blocked,
}

#[derive(Debug, Default)]
pub(crate) struct TypeAliasRenamePlan {
	edits: BTreeMap<PathBuf, Vec<Edit>>,
}
impl TypeAliasRenamePlan {
	pub(crate) fn collect(files: &[PathBuf]) -> Result<Self> {
		let contexts = files
			.iter()
			.map(|file| shared::read_file_context(file))
			.collect::<Result<Vec<_>>>()?
			.into_iter()
			.flatten()
			.collect::<Vec<_>>();
		let mut candidates = BTreeMap::<String, Vec<(PathBuf, TypeAliasRenameFix)>>::new();

		for ctx in &contexts {
			for fix in types::collect_type_alias_rename_fixes(ctx) {
				candidates.entry(path_key(&fix.origin)).or_default().push((ctx.path.clone(), fix));
			}
		}

		// Multiple conditional definitions need compiler name resolution, not a first match.
		let mut aliases = candidates
			.into_iter()
			.filter_map(|(key, mut fixes)| (fixes.len() == 1).then(|| (key, fixes.remove(0))))
			.collect::<BTreeMap<_, _>>();

		if aliases.is_empty() {
			return Ok(Self::default());
		}

		let mut blocked = BTreeSet::new();

		for ctx in &contexts {
			blocked.extend(collect_usage_edits(ctx, &aliases, &[]).1);
		}

		aliases.retain(|key, _| !blocked.contains(key));

		let accepted = aliases.values().map(|(_, fix)| fix.clone()).collect::<Vec<_>>();
		let mut plan = Self::default();

		for ctx in contexts {
			let mut edits = aliases
				.values()
				.filter(|(file, _)| *file == ctx.path)
				.flat_map(|(_, fix)| fix.definition_edits.iter().cloned())
				.collect::<Vec<_>>();
			let skip_ranges = edits.iter().map(|edit| (edit.start, edit.end)).collect::<Vec<_>>();

			edits.extend(types::build_type_alias_usage_rename_edits(&ctx, &accepted, &skip_ranges));

			if !edits.is_empty() {
				plan.edits.insert(ctx.path, edits);
			}
		}

		Ok(plan)
	}

	pub(crate) fn apply(
		&self,
		path: &std::path::Path,
		source: &str,
	) -> Result<(String, usize, bool)> {
		let Some(edits) = self.edits.get(path) else {
			return Ok((source.to_owned(), 0, false));
		};
		let mut rewritten = source.to_owned();
		let count = fixes::apply_edits(&mut rewritten, edits.clone())?;

		Ok((rewritten, count, count > 0))
	}
}

pub(super) fn module_scope(ctx: &FileContext, node: &SyntaxNode) -> Option<Vec<String>> {
	let file_owner = ctx
		.declared_module_path
		.get_or_init(|| module_tree::declared_file_module_path(&ctx.path))
		.as_ref()?;
	let mut owner = vec!["crate".to_owned()];
	let mut inline = node
		.ancestors()
		.filter_map(Module::cast)
		.filter_map(|module| module.name().map(|name| name.text().to_string()))
		.collect::<Vec<_>>();

	inline.reverse();
	owner.extend(file_owner.iter().cloned());
	owner.extend(inline);

	Some(owner)
}

pub(super) fn resolve_path(ctx: &FileContext, path: &ast::Path) -> Option<Vec<String>> {
	let mut parts = syntax::path_segments(path)?;

	if path.syntax().text().to_string().starts_with("::") {
		parts.insert(0, String::new());
	}

	resolve_parts(ctx, path.syntax(), &parts, &mut BTreeSet::new())
}

pub(crate) fn build_type_alias_usage_rename_edits(
	ctx: &FileContext,
	fixes: &[TypeAliasRenameFix],
	skip_ranges: &[(usize, usize)],
) -> Vec<Edit> {
	let aliases = fixes
		.iter()
		.map(|fix| (path_key(&fix.origin), (ctx.path.clone(), fix.clone())))
		.collect::<BTreeMap<_, _>>();

	collect_usage_edits(ctx, &aliases, skip_ranges).0
}

fn collect_usage_edits(
	ctx: &FileContext,
	aliases: &BTreeMap<String, (PathBuf, TypeAliasRenameFix)>,
	skip_ranges: &[(usize, usize)],
) -> (Vec<Edit>, BTreeSet<String>) {
	let mut edits = Vec::new();
	let mut blocked = BTreeSet::new();

	for path in ctx.source_file.syntax().descendants().filter_map(ast::Path::cast) {
		let Some(parent) = path.syntax().parent() else {
			continue;
		};

		// Bare values and macro names have separate namespaces from type aliases.
		if !matches!(
			parent.kind(),
			SyntaxKind::PATH
				| SyntaxKind::PATH_TYPE
				| SyntaxKind::USE_TREE
				| SyntaxKind::RECORD_EXPR
				| SyntaxKind::RECORD_PAT
				| SyntaxKind::TUPLE_STRUCT_PAT
		) {
			continue;
		}

		let Some((resolved, import)) = reference_path(ctx, &path) else {
			continue;
		};
		let key = path_key(&resolved);
		let Some((_, fix)) = aliases.get(&key) else {
			continue;
		};
		let Some(name) = path.segment().and_then(|segment| segment.name_ref()) else {
			continue;
		};
		let start = usize::from(path.syntax().text_range().start());
		let end = usize::from(name.syntax().text_range().end());

		if skip_ranges.iter().any(|(left, right)| start < *right && *left < end) {
			continue;
		}

		let edit = if let Some(tree) = import {
			if fix.preserve_module {
				Edit {
					start: usize::from(name.syntax().text_range().start()),
					end,
					replacement: fix.target.clone(),
					rule: RULE_ID,
				}
			} else if fix.replacement.len() == 1
				|| tree
					.syntax()
					.ancestors()
					.skip(1)
					.filter_map(UseTree::cast)
					.any(|parent| parent.path().is_some())
			{
				// A braced import cannot change owners by replacing only its leaf.
				blocked.insert(key);

				continue;
			} else {
				Edit { start, end, replacement: fix.replacement.join("::"), rule: RULE_ID }
			}
		} else {
			let mut replacement = fix.replacement.join("::");

			if fix.replacement.len() == 1
				&& types::is_primitive_type_ident(&replacement)
				&& !matches!(
					resolve_name(ctx, path.syntax(), &replacement, &mut BTreeSet::new()),
					NameResolution::Unbound
				) {
				replacement = format!("::core::primitive::{replacement}");
			}

			Edit { start, end, replacement, rule: RULE_ID }
		};

		edits.push(edit);
	}

	(edits, blocked)
}

fn reference_path(ctx: &FileContext, path: &ast::Path) -> Option<(Vec<String>, Option<UseTree>)> {
	let tree = path.syntax().ancestors().find_map(UseTree::cast);

	if let Some(tree) = tree {
		if tree.path()?.syntax() != path.syntax() {
			return None;
		}

		let item = tree.syntax().ancestors().find_map(Use::cast)?;
		let binding = syntax::use_bindings(&item)?
			.into_iter()
			.find(|binding| binding.start == usize::from(tree.syntax().text_range().start()))?;
		let resolved = resolve_parts(ctx, item.syntax(), &binding.path, &mut BTreeSet::new())?;

		Some((resolved, Some(tree)))
	} else {
		Some((resolve_path(ctx, path)?, None))
	}
}

fn resolve_parts(
	ctx: &FileContext,
	node: &SyntaxNode,
	parts: &[String],
	visited: &mut BTreeSet<usize>,
) -> Option<Vec<String>> {
	let root = parts.first()?.as_str();

	if root.is_empty() || root == "crate" {
		return Some(parts.to_vec());
	}
	if matches!(root, "self" | "super") {
		let mut resolved = module_scope(ctx, node)?;
		let mut tail = parts;

		if root == "self" {
			tail = &tail[1..];
		}

		while tail.first().is_some_and(|part| part == "super") {
			if resolved.len() <= 1 {
				return None;
			}

			resolved.pop();

			tail = &tail[1..];
		}

		resolved.extend(tail.iter().cloned());

		return Some(resolved);
	}
	if root == "Self" {
		return None;
	}

	let mut resolved = match resolve_name(ctx, node, root, visited) {
		NameResolution::Found(path) => path,
		NameResolution::Blocked => return None,
		NameResolution::Unbound => {
			if parts.len() == 1 {
				return prelude_type_path(root);
			}

			vec![String::new(), root.to_owned()]
		},
	};

	resolved.extend(parts[1..].iter().cloned());

	Some(resolved)
}

fn resolve_name(
	ctx: &FileContext,
	node: &SyntaxNode,
	name: &str,
	visited: &mut BTreeSet<usize>,
) -> NameResolution {
	for ancestor in node.ancestors() {
		let shadows_generic = ancestor
			.children()
			.filter_map(GenericParamList::cast)
			.flat_map(|params| params.generic_params())
			.filter_map(|param| match param {
				GenericParam::TypeParam(param) => param.name(),
				_ => None,
			})
			.any(|param| syntax::is_same_ident(param.text(), name));

		if shadows_generic {
			return NameResolution::Blocked;
		}
		if !matches!(
			ancestor.kind(),
			SyntaxKind::SOURCE_FILE | SyntaxKind::ITEM_LIST | SyntaxKind::STMT_LIST
		) {
			continue;
		}

		let mut matches = Vec::new();
		let mut opaque = false;

		for item in ancestor.children().filter_map(Item::cast) {
			if let Some(declared) = type_item_name(&item)
				&& syntax::is_same_ident(&declared, name)
			{
				matches.push(
					item.syntax().parent().and_then(|parent| module_scope(ctx, &parent)).map(
						|mut path| {
							path.push(declared);

							path
						},
					),
				);
			}
			if let Item::Use(item) = &item {
				let offset = usize::from(item.syntax().text_range().start());
				let Some(bindings) = syntax::use_bindings(item) else {
					opaque = true;

					continue;
				};

				for binding in bindings {
					opaque |= binding.is_glob;

					if binding
						.name
						.as_ref()
						.is_some_and(|binding| syntax::is_same_ident(binding, name))
					{
						let resolved = if visited.insert(offset) {
							let result = resolve_parts(ctx, item.syntax(), &binding.path, visited);

							visited.remove(&offset);

							result
						} else {
							None
						};

						matches.push(resolved);
					}
				}
			}

			opaque |= matches!(item, Item::MacroCall(_));
		}

		if matches.len() == 1 {
			return matches.pop().flatten().map_or(NameResolution::Blocked, NameResolution::Found);
		}
		if !matches.is_empty() || opaque {
			return NameResolution::Blocked;
		}
		if ancestor.kind() == SyntaxKind::ITEM_LIST {
			break;
		}
	}

	NameResolution::Unbound
}

fn type_item_name(item: &Item) -> Option<String> {
	match item {
		Item::Module(item) => item.name(),
		Item::Struct(item) => item.name(),
		Item::Enum(item) => item.name(),
		Item::Union(item) => item.name(),
		Item::Trait(item) => item.name(),
		Item::TypeAlias(item) => item.name(),
		_ => None,
	}
	.map(|name| name.text().to_string())
}

fn prelude_type_path(name: &str) -> Option<Vec<String>> {
	if types::is_primitive_type_ident(name) {
		return Some(vec![name.to_owned()]);
	}

	let path = match name {
		"String" => "::std::string::String",
		"Vec" => "::std::vec::Vec",
		"Option" => "::core::option::Option",
		"Result" => "::core::result::Result",
		"Box" => "::std::boxed::Box",
		_ => return None,
	};

	Some(path.split("::").map(str::to_owned).collect())
}

fn path_key(path: &[String]) -> String {
	path.iter().map(|part| syntax::normalize_ident(part)).collect::<Vec<_>>().join("::")
}
