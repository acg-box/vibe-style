use std::collections::BTreeSet;

use ra_ap_syntax::{
	AstNode, SyntaxKind,
	ast::{self, BlockExpr, HasName, LetStmt, Path},
};

use crate::style::shared::{self, Edit, FileContext, Violation};

const RULE_ID: &str = "RUST-STYLE-LET-001";
const MESSAGE: &str = "Place immutable `let` bindings before mutable ones.";

#[derive(Debug)]
struct LetStmtInfo {
	start: usize,
	end: usize,
	is_mut: bool,
	line: usize,
	stmt: LetStmt,
}

pub(crate) fn check_let_mut_reorder(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	for block in ctx.source_file.syntax().descendants().filter_map(BlockExpr::cast) {
		let Some(stmt_list) = block.stmt_list() else {
			continue;
		};
		let mut current_run: Vec<LetStmtInfo> = Vec::new();

		for statement in stmt_list.statements() {
			match statement {
				ast::Stmt::LetStmt(let_stmt) => {
					let info = collect_let_stmt_info(ctx, &let_stmt);

					if let Some(previous) = current_run.last()
						&& !is_whitespace_between(ctx, previous, &info)
					{
						emit_run_fix(ctx, &current_run, violations, edits, emit_edits);

						current_run.clear();
					}

					current_run.push(info);
				},
				_ => {
					emit_run_fix(ctx, &current_run, violations, edits, emit_edits);

					current_run.clear();
				},
			}
		}

		emit_run_fix(ctx, &current_run, violations, edits, emit_edits);
	}
}

fn collect_let_stmt_info(ctx: &FileContext, let_stmt: &LetStmt) -> LetStmtInfo {
	let range = let_stmt.syntax().text_range();
	let start = usize::from(range.start());

	LetStmtInfo {
		start,
		end: usize::from(range.end()),
		is_mut: let_stmt_is_mut_ident(let_stmt),
		line: shared::line_from_offset(&ctx.line_starts, start),
		stmt: let_stmt.clone(),
	}
}

fn let_stmt_is_mut_ident(let_stmt: &LetStmt) -> bool {
	let Some(pat) = let_stmt.pat() else {
		return false;
	};

	match pat {
		ast::Pat::IdentPat(ident_pat) => ident_pat.mut_token().is_some(),
		_ => false,
	}
}

fn let_stmt_mut_ident_name(let_stmt: &LetStmt) -> Option<String> {
	let pat = let_stmt.pat()?;

	match pat {
		ast::Pat::IdentPat(ident_pat) => {
			ident_pat.mut_token()?;

			Some(ident_pat.name()?.text().to_string())
		},
		_ => None,
	}
}

fn is_whitespace_between(ctx: &FileContext, previous: &LetStmtInfo, next: &LetStmtInfo) -> bool {
	if previous.end > next.start {
		return false;
	}

	let Some(gap_text) = ctx.text.get(previous.end..next.start) else {
		return false;
	};

	gap_text.chars().all(char::is_whitespace)
}

fn let_stmt_references_unqualified_ident(let_stmt: &LetStmt, name: &str) -> bool {
	let_stmt.syntax().descendants().filter_map(Path::cast).any(|path| {
		if path.qualifier().is_some() {
			return false;
		}

		let Some(segment) = path.segment() else {
			return false;
		};
		let Some(name_ref) = segment.name_ref() else {
			return false;
		};

		name_ref.text() == name
	}) || let_stmt
		.syntax()
		.descendants_with_tokens()
		.filter_map(|element| element.into_token())
		.filter(|token| token.kind() == SyntaxKind::IDENT && token.text() == name)
		.any(|token| token.parent_ancestors().any(|node| node.kind() == SyntaxKind::TOKEN_TREE))
}

