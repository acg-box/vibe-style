use ra_ap_syntax::{
	AstNode,
	ast::{
		self, Expr, GenericArgList, HasGenericArgs, LetStmt, MethodCallExpr, Path, PathExpr,
		PathSegment, Type,
	},
};

use crate::style::shared::{self, Edit, FileContext, Violation};

const RULE_ID: &str = "RUST-STYLE-GENERICS-002";
const MESSAGE: &str = "Remove unnecessary turbofish; type is already explicit in the let binding.";

#[derive(Debug)]
struct ExplicitTypeInfo {
	name: String,
	generics: Vec<String>,
}

pub(crate) fn check_unnecessary_turbofish(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	for let_stmt in ctx.source_file.syntax().descendants().filter_map(LetStmt::cast) {
		let Some(let_type) = let_stmt.ty() else {
			continue;
		};
		let Some(initializer) = let_stmt.initializer() else {
			continue;
		};
		let explicit_type_text = strip_whitespace(&let_type.syntax().text().to_string());
		let let_line = shared::line_from_offset(
			&ctx.line_starts,
			usize::from(let_type.syntax().text_range().start()),
		);
		let normalized_initializer = unwrap_turbofish_wrappers(initializer);

		match normalized_initializer {
			ast::Expr::MethodCallExpr(method_call) => {
				if let Some(range) = method_call_turbofish_range(&method_call, &explicit_type_text)
				{
					push_fixable_violation(ctx, violations, edits, emit_edits, range, let_line);
				}
			},
			ast::Expr::CallExpr(call_expr) => {
				let Some(path_expr) = call_expr.expr() else {
					continue;
				};
				let ast::Expr::PathExpr(path_expr) = path_expr else {
					continue;
				};
				let Some(type_info) = explicit_type_segment_info(&let_type) else {
					continue;
				};

				if let Some(range) = associated_turbofish_range(&path_expr, &type_info) {
					push_fixable_violation(ctx, violations, edits, emit_edits, range, let_line);
				}
			},
			_ => {},
		}
	}
}

fn push_fixable_violation(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
	range: (usize, usize),
	line: usize,
) {
	shared::push_violation(violations, ctx, line, RULE_ID, MESSAGE, true);

	if !emit_edits {
		return;
	}

	let (start, end) = range;

	if end > start {
		edits.push(Edit { start, end, replacement: String::new(), rule: RULE_ID });
	}
}

fn method_call_turbofish_range(
	method_call: &MethodCallExpr,
	explicit_type_text: &str,
) -> Option<(usize, usize)> {
	let generic_arg_list = method_call.generic_arg_list()?;
	let args = generic_arg_list.generic_args().collect::<Vec<_>>();

	if args.len() != 1 {
		return None;
	}

	let ast::GenericArg::TypeArg(type_arg) = &args[0] else {
		return None;
	};
	let type_arg_type = type_arg.ty()?;
	let method_type = strip_whitespace(&type_arg_type.syntax().text().to_string());

	if method_type != explicit_type_text || method_type.is_empty() {
		return None;
	}

	let range = generic_arg_list.syntax().text_range();

	Some((usize::from(range.start()), usize::from(range.end())))
}

fn associated_turbofish_range(
	path_expr: &PathExpr,
	type_info: &ExplicitTypeInfo,
) -> Option<(usize, usize)> {
	let path = path_expr.path()?;
	let mut segments = Vec::<PathSegment>::new();

	if !collect_simple_path_segments(&path, &mut segments) {
		return None;
	}

	let mut matching: Option<PathSegment> = None;

	for segment in segments {
		if segment.generic_arg_list().is_none() {
			continue;
		}
		if matching.is_some() {
			return None;
		}

		matching = Some(segment);
	}

	let segment = matching?;
	let generic_arg_list = segment.generic_arg_list()?;
	let name = segment.name_ref().map(|name| name.text().to_string())?;

	if name != type_info.name {
		return None;
	}
	if generic_args_as_text(&generic_arg_list) != type_info.generics {
		return None;
	}

	let range = generic_arg_list.syntax().text_range();

	Some((usize::from(range.start()), usize::from(range.end())))
}

fn explicit_type_segment_info(let_type: &Type) -> Option<ExplicitTypeInfo> {
	let Type::PathType(path_type) = let_type else {
		return None;
	};
	let path = path_type.path()?;
	let mut segments = Vec::<PathSegment>::new();

	if !collect_simple_path_segments(&path, &mut segments) {
		return None;
	}

	let last_segment = segments.last()?;
	let name = last_segment.name_ref().map(|name| name.text().to_string())?;
	let generics =
		last_segment.generic_arg_list().map_or_else(Vec::new, |list| generic_args_as_text(&list));

	Some(ExplicitTypeInfo { name, generics })
}

fn generic_args_as_text(generic_arg_list: &GenericArgList) -> Vec<String> {
	generic_arg_list
		.generic_args()
		.map(|arg| strip_whitespace(&arg.syntax().text().to_string()))
		.collect::<Vec<_>>()
}

fn unwrap_turbofish_wrappers(mut expr: Expr) -> Expr {
	loop {
		let next = match &expr {
			ast::Expr::ParenExpr(paren_expr) => paren_expr.expr(),
			ast::Expr::TryExpr(try_expr) => try_expr.expr(),
			ast::Expr::AwaitExpr(await_expr) => await_expr.expr(),
			ast::Expr::RefExpr(ref_expr) => ref_expr.expr(),
			_ => None,
		};

		match next {
			Some(next_expr) => expr = next_expr,
			None => return expr,
		}
	}
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

fn strip_whitespace(value: &str) -> String {
	value.chars().filter(|ch| !ch.is_whitespace()).collect::<String>()
}
