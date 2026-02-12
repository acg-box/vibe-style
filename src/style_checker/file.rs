use super::shared::{FileContext, SERDE_DEFAULT_RE, TopKind, Violation, push_violation};

pub(crate) fn check_mod_rs(ctx: &FileContext, violations: &mut Vec<Violation>) {
	if ctx.path.file_name().is_some_and(|name| name == "mod.rs") {
		push_violation(
			violations,
			ctx,
			1,
			"RUST-STYLE-FILE-001",
			"Do not use mod.rs. Use flat module files instead.",
			false,
		);
	}
}

fn next_non_attribute_line(lines: &[String], idx: usize) -> Option<usize> {
	let mut cursor = idx + 1;
	while cursor < lines.len() {
		let stripped = lines[cursor].trim();
		if stripped.is_empty()
			|| stripped.starts_with("#[")
			|| stripped.starts_with("///")
			|| stripped.starts_with("//!")
		{
			cursor += 1;
			continue;
		}
		return Some(cursor);
	}
	None
}

pub(crate) fn check_serde_option_default(ctx: &FileContext, violations: &mut Vec<Violation>) {
	for (idx, line) in ctx.lines.iter().enumerate() {
		if !SERDE_DEFAULT_RE.is_match(line) {
			continue;
		}
		let Some(next_idx) = next_non_attribute_line(&ctx.lines, idx) else {
			continue;
		};
		if !ctx.lines[next_idx].contains(": Option<") {
			continue;
		}
		push_violation(
			violations,
			ctx,
			idx + 1,
			"RUST-STYLE-SERDE-001",
			"Do not use #[serde(default)] on Option<T> fields.",
			false,
		);
	}
}

pub(crate) fn check_error_rs_no_use(ctx: &FileContext, violations: &mut Vec<Violation>) {
	if ctx.path.file_name().is_none_or(|name| name != "error.rs") {
		return;
	}

	for item in &ctx.top_items {
		if item.kind != TopKind::Use {
			continue;
		}
		push_violation(
			violations,
			ctx,
			item.line,
			"RUST-STYLE-IMPORT-005",
			"Do not add use imports in error.rs; use fully qualified paths.",
			false,
		);
	}
}