fn let_stmt_borrowed_unqualified_idents(let_stmt: &LetStmt) -> BTreeSet<String> {
	let mut out = BTreeSet::<String>::new();

	for ref_expr in let_stmt.syntax().descendants().filter_map(ast::RefExpr::cast) {
		let Some(ast::Expr::PathExpr(path_expr)) = ref_expr.expr() else {
			continue;
		};
		let Some(path) = path_expr.path() else {
			continue;
		};

		if path.qualifier().is_some() {
			continue;
		}

		let Some(segment) = path.segment() else {
			continue;
		};
		let Some(name_ref) = segment.name_ref() else {
			continue;
		};

		out.insert(name_ref.text().to_string());
	}

	out
}

fn let_stmt_by_value_unqualified_idents(let_stmt: &LetStmt) -> BTreeSet<String> {
	let mut out = BTreeSet::<String>::new();

	for path_expr in let_stmt.syntax().descendants().filter_map(ast::PathExpr::cast) {
		let Some(path) = path_expr.path() else {
			continue;
		};

		if path.qualifier().is_some() {
			continue;
		}

		let Some(segment) = path.segment() else {
			continue;
		};
		let Some(name_ref) = segment.name_ref() else {
			continue;
		};

		out.insert(name_ref.text().to_string());
	}
	// Remove idents that are explicitly borrowed via `&ident` or `&mut ident`.
	for borrowed in let_stmt_borrowed_unqualified_idents(let_stmt) {
		out.remove(&borrowed);
	}
	// If a name is only mentioned inside a macro token tree, treat it as a by-value use since we
	// cannot reliably model borrow/move semantics there.
	for token in let_stmt
		.syntax()
		.descendants_with_tokens()
		.filter_map(|element| element.into_token())
		.filter(|token| token.kind() == SyntaxKind::IDENT)
	{
		if token.parent_ancestors().any(|node| node.kind() == SyntaxKind::TOKEN_TREE) {
			out.insert(token.text().to_string());
		}
	}

	out
}

fn run_is_reorderable_without_binding_errors(run: &[LetStmtInfo]) -> bool {
	if run.len() <= 1 {
		return false;
	}

	let mut seen_mut = false;
	let mut mut_bindings: Vec<String> = Vec::new();
	let mut mut_borrowed_idents = BTreeSet::<String>::new();
	let mut mut_by_value_idents = BTreeSet::<String>::new();

	for stmt in run {
		if stmt.is_mut {
			seen_mut = true;

			if let Some(name) = let_stmt_mut_ident_name(&stmt.stmt) {
				mut_bindings.push(name);
			}

			for borrowed in let_stmt_borrowed_unqualified_idents(&stmt.stmt) {
				if borrowed != "self" {
					mut_borrowed_idents.insert(borrowed);
				}
			}
			for by_value in let_stmt_by_value_unqualified_idents(&stmt.stmt) {
				if by_value != "self" {
					mut_by_value_idents.insert(by_value);
				}
			}

			continue;
		}
		if !seen_mut || mut_bindings.is_empty() {
			continue;
		}
		// If an immutable let depends on a previous `let mut`, reordering would either produce
		// an unbound identifier or change bindings in ways the tool cannot safely validate.
		if mut_bindings.iter().any(|name| let_stmt_references_unqualified_ident(&stmt.stmt, name)) {
			return false;
		}

		let borrowed = let_stmt_borrowed_unqualified_idents(&stmt.stmt);
		let by_value = let_stmt_by_value_unqualified_idents(&stmt.stmt);

		// If a previous mutable binding borrows some name (e.g. `&scored`) and a later immutable
		// binding uses that same name by value (e.g. `consume(scored)`), swapping the two lets can
		// change move/borrow ordering and fail to compile.
		if by_value.iter().any(|name| name != "self" && mut_borrowed_idents.contains(name)) {
			return false;
		}
		if borrowed.iter().any(|name| name != "self" && mut_by_value_idents.contains(name)) {
			return false;
		}
	}

	true
}

