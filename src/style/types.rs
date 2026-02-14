use ra_ap_syntax::{
	AstNode,
	ast::{
		self, GenericArg, GenericParam, HasGenericArgs, HasGenericParams, HasName, Path,
		PathSegment, Type, TypeAlias,
	},
};

use crate::style::shared::{self, FileContext, Violation};

enum AliasGenericParam {
	Lifetime(String),
	Type(String),
}

pub(crate) fn check_type_alias_renames(ctx: &FileContext, violations: &mut Vec<Violation>) {
	for type_alias in ctx.source_file.syntax().descendants().filter_map(TypeAlias::cast) {
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

		shared::push_violation(
			violations,
			ctx,
			line,
			"RUST-STYLE-TYPE-001",
			"Do not use type aliases that only rename another type.",
			false,
		);
	}
}

fn is_meaningless_alias(path: &Path, aliases: &[AliasGenericParam]) -> bool {
	let mut segments = Vec::<PathSegment>::new();

	if !collect_simple_path_segments(path, &mut segments) {
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
	let rhs_args = last_generic_args.generic_args().collect::<Vec<GenericArg>>();

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

fn generic_arg_matches_param(arg: &GenericArg, alias: &AliasGenericParam) -> bool {
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

			if !collect_simple_path_segments(&path, &mut segments) {
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

fn collect_simple_path_segments(path: &Path, out: &mut Vec<PathSegment>) -> bool {
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
