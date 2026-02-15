use ra_ap_syntax::{
	AstNode,
	ast::{self, BlockExpr, LetStmt},
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

fn is_whitespace_between(ctx: &FileContext, previous: &LetStmtInfo, next: &LetStmtInfo) -> bool {
	if previous.end > next.start {
		return false;
	}

	let Some(gap_text) = ctx.text.get(previous.end..next.start) else {
		return false;
	};

	gap_text.chars().all(char::is_whitespace)
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
