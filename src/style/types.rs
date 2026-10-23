use std::collections::BTreeMap;

use ra_ap_syntax::{
	AstNode, SyntaxKind,
	ast::{
		self, GenericParam, HasGenericArgs, HasGenericParams, HasName, HasVisibility, Path,
		PathSegment, Type, TypeAlias, Use,
	},
};

use crate::style::{
	shared::{self, Edit, FileContext, Violation},
	syntax,
};

const RULE_ID: &str = "RUST-STYLE-TYPE-001";
const MESSAGE: &str = "Do not use type aliases that only rename another type.";

enum AliasGenericParam {
	Lifetime(String),
	Type(String),
}

#[derive(Clone, Debug)]
pub(crate) struct TypeAliasRenameFix {
	pub(crate) alias: String,
	pub(crate) target: String,
	pub(crate) definition_edits: Vec<Edit>,
}

pub(crate) fn check_type_alias_renames(ctx: &FileContext, violations: &mut Vec<Violation>) {
	for type_alias in ctx.source_file.syntax().descendants().filter_map(TypeAlias::cast) {
		if !is_meaningless_type_alias_item(&type_alias) {
			continue;
		}

		let Some(Type::PathType(path_type)) = type_alias.ty() else {
			continue;
		};
		let Some(aliases) = alias_generic_keys(&type_alias) else {
			continue;
		};
		let Some(rhs_path) = path_type.path() else {
			continue;
		};

		if !is_meaningless_alias(&rhs_path, &aliases) {
			continue;
		}

		let start = usize::from(path_type.syntax().text_range().start());
		let line = shared::line_from_offset(&ctx.line_starts, start);
		let fixable = type_alias_autofix_plan(ctx, &type_alias, &rhs_path, &aliases).is_some();

		shared::push_violation(violations, ctx, line, RULE_ID, MESSAGE, fixable);
	}
}

pub(crate) fn collect_type_alias_rename_fixes(ctx: &FileContext) -> Vec<TypeAliasRenameFix> {
	let mut out = Vec::new();

	for type_alias in ctx.source_file.syntax().descendants().filter_map(TypeAlias::cast) {
		if !is_meaningless_type_alias_item(&type_alias) {
			continue;
		}

		let Some(Type::PathType(path_type)) = type_alias.ty() else {
			continue;
		};
		let Some(aliases) = alias_generic_keys(&type_alias) else {
			continue;
		};
		let Some(rhs_path) = path_type.path() else {
			continue;
		};
		let Some(plan) = type_alias_autofix_plan(ctx, &type_alias, &rhs_path, &aliases) else {
			continue;
		};

		out.push(plan);
	}

	out
}

pub(crate) fn build_type_alias_usage_rename_edits(
	ctx: &FileContext,
	renames: &BTreeMap<String, String>,
	skip_ranges: &[(usize, usize)],
) -> Vec<Edit> {
	if renames.is_empty() {
		return Vec::new();
	}

	let mut edits = Vec::new();

	for segment in ctx.source_file.syntax().descendants().filter_map(PathSegment::cast) {
		let Some(name_ref) = segment.name_ref() else {
			continue;
		};
		let Some(replacement) = renames.get(name_ref.text()) else {
			continue;
		};
		let range = name_ref.syntax().text_range();
		let start = usize::from(range.start());
		let end = usize::from(range.end());

		if skip_ranges.iter().any(|(skip_start, skip_end)| start < *skip_end && end > *skip_start) {
			continue;
		}

		edits.push(Edit { start, end, replacement: replacement.clone(), rule: RULE_ID });
	}

	edits
}

fn is_meaningless_type_alias_item(type_alias: &TypeAlias) -> bool {
	let mut ancestor = type_alias.syntax().parent();

	while let Some(node) = ancestor {
		match node.kind() {
			SyntaxKind::ASSOC_ITEM_LIST => return false,
			SyntaxKind::ITEM_LIST | SyntaxKind::SOURCE_FILE | SyntaxKind::BLOCK_EXPR => {
				return true;
			},
			_ => {},
		}

		ancestor = node.parent();
	}

	true
}