fn emit_run_fix(
	ctx: &FileContext,
	run: &[LetStmtInfo],
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	emit_edits: bool,
) {
	if run.len() <= 1 {
		return;
	}

	let mut seen_mut = false;
	let mut offending_line = None;

	for stmt in run {
		if stmt.is_mut {
			seen_mut = true;

			continue;
		}
		if seen_mut {
			offending_line = Some(stmt.line);

			break;
		}
	}

	let Some(offending_line) = offending_line else {
		return;
	};

	if !run_is_reorderable_without_binding_errors(run) {
		// The project convention is to only enforce this rule when the `let` bindings can be
		// safely reordered. If not, treat it as compliant rather than emitting a violation.
		return;
	}

	let can_edit = can_rewrite_run(ctx, run);

	shared::push_violation(violations, ctx, offending_line, RULE_ID, MESSAGE, can_edit);

	if !emit_edits || !can_edit {
		return;
	}

	let start = run.first().map(|stmt| stmt.start).unwrap_or(0);
	let end = run.last().map(|stmt| stmt.end).unwrap_or(0);

	if end <= start {
		return;
	}

	let separator = ctx.text.get(run[0].end..run[1].start).expect("Expected gap to exist.");
	let mut reordered = Vec::with_capacity(run.len());

	reordered.extend(run.iter().filter(|stmt| !stmt.is_mut));
	reordered.extend(run.iter().filter(|stmt| stmt.is_mut));

	let mut replacement = String::with_capacity(end.saturating_sub(start));

	for (idx, stmt) in reordered.iter().enumerate() {
		let Some(stmt_text) = ctx.text.get(stmt.start..stmt.end) else {
			return;
		};

		if idx > 0 {
			replacement.push_str(separator);
		}

		replacement.push_str(stmt_text);
	}

	if replacement != ctx.text[start..end] {
		edits.push(Edit { start, end, replacement, rule: RULE_ID });
	}
}

fn can_rewrite_run(ctx: &FileContext, run: &[LetStmtInfo]) -> bool {
	if run.len() <= 1 {
		return false;
	}

	let Some(gap0) = ctx.text.get(run[0].end..run[1].start) else {
		return false;
	};

	if !gap0.chars().all(char::is_whitespace) {
		return false;
	}

	for window in run.windows(2) {
		let (a, b) = (&window[0], &window[1]);
		let Some(gap) = ctx.text.get(a.end..b.start) else {
			return false;
		};

		if gap != gap0 {
			return false;
		}
	}

	true
}

#[cfg(test)]
mod tests {
	use std::path::Path;

	use crate::style::{bindings, shared};

	#[test]
	fn let_else_dependency_is_treated_as_compliant() {
		let text = r#"
fn demo(opt: Option<u8>) -> usize {
	let mut out = 0usize;
	let Some(value) = opt else {
		return out;
	};
	let immutable = value as usize;

	out + immutable
}
"#;
		let ctx =
			shared::read_file_context_from_text(Path::new("let_else_dep.rs"), text.to_owned())
				.expect("context")
				.expect("has ctx");
		let mut violations = Vec::new();
		let mut edits = Vec::new();

		bindings::check_let_mut_reorder(&ctx, &mut violations, &mut edits, true);

		assert!(
			!violations.iter().any(|v| v.rule == super::RULE_ID),
			"did not expect {} violation for let-else dependency case",
			super::RULE_ID
		);
		assert!(edits.iter().all(|e| e.rule != super::RULE_ID));
	}

	#[test]
	fn move_after_borrow_dependency_is_treated_as_compliant() {
		let text = r#"
fn consume(v: Vec<u8>) -> Vec<u8> {
	v
}

fn borrow_len(v: &Vec<u8>) -> usize {
	v.len()
}

fn demo(scored: Vec<u8>) -> usize {
	let mut trace_len = borrow_len(&scored);
	let results = consume(scored);

	trace_len += results.len();

	trace_len
}
"#;
		let ctx =
			shared::read_file_context_from_text(Path::new("move_after_borrow.rs"), text.to_owned())
				.expect("context")
				.expect("has ctx");
		let mut violations = Vec::new();
		let mut edits = Vec::new();

		bindings::check_let_mut_reorder(&ctx, &mut violations, &mut edits, true);

		assert!(
			!violations.iter().any(|v| v.rule == super::RULE_ID),
			"did not expect {} violation for borrow/move dependency case",
			super::RULE_ID
		);
		assert!(edits.iter().all(|e| e.rule != super::RULE_ID));
	}
}