fn is_meaningless_alias(path: &Path, aliases: &[AliasGenericParam]) -> bool {
	let mut segments = Vec::<PathSegment>::new();

	if !syntax::collect_simple_path_segments(path, &mut segments) {
		return false;
	}
	if segments.is_empty() {
		return false;
	}

	for segment in segments.iter().take(segments.len() - 1) {
		if segment.generic_arg_list().is_some() {
			return false;
		}
	}

	let Some(last_segment) = segments.last() else {
		return false;
	};
	let Some(last_generic_args) = last_segment.generic_arg_list() else {
		return aliases.is_empty();
	};
	let rhs_args = last_generic_args.generic_args().collect::<Vec<ra_ap_syntax::ast::GenericArg>>();

	if rhs_args.len() != aliases.len() {
		return false;
	}

	for (rhs_arg, alias) in rhs_args.iter().zip(aliases) {
		if !generic_arg_matches_param(rhs_arg, alias) {
			return false;
		}
	}

	true
}

fn type_alias_autofix_plan(
	ctx: &FileContext,
	type_alias: &TypeAlias,
	rhs_path: &Path,
	aliases: &[AliasGenericParam],
) -> Option<TypeAliasRenameFix> {
	if !is_meaningless_alias(rhs_path, aliases) {
		return None;
	}

	let alias_name = type_alias.name()?.text().to_string();
	let mut rhs_segments = Vec::<PathSegment>::new();

	if !syntax::collect_simple_path_segments(rhs_path, &mut rhs_segments) || rhs_segments.is_empty()
	{
		return None;
	}

	let target_segment = rhs_segments.last()?;
	let target_name = target_segment.name_ref()?.text().to_string();
	let alias_is_public = type_alias.visibility().is_some();
	let alias_start = usize::from(type_alias.syntax().text_range().start());
	let alias_end = usize::from(type_alias.syntax().text_range().end());
	let type_start = usize::from(type_alias.type_token()?.text_range().start());
	let prefix = ctx.text.get(alias_start..type_start)?;
	let mut definition_edits = Vec::new();

	if alias_is_public {
		if rhs_segments.len() >= 2 {
			let mut rhs_text = rhs_path.syntax().text().to_string();

			if let Some(args) = target_segment.generic_arg_list() {
				let base = usize::from(rhs_path.syntax().text_range().start());
				let range = args.syntax().text_range();

				rhs_text.replace_range(
					usize::from(range.start()) - base..usize::from(range.end()) - base,
					"",
				);
			}

			definition_edits.push(Edit {
				start: alias_start,
				end: alias_end,
				replacement: format!("{prefix}use {rhs_text};"),
				rule: RULE_ID,
			});
		} else if let Some((use_start, use_end, use_path, use_is_pub)) =
			find_simple_sibling_use_importing_ident(ctx, type_alias, target_name.as_str())
		{
			if use_is_pub {
				// The target is already exported; remove the alias and rewrite callers.
				definition_edits.push(Edit {
					start: alias_start,
					end: alias_end,
					replacement: String::new(),
					rule: RULE_ID,
				});
			} else {
				definition_edits.push(Edit {
					start: alias_start,
					end: alias_end,
					replacement: format!("{prefix}use {use_path};"),
					rule: RULE_ID,
				});
				definition_edits.push(Edit {
					start: use_start,
					end: use_end,
					replacement: String::new(),
					rule: RULE_ID,
				});
			}
		} else if is_primitive_type_ident(&target_name) {
			definition_edits.push(Edit {
				start: alias_start,
				end: alias_end,
				replacement: String::new(),
				rule: RULE_ID,
			});
		} else {
			return None;
		}
	} else {
		definition_edits.push(Edit {
			start: alias_start,
			end: alias_end,
			replacement: String::new(),
			rule: RULE_ID,
		});
	}

	Some(TypeAliasRenameFix { alias: alias_name, target: target_name, definition_edits })
}

fn is_primitive_type_ident(name: &str) -> bool {
	matches!(
		name,
		"bool"
			| "char"
			| "str"
			| "i8"
			| "i16"
			| "i32"
			| "i64"
			| "i128"
			| "isize"
			| "u8"
			| "u16"
			| "u32"
			| "u64"
			| "u128"
			| "usize"
			| "f32"
			| "f64"
	)
}

fn simple_use_path_text(text: &str) -> Option<String> {
	let text = text.trim();
	let (start, end) = syntax::find_use_path_range(text)?;
	let use_path = text.get(start..end)?.trim();

	if use_path.is_empty()
		|| use_path.contains('{')
		|| use_path.contains('}')
		|| use_path.contains('*')
		|| use_path.contains(" as ")
	{
		return None;
	}

	Some(use_path.to_string())
}

fn find_simple_sibling_use_importing_ident(
	ctx: &FileContext,
	type_alias: &TypeAlias,
	ident: &str,
) -> Option<(usize, usize, String, bool)> {
	let parent = type_alias.syntax().parent()?;

	for use_item in ctx.source_file.syntax().descendants().filter_map(Use::cast) {
		if use_item.syntax().parent() != Some(parent.clone()) {
			continue;
		}

		let Some(path_text) = simple_use_path_text(&use_item.syntax().text().to_string()) else {
			continue;
		};
		let last = path_text.rsplit("::").next().unwrap_or(path_text.as_str()).trim();

		if last != ident {
			continue;
		}

		let start = usize::from(use_item.syntax().text_range().start());
		let end = usize::from(use_item.syntax().text_range().end());
		let is_pub = use_item.visibility().is_some();

		return Some((start, end, path_text, is_pub));
	}

	None
}

fn generic_arg_matches_param(
	arg: &ra_ap_syntax::ast::GenericArg,
	alias: &AliasGenericParam,
) -> bool {
	match (arg, alias) {
		(ast::GenericArg::LifetimeArg(lifetime_arg), AliasGenericParam::Lifetime(expected)) => {
			let Some(lifetime) = lifetime_arg.lifetime() else {
				return false;
			};
			let Some(token) = lifetime.lifetime_ident_token() else {
				return false;
			};

			token.text() == expected.as_str()
		},
		(ast::GenericArg::TypeArg(type_arg), AliasGenericParam::Type(expected)) => {
			let Some(type_arg_type) = type_arg.ty() else {
				return false;
			};
			let Type::PathType(path_type) = type_arg_type else {
				return false;
			};
			let Some(path) = path_type.path() else {
				return false;
			};
			let mut segments = Vec::<PathSegment>::new();

			if !syntax::collect_simple_path_segments(&path, &mut segments) {
				return false;
			}
			if segments.len() != 1 {
				return false;
			}

			let Some(name_ref) = segments[0].name_ref() else {
				return false;
			};

			name_ref.text() == expected.as_str()
		},
		_ => false,
	}
}

fn alias_generic_keys(type_alias: &TypeAlias) -> Option<Vec<AliasGenericParam>> {
	let Some(generic_params) = type_alias.generic_param_list() else {
		return Some(Vec::new());
	};
	let mut out = Vec::new();

	for param in generic_params.generic_params() {
		match param {
			GenericParam::TypeParam(type_param) => {
				// A default generic parameter (for example `E = Error`) changes the alias API
				// surface, so it is not considered a pure rename.
				if type_param
					.syntax()
					.children_with_tokens()
					.any(|token| token.kind() == SyntaxKind::EQ)
				{
					return None;
				}

				let name = type_param.name()?;

				out.push(AliasGenericParam::Type(name.text().to_string()));
			},
			GenericParam::LifetimeParam(lifetime_param) => {
				let lifetime = lifetime_param.lifetime()?;
				let token = lifetime.lifetime_ident_token()?;

				out.push(AliasGenericParam::Lifetime(token.text().to_string()));
			},
			GenericParam::ConstParam(_) => return None,
		}
	}

	Some(out)
}

#[cfg(test)]
mod tests {
	use std::path::Path;

	use crate::style::{fixes, shared, types};

	#[test]
	fn alias_reexports_preserve_visibility_attributes_and_generic_constructors() {
		let original =
			"/// Collection.\n#[cfg(test)]\npub(crate) type Items<T> = std::vec::Vec<T>;\n";
		let ctx = shared::read_file_context_from_text(Path::new("aliases.rs"), original.to_owned())
			.expect("Parse source.")
			.expect("Source context.");
		let edits = types::collect_type_alias_rename_fixes(&ctx)
			.into_iter()
			.flat_map(|plan| plan.definition_edits)
			.collect();
		let mut rewritten = original.to_owned();

		fixes::apply_edits(&mut rewritten, edits).expect("Rewrite alias.");

		assert_eq!(rewritten, "/// Collection.\n#[cfg(test)]\npub(crate) use std::vec::Vec;\n");
	}

	#[test]
	fn alias_reexports_find_simple_imports_after_grouped_imports() {
		let original =
			"use std::fmt::{Debug, Display};\nuse std::io::Error;\npub type Failure = Error;\n";
		let ctx = shared::read_file_context_from_text(Path::new("aliases.rs"), original.to_owned())
			.expect("Parse source.")
			.expect("Source context.");

		assert_eq!(types::collect_type_alias_rename_fixes(&ctx).len(), 1);
	}
}
