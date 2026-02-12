// std
use std::{
	collections::{BTreeSet, HashMap, HashSet},
	fs,
	path::{Path, PathBuf},
	process::Command,
};

// crates.io
use once_cell::sync::Lazy;
use ra_ap_syntax::{
	AstNode, Edition, SourceFile, TextRange,
	ast::{self, HasArgList, HasAttrs, HasModuleItem, HasName, HasTypeBounds, HasVisibility},
};
use regex::Regex;

// self
use crate::prelude::*;

const STYLE_RULE_IDS: [&str; 27] = [
	"RUST-STYLE-MOD-001",
	"RUST-STYLE-MOD-002",
	"RUST-STYLE-MOD-003",
	"RUST-STYLE-MOD-005",
	"RUST-STYLE-MOD-007",
	"RUST-STYLE-FILE-001",
	"RUST-STYLE-SERDE-001",
	"RUST-STYLE-IMPORT-001",
	"RUST-STYLE-IMPORT-002",
	"RUST-STYLE-IMPORT-003",
	"RUST-STYLE-IMPORT-004",
	"RUST-STYLE-IMPORT-005",
	"RUST-STYLE-IMPORT-006",
	"RUST-STYLE-IMPORT-007",
	"RUST-STYLE-IMPL-001",
	"RUST-STYLE-IMPL-003",
	"RUST-STYLE-GENERICS-001",
	"RUST-STYLE-LOG-002",
	"RUST-STYLE-RUNTIME-001",
	"RUST-STYLE-RUNTIME-002",
	"RUST-STYLE-NUM-001",
	"RUST-STYLE-NUM-002",
	"RUST-STYLE-READ-002",
	"RUST-STYLE-SPACE-003",
	"RUST-STYLE-SPACE-004",
	"RUST-STYLE-TEST-001",
	"RUST-STYLE-TEST-002",
];

static SERDE_DEFAULT_RE: Lazy<Regex> =
	Lazy::new(|| Regex::new(r"^\s*#\s*\[\s*serde\s*\(\s*default\b[^)]*\)\s*]\s*$").unwrap());
static USE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*(pub\s+)?use\s+(.+);\s*$").unwrap());
static INLINE_BOUNDS_RE: Lazy<Regex> = Lazy::new(|| {
	Regex::new(
		r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:fn|impl|struct|enum|trait)\b[^\n{;]*<[^>{}]*\b(?:[A-Za-z_][A-Za-z0-9_]*|'[A-Za-z_][A-Za-z0-9_]*)\s*:[^>{}]*>",
	)
	.unwrap()
});
static STD_QUALIFIED_MACRO_RE: Lazy<Regex> = Lazy::new(|| {
	Regex::new(r"\bstd::(vec|format|println|eprintln|dbg|write|writeln)!\s*\(").unwrap()
});
static NUM_SUFFIX_RE: Lazy<Regex> = Lazy::new(|| {
	Regex::new(r"\b\d+(?:\.\d+)?(f32|f64|i8|i16|i32|i64|i128|isize|u8|u16|u32|u64|u128|usize)\b")
		.unwrap()
});
static PLAIN_INT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b[1-9]\d{3,}\b").unwrap());
static SNAKE_CASE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[a-z][a-z0-9_]*$").unwrap());

#[derive(Debug, Clone)]
struct Violation {
	file: PathBuf,
	line: usize,
	rule: &'static str,
	message: String,
	fixable: bool,
}

impl Violation {
	fn format(&self) -> String {
		format!(
			"{}:{}:1: [{}] {}{}",
			self.file.display(),
			self.line,
			self.rule,
			self.message,
			if self.fixable { " (fixable)" } else { "" }
		)
	}
}

#[derive(Debug, Clone)]
struct Edit {
	start: usize,
	end: usize,
	replacement: String,
	rule: &'static str,
}

#[derive(Debug, Clone)]
pub(crate) struct RunSummary {
	pub(crate) file_count: usize,
	pub(crate) violation_count: usize,
	pub(crate) unfixable_count: usize,
	pub(crate) applied_fix_count: usize,
	pub(crate) output_lines: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum TopKind {
	Mod,
	Use,
	MacroRules,
	Type,
	Const,
	Static,
	Trait,
	Enum,
	Struct,
	Impl,
	Fn,
	Other,
}

#[derive(Debug, Clone)]
struct TopItem {
	kind: TopKind,
	name: Option<String>,
	line: usize,
	start_line: usize,
	end_line: usize,
	is_pub: bool,
	is_async: bool,
	attrs: Vec<String>,
	impl_target: Option<String>,
	raw: String,
}

#[derive(Debug)]
struct FileContext {
	path: PathBuf,
	text: String,
	lines: Vec<String>,
	line_starts: Vec<usize>,
	source_file: SourceFile,
	top_items: Vec<TopItem>,
}

pub(crate) fn run_check(requested_files: &[PathBuf]) -> Result<RunSummary> {
	let files = resolve_files(requested_files)?;
	let mut violations: Vec<Violation> = Vec::new();

	for file in &files {
		if let Some(ctx) = read_file_context(file)? {
			let (mut found, _edits) = collect_violations(&ctx, true);
			violations.append(&mut found);
		}
	}

	violations
		.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)).then(a.rule.cmp(b.rule)));

	let unfixable_count = violations.iter().filter(|v| !v.fixable).count();
	let output_lines = violations.into_iter().map(|v| v.format()).collect::<Vec<_>>();
	let violation_count = output_lines.len();

	Ok(RunSummary {
		file_count: files.len(),
		violation_count,
		unfixable_count,
		applied_fix_count: 0,
		output_lines,
	})
}

pub(crate) fn run_fix(requested_files: &[PathBuf]) -> Result<RunSummary> {
	let files = resolve_files(requested_files)?;
	let mut output_lines = Vec::new();
	let mut total_applied = 0_usize;

	for file in &files {
		let mut text = match fs::read_to_string(file) {
			Ok(text) => text,
			Err(_) => continue,
		};

		let mut pass = 0;
		let mut changed = false;

		while pass < 8 {
			pass += 1;
			let Some(ctx) = read_file_context_from_text(file, text.clone())? else {
				break;
			};
			let (_violations, edits) = collect_violations(&ctx, true);
			if edits.is_empty() {
				break;
			}

			let applied = apply_edits(&mut text, edits)?;
			if applied == 0 {
				break;
			}
			total_applied += applied;
			changed = true;
		}

		if changed {
			fs::write(file, text)?;
		}
	}

	let checked = run_check(requested_files)?;
	output_lines.extend(checked.output_lines);

	Ok(RunSummary {
		file_count: checked.file_count,
		violation_count: checked.violation_count,
		unfixable_count: checked.unfixable_count,
		applied_fix_count: total_applied,
		output_lines,
	})
}

pub(crate) fn print_coverage() {
	for rule in STYLE_RULE_IDS {
		println!("{rule}\timplemented");
	}
}

fn resolve_files(requested_files: &[PathBuf]) -> Result<Vec<PathBuf>> {
	if !requested_files.is_empty() {
		let mut files = Vec::new();
		for file in requested_files {
			if file.extension().is_some_and(|ext| ext == "rs") {
				files.push(file.clone());
			}
		}
		return Ok(files);
	}

	let output = Command::new("git")
		.args(["ls-files", "*.rs"])
		.output()
		.map_err(|err| eyre::eyre!("Failed to run git ls-files: {err}."))?;

	if !output.status.success() {
		return Err(eyre::eyre!("git ls-files failed with status {}.", output.status));
	}

	let stdout = String::from_utf8(output.stdout)?;
	let mut files = Vec::new();
	for line in stdout.lines() {
		if !line.is_empty() {
			files.push(PathBuf::from(line));
		}
	}
	Ok(files)
}

fn read_file_context(path: &Path) -> Result<Option<FileContext>> {
	let text = match fs::read_to_string(path) {
		Ok(text) => text,
		Err(_) => return Ok(None),
	};
	read_file_context_from_text(path, text)
}

fn read_file_context_from_text(path: &Path, text: String) -> Result<Option<FileContext>> {
	if text.is_empty() {
		return Ok(None);
	}
	let lines = text.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
	let line_starts = build_line_starts(&text);
	let parse = SourceFile::parse(&text, Edition::CURRENT);
	let source_file = parse.tree();
	let top_items = collect_top_items(&source_file, &line_starts);

	Ok(Some(FileContext {
		path: path.to_path_buf(),
		text,
		lines,
		line_starts,
		source_file,
		top_items,
	}))
}

fn build_line_starts(text: &str) -> Vec<usize> {
	let mut starts = vec![0_usize];
	for (idx, ch) in text.char_indices() {
		if ch == '\n' {
			starts.push(idx + 1);
		}
	}
	starts
}

fn line_from_offset(line_starts: &[usize], offset: usize) -> usize {
	match line_starts.binary_search(&offset) {
		Ok(pos) => pos + 1,
		Err(pos) => pos,
	}
}

fn offset_from_line(line_starts: &[usize], line_one_based: usize) -> Option<usize> {
	if line_one_based == 0 {
		return None;
	}
	line_starts.get(line_one_based - 1).copied()
}

fn text_range_to_lines(line_starts: &[usize], range: TextRange) -> (usize, usize) {
	let start = usize::from(range.start());
	let end_exclusive = usize::from(range.end());
	let start_line = line_from_offset(line_starts, start);
	let end_line = if end_exclusive == 0 {
		1
	} else {
		line_from_offset(line_starts, end_exclusive.saturating_sub(1))
	};
	(start_line, end_line)
}

fn collect_top_items(source_file: &SourceFile, line_starts: &[usize]) -> Vec<TopItem> {
	let mut items = Vec::new();
	for item in source_file.items() {
		let kind = classify_top_kind(&item);
		let name = item_name(&item);
		let is_pub = item_visibility_is_pub(&item);
		let is_async = matches!(&item, ast::Item::Fn(func) if func.async_token().is_some());
		let attrs = item.attrs().map(|attr| attr.syntax().text().to_string()).collect::<Vec<_>>();
		let impl_target = if let ast::Item::Impl(impl_item) = &item {
			impl_item
				.self_ty()
				.and_then(|ty| extract_impl_target_name(&ty.syntax().text().to_string()))
		} else {
			None
		};
		let raw = item.syntax().text().to_string();
		let (start_line, end_line) = text_range_to_lines(line_starts, item.syntax().text_range());

		items.push(TopItem {
			kind,
			name,
			line: start_line,
			start_line,
			end_line,
			is_pub,
			is_async,
			attrs,
			impl_target,
			raw,
		});
	}
	items
}

fn classify_top_kind(item: &ast::Item) -> TopKind {
	match item {
		ast::Item::Module(_) => TopKind::Mod,
		ast::Item::Use(_) => TopKind::Use,
		ast::Item::MacroRules(_) => TopKind::MacroRules,
		ast::Item::TypeAlias(_) => TopKind::Type,
		ast::Item::Const(_) => TopKind::Const,
		ast::Item::Static(_) => TopKind::Static,
		ast::Item::Trait(_) => TopKind::Trait,
		ast::Item::Enum(_) => TopKind::Enum,
		ast::Item::Struct(_) => TopKind::Struct,
		ast::Item::Impl(_) => TopKind::Impl,
		ast::Item::Fn(_) => TopKind::Fn,
		_ => TopKind::Other,
	}
}

fn item_name(item: &ast::Item) -> Option<String> {
	match item {
		ast::Item::Module(node) => node.name().map(|name| name.text().to_string()),
		ast::Item::TypeAlias(node) => node.name().map(|name| name.text().to_string()),
		ast::Item::Const(node) => node.name().map(|name| name.text().to_string()),
		ast::Item::Static(node) => node.name().map(|name| name.text().to_string()),
		ast::Item::Trait(node) => node.name().map(|name| name.text().to_string()),
		ast::Item::Enum(node) => node.name().map(|name| name.text().to_string()),
		ast::Item::Struct(node) => node.name().map(|name| name.text().to_string()),
		ast::Item::Fn(node) => node.name().map(|name| name.text().to_string()),
		_ => None,
	}
}

fn item_visibility_is_pub(item: &ast::Item) -> bool {
	match item {
		ast::Item::Module(node) => node.visibility().is_some(),
		ast::Item::Use(node) => node.visibility().is_some(),
		ast::Item::TypeAlias(node) => node.visibility().is_some(),
		ast::Item::Const(node) => node.visibility().is_some(),
		ast::Item::Static(node) => node.visibility().is_some(),
		ast::Item::Trait(node) => node.visibility().is_some(),
		ast::Item::Enum(node) => node.visibility().is_some(),
		ast::Item::Struct(node) => node.visibility().is_some(),
		ast::Item::Fn(node) => node.visibility().is_some(),
		ast::Item::Impl(node) => node.visibility().is_some(),
		_ => false,
	}
}

fn extract_impl_target_name(ty_text: &str) -> Option<String> {
	let ty_text = ty_text.trim();
	if ty_text.is_empty() {
		return None;
	}
	let text = ty_text
		.split(['<', '{', ' '])
		.next()
		.unwrap_or(ty_text)
		.rsplit("::")
		.next()
		.unwrap_or(ty_text)
		.trim();
	if text.is_empty() { None } else { Some(text.to_string()) }
}

fn collect_violations(ctx: &FileContext, with_fixes: bool) -> (Vec<Violation>, Vec<Edit>) {
	let mut violations = Vec::new();
	let mut edits = Vec::new();

	check_mod_rs(ctx, &mut violations);
	check_serde_option_default(ctx, &mut violations);
	check_error_rs_no_use(ctx, &mut violations);
	check_import_rules(ctx, &mut violations, &mut edits, with_fixes);
	check_module_order(ctx, &mut violations);
	check_cfg_test_mod_tests_use_super(ctx, &mut violations, &mut edits, with_fixes);
	check_impl_adjacency(ctx, &mut violations);
	check_impl_rules(ctx, &mut violations, &mut edits, with_fixes);
	check_inline_trait_bounds(ctx, &mut violations);
	check_std_macro_calls(ctx, &mut violations, &mut edits, with_fixes);
	check_logging_quality(ctx, &mut violations);
	check_expect_unwrap(ctx, &mut violations);
	check_numeric_literals(ctx, &mut violations, &mut edits, with_fixes);
	check_function_length(ctx, &mut violations);
	check_vertical_spacing(ctx, &mut violations);
	check_test_rules(ctx, &mut violations);

	(violations, edits)
}

fn push_violation(
	violations: &mut Vec<Violation>,
	ctx: &FileContext,
	line: usize,
	rule: &'static str,
	message: &str,
	fixable: bool,
) {
	violations.push(Violation {
		file: ctx.path.clone(),
		line,
		rule,
		message: message.to_owned(),
		fixable,
	});
}

fn check_mod_rs(ctx: &FileContext, violations: &mut Vec<Violation>) {
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

fn check_serde_option_default(ctx: &FileContext, violations: &mut Vec<Violation>) {
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

fn check_error_rs_no_use(ctx: &FileContext, violations: &mut Vec<Violation>) {
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

fn extract_use_path_from_line(line: &str) -> Option<String> {
	USE_RE
		.captures(line)
		.and_then(|caps| caps.get(2).map(|capture| capture.as_str().trim().to_owned()))
}

fn imported_symbols_from_use_path(path: &str) -> Vec<String> {
	let compact = path.replace(' ', "");
	if compact.ends_with("::*") {
		return Vec::new();
	}

	fn normalize_symbol(segment: &str) -> Option<String> {
		let mut symbol = segment.trim().to_owned();
		if symbol.is_empty() {
			return None;
		}
		if let Some((left, _)) = symbol.split_once(" as ") {
			symbol = left.trim().to_owned();
		}
		if matches!(symbol.as_str(), "*" | "self" | "super" | "crate") {
			return None;
		}
		if let Some((_, right)) = symbol.rsplit_once("::") {
			symbol = right.to_owned();
		}
		if let Some(stripped) = symbol.strip_prefix("r#") {
			symbol = stripped.to_owned();
		}
		if symbol.is_empty() { None } else { Some(symbol) }
	}

	if path.contains('{') && path.contains('}') {
		let inside = path
			.split_once('{')
			.and_then(|(_, right)| right.rsplit_once('}').map(|(inside, _)| inside))
			.unwrap_or_default();
		let mut out = Vec::new();
		for segment in inside.split(',') {
			if let Some(symbol) = normalize_symbol(segment) {
				out.push(symbol);
			}
		}
		return out;
	}

	let tail = path.rsplit("::").next().unwrap_or(path);
	normalize_symbol(tail).into_iter().collect()
}

fn contains_unqualified_symbol_call(lines: &[String], symbol: &str, is_macro: bool) -> bool {
	let pattern = if is_macro {
		format!(r"\b{}!\s*\(", regex::escape(symbol))
	} else {
		format!(r"\b{}\s*\(", regex::escape(symbol))
	};
	let re = Regex::new(&pattern).unwrap();

	for line in lines {
		let code = strip_string_and_line_comment(line, false).0;
		for matched in re.find_iter(&code) {
			let prev =
				if matched.start() == 0 { None } else { code[..matched.start()].chars().last() };
			if prev != Some(':') {
				return true;
			}
		}
	}
	false
}

fn use_origin(path: &str) -> usize {
	let trimmed = path.replace("pub ", "");
	let root = trimmed.trim_start_matches(':').split("::").next().unwrap_or_default();
	if matches!(root, "std" | "core" | "alloc") {
		0
	} else if matches!(root, "crate" | "self" | "super") || root.starts_with("elf_") {
		2
	} else {
		1
	}
}

fn is_cfg_test_attrs(attrs: &[String]) -> bool {
	attrs.iter().any(|attr| attr.replace(' ', "").contains("#[cfg(test)]"))
}

fn check_import_rules(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	with_fixes: bool,
) {
	let use_items = ctx
		.top_items
		.iter()
		.filter(|item| item.kind == TopKind::Use && !item.is_pub)
		.collect::<Vec<_>>();

	let mut has_prelude_glob = false;
	for item in &use_items {
		if let Some(line) = ctx.lines.get(item.line.saturating_sub(1)) {
			if let Some(path) = extract_use_path_from_line(line) {
				if path.replace(' ', "") == "crate::prelude::*" {
					has_prelude_glob = true;
				}
			}
		}
	}

	for item in &use_items {
		let Some(line) = ctx.lines.get(item.line.saturating_sub(1)) else {
			continue;
		};
		let Some(path) = extract_use_path_from_line(line) else {
			continue;
		};

		if let Some(alias_caps) =
			Regex::new(r"\bas\s+([A-Za-z_][A-Za-z0-9_]*)\b").unwrap().captures(&path)
		{
			if alias_caps.get(1).map(|m| m.as_str()) != Some("_") {
				push_violation(
					violations,
					ctx,
					item.line,
					"RUST-STYLE-IMPORT-003",
					"Import aliases are not allowed except `as _` in test keep-alive modules.",
					false,
				);
			}
		}

		let compact_path = path.replace(' ', "");
		if has_prelude_glob
			&& compact_path.starts_with("crate::")
			&& compact_path != "crate::prelude::*"
		{
			push_violation(
				violations,
				ctx,
				item.line,
				"RUST-STYLE-IMPORT-007",
				"Avoid redundant crate imports when crate::prelude::* is imported.",
				with_fixes,
			);
			if with_fixes {
				if let (Some(start), Some(next)) = (
					offset_from_line(&ctx.line_starts, item.start_line),
					offset_from_line(&ctx.line_starts, item.end_line + 1),
				) {
					edits.push(Edit {
						start,
						end: next,
						replacement: String::new(),
						rule: "RUST-STYLE-IMPORT-007",
					});
				}
			}
		}

		if path.contains("::") {
			let imported_symbols = imported_symbols_from_use_path(&path);
			for symbol in imported_symbols {
				if symbol.is_empty() || !symbol.chars().next().is_some_and(char::is_lowercase) {
					continue;
				}

				let local_fn_def_re = Regex::new(&format!(
					r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:const\s+)?(?:unsafe\s+)?fn\s+{}\b",
					regex::escape(&symbol)
				))
				.unwrap();
				let local_macro_def_re = Regex::new(&format!(
					r"^\s*(?:macro_rules!\s*{}\b|macro\s+{}\b)",
					regex::escape(&symbol),
					regex::escape(&symbol),
				))
				.unwrap();

				let local_fn_defined = ctx.lines.iter().any(|line| {
					let code = strip_string_and_line_comment(line, false).0;
					local_fn_def_re.is_match(&code)
				});
				let local_macro_defined = ctx.lines.iter().any(|line| {
					let code = strip_string_and_line_comment(line, false).0;
					local_macro_def_re.is_match(&code)
				});
				let called_fn_unqualified =
					contains_unqualified_symbol_call(&ctx.lines, &symbol, false);
				let called_macro_unqualified =
					contains_unqualified_symbol_call(&ctx.lines, &symbol, true);

				if (called_fn_unqualified && !local_fn_defined)
					|| (called_macro_unqualified && !local_macro_defined)
				{
					push_violation(
						violations,
						ctx,
						item.line,
						"RUST-STYLE-IMPORT-004",
						"Do not import free functions or macros into scope; prefer qualified module paths.",
						false,
					);
					break;
				}
			}
		}
	}

	for (prev, curr) in use_items.iter().zip(use_items.iter().skip(1)) {
		let Some(prev_line) = ctx.lines.get(prev.line.saturating_sub(1)) else {
			continue;
		};
		let Some(curr_line) = ctx.lines.get(curr.line.saturating_sub(1)) else {
			continue;
		};
		let Some(prev_path) = extract_use_path_from_line(prev_line) else {
			continue;
		};
		let Some(curr_path) = extract_use_path_from_line(curr_line) else {
			continue;
		};

		let prev_origin = use_origin(&prev_path);
		let curr_origin = use_origin(&curr_path);
		if curr_origin < prev_origin {
			push_violation(
				violations,
				ctx,
				curr.line,
				"RUST-STYLE-IMPORT-001",
				"Import groups must be ordered: std, third-party, self/workspace.",
				false,
			);
		}

		let between = &ctx.lines[prev.line..curr.line.saturating_sub(1)];
		let has_blank = between.iter().any(|line| line.trim().is_empty());
		let has_header_comment = between.iter().any(|line| line.trim_start().starts_with("//"));

		if curr_origin != prev_origin && !has_blank {
			push_violation(
				violations,
				ctx,
				curr.line,
				"RUST-STYLE-IMPORT-002",
				"Separate import groups with one blank line.",
				false,
			);
		}
		if curr_origin == prev_origin && has_blank {
			push_violation(
				violations,
				ctx,
				curr.line,
				"RUST-STYLE-IMPORT-002",
				"Do not place blank lines inside an import group.",
				false,
			);
		}
		if has_header_comment {
			push_violation(
				violations,
				ctx,
				curr.line,
				"RUST-STYLE-IMPORT-002",
				"Do not use header comments for import groups.",
				false,
			);
		}
	}
}

fn order_bucket(kind: TopKind) -> Option<usize> {
	match kind {
		TopKind::Mod => Some(0),
		TopKind::Use => Some(1),
		TopKind::MacroRules => Some(2),
		TopKind::Type => Some(3),
		TopKind::Const => Some(4),
		TopKind::Static => Some(5),
		TopKind::Trait => Some(6),
		TopKind::Enum | TopKind::Struct | TopKind::Impl => Some(8),
		TopKind::Fn => Some(10),
		TopKind::Other => None,
	}
}

fn check_module_order(ctx: &FileContext, violations: &mut Vec<Violation>) {
	let items_for_order = ctx
		.top_items
		.iter()
		.filter(|item| !(item.kind == TopKind::Mod && is_cfg_test_attrs(&item.attrs)))
		.collect::<Vec<_>>();

	let mut order_seen: Vec<usize> = Vec::new();
	for item in &items_for_order {
		let Some(order) = order_bucket(item.kind) else {
			continue;
		};
		if let Some(last) = order_seen.last().copied() {
			if order < last {
				push_violation(
					violations,
					ctx,
					item.line,
					"RUST-STYLE-MOD-001",
					"Top-level module item order does not match rust.md order.",
					false,
				);
			}
		}
		order_seen.push(order);
	}

	let mut non_pub_seen: HashMap<TopKind, bool> = HashMap::new();
	for item in &items_for_order {
		let seen_non_pub = non_pub_seen.get(&item.kind).copied().unwrap_or(false);
		if item.is_pub {
			if seen_non_pub {
				push_violation(
					violations,
					ctx,
					item.line,
					"RUST-STYLE-MOD-002",
					"Place pub items before non-pub items within the same group.",
					false,
				);
			}
		} else {
			non_pub_seen.insert(item.kind, true);
		}
	}

	let mut async_seen = HashMap::new();
	async_seen.insert(true, false);
	async_seen.insert(false, false);
	for item in &items_for_order {
		if item.kind != TopKind::Fn {
			continue;
		}
		let key = item.is_pub;
		if item.is_async {
			async_seen.insert(key, true);
		} else if async_seen.get(&key).copied().unwrap_or(false) {
			push_violation(
				violations,
				ctx,
				item.line,
				"RUST-STYLE-MOD-003",
				"Place non-async functions before async functions at the same visibility.",
				false,
			);
		}
	}

	let mut last_non_test_idx: Option<usize> = None;
	for (idx, item) in ctx.top_items.iter().enumerate() {
		if !(item.kind == TopKind::Mod && is_cfg_test_attrs(&item.attrs)) {
			last_non_test_idx = Some(idx);
		}
	}

	if let Some(last_non_test_idx) = last_non_test_idx {
		for (idx, item) in ctx.top_items.iter().enumerate() {
			if !(item.kind == TopKind::Mod && is_cfg_test_attrs(&item.attrs)) {
				continue;
			}
			if idx < last_non_test_idx {
				push_violation(
					violations,
					ctx,
					item.line,
					"RUST-STYLE-MOD-001",
					"Place #[cfg(test)] modules after all non-test items.",
					false,
				);
			}
		}
	}
}

fn check_cfg_test_mod_tests_use_super(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	with_fixes: bool,
) {
	for item in ctx.source_file.items() {
		let ast::Item::Module(module) = item else {
			continue;
		};
		if !module
			.attrs()
			.any(|attr| attr.syntax().text().to_string().replace(' ', "").contains("cfg(test)"))
		{
			continue;
		}

		let name = module.name().map(|name| name.text().to_string()).unwrap_or_default();
		if name == "_test" {
			continue;
		}
		if name != "tests" {
			continue;
		}
		let Some(item_list) = module.item_list() else {
			continue;
		};

		let mut found_super_use = false;
		for nested in item_list.items() {
			let ast::Item::Use(use_item) = nested else {
				continue;
			};
			if let Some(path) =
				use_item.use_tree().map(|tree| tree.syntax().text().to_string().replace(' ', ""))
			{
				if path == "super::*" {
					found_super_use = true;
					break;
				}
			}
		}

		if !found_super_use {
			let line = line_from_offset(
				&ctx.line_starts,
				usize::from(module.syntax().text_range().start()),
			);
			push_violation(
				violations,
				ctx,
				line,
				"RUST-STYLE-MOD-007",
				"#[cfg(test)] mod tests should include `use super::*;` unless it is a keep-alive module.",
				with_fixes,
			);

			if with_fixes {
				if let Some(open) = item_list.l_curly_token() {
					let insert_at = usize::from(open.text_range().end());
					edits.push(Edit {
						start: insert_at,
						end: insert_at,
						replacement: "\n\t// self\n\tuse super::*;".to_owned(),
						rule: "RUST-STYLE-MOD-007",
					});
				}
			}
		}
	}
}

fn classify_impl_trait_order(raw: &str) -> usize {
	let header = strip_string_and_line_comment(raw, false).0;
	let Some((left, _right)) = header.split_once(" for ") else {
		return 0;
	};
	let mut trait_part =
		left.split_once("impl").map(|(_, right)| right.trim().to_owned()).unwrap_or_default();
	if trait_part.starts_with('<') {
		if let Some((_, after)) = trait_part.split_once('>') {
			trait_part = after.trim().to_owned();
		}
	}
	let trait_name = trait_part.split(['<', ' ', '{']).next().unwrap_or_default().trim();
	if trait_name.starts_with("std::")
		|| trait_name.starts_with("core::")
		|| trait_name.starts_with("alloc::")
	{
		1
	} else if trait_name.starts_with("crate::")
		|| trait_name.starts_with("self::")
		|| trait_name.starts_with("super::")
		|| trait_name.starts_with("elf_")
	{
		3
	} else {
		2
	}
}

fn check_impl_adjacency(ctx: &FileContext, violations: &mut Vec<Violation>) {
	let mut type_indices: HashMap<String, usize> = HashMap::new();
	for (idx, item) in ctx.top_items.iter().enumerate() {
		if !(item.kind == TopKind::Struct || item.kind == TopKind::Enum) {
			continue;
		}
		if let Some(name) = &item.name {
			type_indices.insert(name.clone(), idx);
		}
	}

	let mut impl_by_target: HashMap<String, Vec<usize>> = HashMap::new();
	for (idx, item) in ctx.top_items.iter().enumerate() {
		if item.kind != TopKind::Impl {
			continue;
		}
		if let Some(target) = &item.impl_target {
			impl_by_target.entry(target.clone()).or_default().push(idx);
		}
	}

	for (target, impl_indices) in &impl_by_target {
		let first_impl = impl_indices[0];
		let last_impl = *impl_indices.last().unwrap_or(&first_impl);

		for idx in first_impl..=last_impl {
			let item = &ctx.top_items[idx];
			if item.kind != TopKind::Impl || item.impl_target.as_deref() != Some(target.as_str()) {
				push_violation(
					violations,
					ctx,
					item.line,
					"RUST-STYLE-IMPL-003",
					&format!("impl blocks for `{target}` must be contiguous."),
					false,
				);
				break;
			}
		}

		let mut order_values = Vec::new();
		for idx in impl_indices {
			order_values.push(classify_impl_trait_order(&ctx.top_items[*idx].raw));
		}
		for pos in 1..order_values.len() {
			if order_values[pos] < order_values[pos - 1] {
				push_violation(
					violations,
					ctx,
					ctx.top_items[impl_indices[pos]].line,
					"RUST-STYLE-IMPL-003",
					&format!(
						"impl block order for `{target}` must be inherent, std traits, third-party traits, then workspace-member traits."
					),
					false,
				);
				break;
			}
		}
	}

	for (type_name, type_idx) in type_indices {
		let Some(impl_indices) = impl_by_target.get(&type_name) else {
			continue;
		};
		if impl_indices.is_empty() {
			continue;
		}
		let first_impl = impl_indices[0];
		if first_impl != type_idx + 1 {
			push_violation(
				violations,
				ctx,
				ctx.top_items[first_impl].line,
				"RUST-STYLE-MOD-005",
				&format!("Keep `{type_name}` definitions and related impl blocks adjacent."),
				false,
			);
			continue;
		}

		let type_end = ctx.top_items[type_idx].end_line;
		let impl_start = ctx.top_items[first_impl].start_line;
		if impl_start > type_end + 1 {
			let between = &ctx.lines[type_end..impl_start.saturating_sub(1)];
			if between.iter().any(|line| line.trim().is_empty()) {
				push_violation(
					violations,
					ctx,
					ctx.top_items[first_impl].line,
					"RUST-STYLE-MOD-005",
					&format!(
						"Do not insert blank lines between `{type_name}` and its first impl block."
					),
					false,
				);
			}
		}
	}
}

fn check_impl_rules(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	with_fixes: bool,
) {
	for item in ctx.source_file.items() {
		let ast::Item::Impl(impl_item) = item else {
			continue;
		};
		let Some(self_ty) = impl_item.self_ty() else {
			continue;
		};
		let Some(target) = extract_impl_target_name(&self_ty.syntax().text().to_string()) else {
			continue;
		};

		let qualified_target = format!(
			r"(?:{}\b|(?:crate|self|super)::(?:[A-Za-z_][A-Za-z0-9_]*::)*{}\b)",
			regex::escape(&target),
			regex::escape(&target)
		);
		let return_self_type_re = Regex::new(&format!(r"->\s*{qualified_target}")).unwrap();
		let param_self_type_re = Regex::new(&format!(r":\s*{qualified_target}")).unwrap();

		let Some(items) = impl_item.assoc_item_list() else {
			continue;
		};
		for assoc in items.assoc_items() {
			let ast::AssocItem::Fn(function) = assoc else {
				continue;
			};
			let signature_text = if let Some(body) = function.body() {
				let sig_range = TextRange::new(
					function.syntax().text_range().start(),
					body.syntax().text_range().start(),
				);
				ctx.text[sig_range].to_owned()
			} else {
				function.syntax().text().to_string()
			};

			if return_self_type_re.is_match(&signature_text)
				|| param_self_type_re.is_match(&signature_text)
			{
				let line = line_from_offset(
					&ctx.line_starts,
					usize::from(function.syntax().text_range().start()),
				);
				push_violation(
					violations,
					ctx,
					line,
					"RUST-STYLE-IMPL-001",
					&format!(
						"Use Self instead of concrete type `{target}` in impl method signatures."
					),
					with_fixes,
				);

				if with_fixes {
					let replaced =
						return_self_type_re.replace_all(&signature_text, "-> Self").to_string();
					let replaced = param_self_type_re.replace_all(&replaced, ": Self").to_string();
					let start = usize::from(function.syntax().text_range().start());
					let end = start + signature_text.len();
					edits.push(Edit {
						start,
						end,
						replacement: replaced,
						rule: "RUST-STYLE-IMPL-001",
					});
				}
			}
		}
	}
}

fn check_inline_trait_bounds(ctx: &FileContext, violations: &mut Vec<Violation>) {
	for item in ctx.source_file.syntax().descendants().filter_map(ast::GenericParamList::cast) {
		for param in item.generic_params() {
			if let ast::GenericParam::TypeParam(type_param) = param {
				if type_param.type_bound_list().is_some() {
					let line = line_from_offset(
						&ctx.line_starts,
						usize::from(type_param.syntax().text_range().start()),
					);
					push_violation(
						violations,
						ctx,
						line,
						"RUST-STYLE-GENERICS-001",
						"Inline trait bounds are not allowed. Move bounds into a where clause.",
						false,
					);
				}
			}
		}
	}

	for (idx, line) in ctx.lines.iter().enumerate() {
		let code = strip_string_and_line_comment(line, false).0;
		if INLINE_BOUNDS_RE.is_match(&code) {
			push_violation(
				violations,
				ctx,
				idx + 1,
				"RUST-STYLE-GENERICS-001",
				"Inline trait bounds are not allowed. Move bounds into a where clause.",
				false,
			);
		}
	}
}

fn check_std_macro_calls(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	with_fixes: bool,
) {
	for (idx, line) in ctx.lines.iter().enumerate() {
		let code = strip_string_and_line_comment(line, false).0;
		if !STD_QUALIFIED_MACRO_RE.is_match(&code) {
			continue;
		}
		push_violation(
			violations,
			ctx,
			idx + 1,
			"RUST-STYLE-IMPORT-006",
			"Do not qualify standard macros with std::.",
			with_fixes,
		);

		if with_fixes {
			for caps in STD_QUALIFIED_MACRO_RE.captures_iter(&code) {
				let Some(matched) = caps.get(0) else {
					continue;
				};
				let absolute_line_start = offset_from_line(&ctx.line_starts, idx + 1).unwrap_or(0);
				edits.push(Edit {
					start: absolute_line_start + matched.start(),
					end: absolute_line_start + matched.start() + 5,
					replacement: String::new(),
					rule: "RUST-STYLE-IMPORT-006",
				});
			}
		}
	}
}

fn split_top_level_args(args: &str) -> Vec<String> {
	let mut parts = Vec::new();
	let mut start = 0_usize;
	let mut paren = 0_i32;
	let mut brace = 0_i32;
	let mut bracket = 0_i32;
	let mut in_str = false;
	let mut escape = false;
	let mut in_char = false;
	let mut char_escape = false;
	let mut in_line_comment = false;
	let mut block_comment_depth = 0_i32;
	let chars = args.char_indices().collect::<Vec<_>>();
	let mut idx = 0_usize;

	while idx < chars.len() {
		let (offset, ch) = chars[idx];
		let next = if idx + 1 < chars.len() { Some(chars[idx + 1].1) } else { None };

		if in_line_comment {
			if ch == '\n' {
				in_line_comment = false;
			}
			idx += 1;
			continue;
		}
		if block_comment_depth > 0 {
			if ch == '/' && next == Some('*') {
				block_comment_depth += 1;
				idx += 2;
				continue;
			}
			if ch == '*' && next == Some('/') {
				block_comment_depth -= 1;
				idx += 2;
				continue;
			}
			idx += 1;
			continue;
		}
		if in_str {
			if escape {
				escape = false;
			} else if ch == '\\' {
				escape = true;
			} else if ch == '"' {
				in_str = false;
			}
			idx += 1;
			continue;
		}
		if in_char {
			if char_escape {
				char_escape = false;
			} else if ch == '\\' {
				char_escape = true;
			} else if ch == '\'' {
				in_char = false;
			}
			idx += 1;
			continue;
		}

		if ch == '/' && next == Some('/') {
			in_line_comment = true;
			idx += 2;
			continue;
		}
		if ch == '/' && next == Some('*') {
			block_comment_depth += 1;
			idx += 2;
			continue;
		}
		if ch == '"' {
			in_str = true;
			escape = false;
			idx += 1;
			continue;
		}
		if ch == '\'' {
			in_char = true;
			char_escape = false;
			idx += 1;
			continue;
		}

		match ch {
			'(' => paren += 1,
			')' => paren = (paren - 1).max(0),
			'{' => brace += 1,
			'}' => brace = (brace - 1).max(0),
			'[' => bracket += 1,
			']' => bracket = (bracket - 1).max(0),
			',' if paren == 0 && brace == 0 && bracket == 0 => {
				let segment = args[start..offset].trim();
				if !segment.is_empty() {
					parts.push(segment.to_owned());
				}
				start = offset + 1;
			},
			_ => {},
		}

		idx += 1;
	}

	let tail = args[start..].trim();
	if !tail.is_empty() {
		parts.push(tail.to_owned());
	}
	parts
}

fn parse_string_literal(text: &str) -> Option<String> {
	let stripped = text.trim();
	if stripped.len() >= 2 && stripped.starts_with('"') && stripped.ends_with('"') {
		return Some(stripped[1..stripped.len() - 1].to_owned());
	}

	let raw_re = Regex::new(r#"^r(?P<hashes>#+)?\"(?P<body>[\s\S]*)\"(?P=hashes)?$"#).unwrap();
	raw_re
		.captures(stripped)
		.and_then(|caps| caps.name("body").map(|body| body.as_str().to_owned()))
}

fn is_sentence(text: &str) -> bool {
	let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
	if normalized.is_empty() {
		return false;
	}
	let first = normalized.chars().next().unwrap_or('a');
	let last = normalized.chars().last().unwrap_or('.');
	first.is_uppercase() && matches!(last, '.' | '!' | '?')
}

fn has_structured_fields(text: &str) -> bool {
	Regex::new(r"\b[A-Za-z_][A-Za-z0-9_]*\s*=").unwrap().is_match(text)
		|| Regex::new(r"[%?]\s*[A-Za-z_][A-Za-z0-9_:]*").unwrap().is_match(text)
}

fn macro_path_text(macro_call: &ast::MacroCall) -> Option<String> {
	macro_call.path().map(|path| path.syntax().text().to_string())
}

fn check_logging_quality(ctx: &FileContext, violations: &mut Vec<Violation>) {
	for macro_call in ctx.source_file.syntax().descendants().filter_map(ast::MacroCall::cast) {
		let Some(path_text) = macro_path_text(&macro_call) else {
			continue;
		};
		let normalized = path_text.replace(' ', "");
		if !matches!(
			normalized.as_str(),
			"tracing::trace"
				| "tracing::debug"
				| "tracing::info"
				| "tracing::warn"
				| "tracing::error"
		) {
			continue;
		}
		let Some(tt) = macro_call.token_tree() else {
			continue;
		};
		let tt_text = tt.syntax().text().to_string();
		if tt_text.len() < 2 {
			continue;
		}
		let args = tt_text[1..tt_text.len() - 1].to_owned();
		let parts = split_top_level_args(&args);
		if parts.is_empty() {
			continue;
		}

		let message = parse_string_literal(parts.last().map(String::as_str).unwrap_or_default());
		let head_parts = if message.is_some() {
			parts[..parts.len().saturating_sub(1)].to_vec()
		} else {
			parts.clone()
		};
		let head_text = head_parts.join(", ");
		let line = line_from_offset(
			&ctx.line_starts,
			usize::from(macro_call.syntax().text_range().start()),
		);

		if let Some(message) = message {
			if message.contains('{') || message.contains('}') {
				push_violation(
					violations,
					ctx,
					line,
					"RUST-STYLE-LOG-002",
					"Do not interpolate dynamic values in log message strings; use structured fields.",
					false,
				);
			}
			if !is_sentence(&message) {
				push_violation(
					violations,
					ctx,
					line,
					"RUST-STYLE-LOG-002",
					"Log messages should be complete sentences with capitalization and punctuation.",
					false,
				);
			}
		}

		if parts.len() > 1 && !has_structured_fields(&head_text) {
			push_violation(
				violations,
				ctx,
				line,
				"RUST-STYLE-LOG-002",
				"Prefer structured logging fields for dynamic context values.",
				false,
			);
		}
	}
}

fn is_test_file(path: &Path) -> bool {
	let text = path.to_string_lossy().replace('\\', "/");
	text.contains("/tests/") || text.ends_with("_test.rs")
}

fn check_expect_unwrap(ctx: &FileContext, violations: &mut Vec<Violation>) {
	if is_test_file(&ctx.path) {
		return;
	}

	for method_call in ctx.source_file.syntax().descendants().filter_map(ast::MethodCallExpr::cast)
	{
		let Some(name) = method_call.name_ref().map(|name| name.text().to_string()) else {
			continue;
		};
		let line = line_from_offset(
			&ctx.line_starts,
			usize::from(method_call.syntax().text_range().start()),
		);

		if name == "unwrap" {
			push_violation(
				violations,
				ctx,
				line,
				"RUST-STYLE-RUNTIME-001",
				"Do not use unwrap() in non-test code.",
				false,
			);
			continue;
		}

		if name != "expect" {
			continue;
		}
		let Some(arg_list) = method_call.arg_list() else {
			push_violation(
				violations,
				ctx,
				line,
				"RUST-STYLE-RUNTIME-002",
				"expect() must use a clear, user-actionable string literal message.",
				false,
			);
			continue;
		};
		let mut args = arg_list.args();
		let Some(first_arg) = args.next() else {
			push_violation(
				violations,
				ctx,
				line,
				"RUST-STYLE-RUNTIME-002",
				"expect() message must not be empty.",
				false,
			);
			continue;
		};
		let literal = first_arg
			.syntax()
			.descendants()
			.filter_map(ast::Literal::cast)
			.next()
			.and_then(|lit| parse_string_literal(&lit.syntax().text().to_string()));
		let Some(message) = literal else {
			push_violation(
				violations,
				ctx,
				line,
				"RUST-STYLE-RUNTIME-002",
				"expect() must use a clear, user-actionable string literal message.",
				false,
			);
			continue;
		};
		let message = message.trim().to_owned();
		if message.is_empty() {
			push_violation(
				violations,
				ctx,
				line,
				"RUST-STYLE-RUNTIME-002",
				"expect() message must not be empty.",
				false,
			);
			continue;
		}
		let first = message.chars().next().unwrap_or('a');
		let last = message.chars().last().unwrap_or('.');
		if !first.is_uppercase() || !matches!(last, '.' | '!' | '?') {
			push_violation(
				violations,
				ctx,
				line,
				"RUST-STYLE-RUNTIME-002",
				"expect() message should start with a capital letter and end with punctuation.",
				false,
			);
		}
	}
}

fn add_numeric_grouping(number: &str) -> String {
	let mut rev = String::new();
	for (idx, ch) in number.chars().rev().enumerate() {
		if idx > 0 && idx % 3 == 0 {
			rev.push('_');
		}
		rev.push(ch);
	}
	rev.chars().rev().collect()
}

fn suffix_boundary(literal: &str) -> Option<usize> {
	for (idx, ch) in literal.char_indices() {
		if !(ch.is_ascii_digit() || ch == '.') {
			return Some(idx);
		}
	}
	None
}

fn check_numeric_literals(
	ctx: &FileContext,
	violations: &mut Vec<Violation>,
	edits: &mut Vec<Edit>,
	with_fixes: bool,
) {
	for (idx, line) in ctx.lines.iter().enumerate() {
		let code = strip_string_and_line_comment(line, false).0;

		for matched in NUM_SUFFIX_RE.find_iter(&code) {
			if matched.start() == 0 {
				continue;
			}
			if code.as_bytes()[matched.start() - 1] != b'_' {
				push_violation(
					violations,
					ctx,
					idx + 1,
					"RUST-STYLE-NUM-001",
					"Numeric suffixes must be separated by an underscore (for example 10_f32).",
					with_fixes,
				);
				if with_fixes {
					let absolute_line_start =
						offset_from_line(&ctx.line_starts, idx + 1).unwrap_or(0);
					let boundary = suffix_boundary(matched.as_str()).unwrap_or(0);
					edits.push(Edit {
						start: absolute_line_start + matched.start() + boundary,
						end: absolute_line_start + matched.start() + boundary,
						replacement: "_".to_owned(),
						rule: "RUST-STYLE-NUM-001",
					});
				}
				break;
			}
		}

		for matched in PLAIN_INT_RE.find_iter(&code) {
			let number = matched.as_str();
			if number.contains('_') {
				continue;
			}
			push_violation(
				violations,
				ctx,
				idx + 1,
				"RUST-STYLE-NUM-002",
				"Integers with more than three digits must use underscore separators.",
				with_fixes,
			);
			if with_fixes {
				let absolute_line_start = offset_from_line(&ctx.line_starts, idx + 1).unwrap_or(0);
				edits.push(Edit {
					start: absolute_line_start + matched.start(),
					end: absolute_line_start + matched.end(),
					replacement: add_numeric_grouping(number),
					rule: "RUST-STYLE-NUM-002",
				});
			}
			break;
		}
	}
}

fn function_ranges(ctx: &FileContext) -> Vec<(usize, usize)> {
	let mut ranges = Vec::new();
	for function in ctx.source_file.syntax().descendants().filter_map(ast::Fn::cast) {
		let Some(body) = function.body() else {
			continue;
		};
		let (start_line, end_line) =
			text_range_to_lines(&ctx.line_starts, body.syntax().text_range());
		ranges.push((start_line.saturating_sub(1), end_line.saturating_sub(1)));
	}
	ranges
}

fn check_function_length(ctx: &FileContext, violations: &mut Vec<Violation>) {
	for (start, end) in function_ranges(ctx) {
		if end < start {
			continue;
		}
		let length = end - start + 1;
		if length > 120 {
			push_violation(
				violations,
				ctx,
				start + 1,
				"RUST-STYLE-READ-002",
				&format!("Function body has {length} lines; keep functions at or under 120 lines."),
				false,
			);
		}
	}
}

fn check_test_rules(ctx: &FileContext, violations: &mut Vec<Violation>) {
	for function in ctx.source_file.syntax().descendants().filter_map(ast::Fn::cast) {
		let is_test = function
			.attrs()
			.any(|attr| attr.as_simple_atom().map(|atom| atom.as_str() == "test").unwrap_or(false));
		if !is_test {
			continue;
		}

		let name = function.name().map(|name| name.text().to_string()).unwrap_or_default();
		if !SNAKE_CASE_RE.is_match(&name) || !name.contains('_') {
			let line = line_from_offset(
				&ctx.line_starts,
				usize::from(function.syntax().text_range().start()),
			);
			push_violation(
				violations,
				ctx,
				line,
				"RUST-STYLE-TEST-001",
				"Test function names should be descriptive snake_case.",
				false,
			);
		}
	}

	for item in ctx.source_file.items() {
		let ast::Item::Module(module) = item else {
			continue;
		};
		let Some(name) = module.name().map(|name| name.text().to_string()) else {
			continue;
		};
		if name != "_test" {
			continue;
		}
		if !module
			.attrs()
			.any(|attr| attr.syntax().text().to_string().replace(' ', "").contains("cfg(test)"))
		{
			continue;
		}
		let contains_behavior_tests = module.item_list().is_some_and(|list| {
			list.items().any(|item| {
				if let ast::Item::Fn(function) = item {
					function.attrs().any(|attr| {
						attr.as_simple_atom().map(|atom| atom.as_str() == "test").unwrap_or(false)
					})
				} else {
					false
				}
			})
		});
		if contains_behavior_tests {
			push_violation(
				violations,
				ctx,
				1,
				"RUST-STYLE-TEST-002",
				"`#[cfg(test)] mod _test` is reserved for keep-alive imports and must not contain behavior tests.",
				false,
			);
		}
	}
}

fn strip_string_and_line_comment(line: &str, mut in_str: bool) -> (String, bool) {
	let mut out = String::with_capacity(line.len());
	let mut escape = false;
	let mut idx = 0;
	let chars = line.chars().collect::<Vec<_>>();

	while idx < chars.len() {
		let ch = chars[idx];
		let next = chars.get(idx + 1).copied();

		if in_str {
			if escape {
				escape = false;
			} else if ch == '\\' {
				escape = true;
			} else if ch == '"' {
				in_str = false;
			}
			out.push(' ');
			idx += 1;
			continue;
		}

		if ch == '"' {
			in_str = true;
			out.push(' ');
			idx += 1;
			continue;
		}

		if ch == '/' && next == Some('/') {
			break;
		}

		out.push(ch);
		idx += 1;
	}

	(out, in_str)
}

fn normalize_statement_text(statement_lines: &[String]) -> String {
	let mut parts = Vec::new();
	let mut in_str = false;
	for raw in statement_lines {
		let (mut code, next_state) = strip_string_and_line_comment(raw, in_str);
		in_str = next_state;
		code = code.trim().to_owned();
		if code.is_empty() || code.starts_with('#') {
			continue;
		}
		parts.push(code);
	}
	parts.join(" ")
}

fn strip_turbofish(text: &str) -> String {
	let mut out = String::with_capacity(text.len());
	let mut idx = 0;
	let chars = text.chars().collect::<Vec<_>>();

	while idx < chars.len() {
		if idx + 2 < chars.len()
			&& chars[idx] == ':'
			&& chars[idx + 1] == ':'
			&& chars[idx + 2] == '<'
		{
			idx += 3;
			let mut depth = 1_i32;
			while idx < chars.len() && depth > 0 {
				if chars[idx] == '<' {
					depth += 1;
				} else if chars[idx] == '>' {
					depth -= 1;
				}
				idx += 1;
			}
			continue;
		}
		out.push(chars[idx]);
		idx += 1;
	}

	out
}

fn parse_ufcs_target_call(text: &str) -> Option<(String, String)> {
	if !text.starts_with('<') {
		return None;
	}
	let chars = text.chars().collect::<Vec<_>>();
	let mut depth = 0_i32;
	let mut close_idx = None;
	for (idx, ch) in chars.iter().enumerate() {
		if *ch == '<' {
			depth += 1;
		} else if *ch == '>' {
			depth -= 1;
			if depth == 0 {
				close_idx = Some(idx);
				break;
			}
		}
	}
	let close_idx = close_idx?;
	let body = text[1..close_idx].trim();
	let mut rest = text[close_idx + 1..].trim_start();
	if !rest.starts_with("::") {
		return None;
	}
	rest = &rest[2..];
	let fn_match = Regex::new(r"^(?P<func>[A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap().captures(rest)?;
	let func = fn_match.name("func")?.as_str().to_owned();
	let target = if let Some((_, right)) = body.split_once(" as ") {
		right.trim().to_owned()
	} else {
		body.to_owned()
	};
	if target.is_empty() { None } else { Some((target, func)) }
}

fn contains_assignment_operator(text: &str) -> bool {
	for op in ["+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "<<=", ">>="] {
		if text.contains(op) {
			return true;
		}
	}

	let bytes = text.as_bytes();
	for idx in 0..bytes.len() {
		if bytes[idx] != b'=' {
			continue;
		}
		let prev = if idx > 0 { Some(bytes[idx - 1] as char) } else { None };
		let next = if idx + 1 < bytes.len() { Some(bytes[idx + 1] as char) } else { None };
		if prev == Some('=') || prev == Some('!') || prev == Some('<') || prev == Some('>') {
			continue;
		}
		if next == Some('=') || next == Some('>') {
			continue;
		}
		return true;
	}

	false
}

fn classify_statement_type(statement_lines: &[String]) -> String {
	let mut normalized = normalize_statement_text(statement_lines);
	if normalized.is_empty() {
		return "empty".to_owned();
	}
	normalized = strip_turbofish(&normalized);
	let first = normalized.as_str();

	if Regex::new(r"^let\b").unwrap().is_match(first) {
		return "let".to_owned();
	}
	if Regex::new(r"^if\s+let\b").unwrap().is_match(first) {
		return "if-let".to_owned();
	}
	if Regex::new(r"^if\b").unwrap().is_match(first) {
		return "if".to_owned();
	}
	if Regex::new(r"^match\b").unwrap().is_match(first) {
		return "match".to_owned();
	}
	if Regex::new(r"^for\b").unwrap().is_match(first) {
		return "for".to_owned();
	}
	if Regex::new(r"^while\b").unwrap().is_match(first) {
		return "while".to_owned();
	}
	if Regex::new(r"^loop\b").unwrap().is_match(first) {
		return "loop".to_owned();
	}
	if Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*(?:\.await)?\?\s*;?$")
		.unwrap()
		.is_match(first)
	{
		return "try-expr".to_owned();
	}
	if contains_assignment_operator(first) {
		return "assign".to_owned();
	}
	if Regex::new(r"^(?P<name>[A-Za-z_][A-Za-z0-9_:]*)!\s*\(").unwrap().is_match(first) {
		let macro_name = Regex::new(r"^(?P<name>[A-Za-z_][A-Za-z0-9_:]*)!\s*\(")
			.unwrap()
			.captures(first)
			.and_then(|caps| caps.name("name"))
			.map(|value| value.as_str().to_owned())
			.unwrap_or_default();
		if macro_name.contains("::") {
			return "macro-path".to_owned();
		}
		return "macro".to_owned();
	}
	if parse_ufcs_target_call(first).is_some() {
		return "path-call".to_owned();
	}
	if Regex::new(r"^(?P<target>[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)+)\s*\(")
		.unwrap()
		.is_match(first)
	{
		return "path-call".to_owned();
	}
	if Regex::new(r"^(?P<target>[A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap().is_match(first) {
		return "call".to_owned();
	}
	if Regex::new(r"^[^;]*\.(?P<method>[A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap().is_match(first) {
		return "method".to_owned();
	}

	let token = Regex::new(r"[\s({;]").unwrap().split(first).next().unwrap_or_default();
	if token.is_empty() { "other".to_owned() } else { format!("shape:{token}") }
}

fn extract_top_level_statements(
	lines: &[String],
	fn_start: usize,
	fn_end: usize,
) -> Vec<(usize, usize, String)> {
	let mut statements = Vec::new();
	let mut brace_depth = 1_i32;
	let mut paren_depth = 0_i32;
	let mut bracket_depth = 0_i32;
	let mut current_start: Option<usize> = None;

	for idx in (fn_start + 1)..fn_end {
		let raw_line = &lines[idx];
		let stripped = raw_line.trim();
		let code = strip_string_and_line_comment(raw_line, false).0;

		if current_start.is_none()
			&& brace_depth == 1
			&& !stripped.is_empty()
			&& !stripped.starts_with("//")
			&& !stripped.starts_with('#')
			&& stripped != "}"
		{
			current_start = Some(idx);
		}

		for ch in code.chars() {
			match ch {
				'(' => paren_depth += 1,
				')' => paren_depth = (paren_depth - 1).max(0),
				'[' => bracket_depth += 1,
				']' => bracket_depth = (bracket_depth - 1).max(0),
				'{' => brace_depth += 1,
				'}' => brace_depth = (brace_depth - 1).max(0),
				_ => {},
			}
		}

		let Some(current_start_value) = current_start else {
			continue;
		};

		let stripped_code = code.trim();
		let statement_closed = brace_depth == 1
			&& paren_depth == 0
			&& bracket_depth == 0
			&& !stripped_code.is_empty()
			&& (stripped_code.ends_with(';') || stripped_code.ends_with('}'));

		if statement_closed {
			let span_lines = lines[current_start_value..=idx].to_vec();
			statements.push((current_start_value, idx, classify_statement_type(&span_lines)));
			current_start = None;
		}
	}

	if let Some(current_start) = current_start {
		if fn_end > current_start {
			let span_lines = lines[current_start..fn_end].to_vec();
			statements.push((
				current_start,
				fn_end.saturating_sub(1),
				classify_statement_type(&span_lines),
			));
		}
	}

	statements
}

fn first_significant_statement_line(lines: &[String]) -> Option<String> {
	for line in lines {
		let stripped = line.trim();
		if stripped.is_empty() || stripped.starts_with("//") || stripped.starts_with('#') {
			continue;
		}
		return Some(stripped.to_owned());
	}
	None
}

fn last_significant_statement_line(lines: &[String]) -> Option<String> {
	for line in lines.iter().rev() {
		let stripped = line.trim();
		if stripped.is_empty() || stripped.starts_with("//") || stripped.starts_with('#') {
			continue;
		}
		return Some(stripped.to_owned());
	}
	None
}

fn is_return_or_tail_statement(statement_lines: &[String]) -> bool {
	let Some(first) = first_significant_statement_line(statement_lines) else {
		return false;
	};
	if Regex::new(r"^return\b").unwrap().is_match(&first) {
		return true;
	}
	let Some(last) = last_significant_statement_line(statement_lines) else {
		return false;
	};
	if Regex::new(r"^return\b").unwrap().is_match(&last) {
		return true;
	}
	if last.ends_with(';') || last.ends_with('{') || matches!(last.as_str(), "}" | "};") {
		return false;
	}
	true
}

fn is_explicit_return_statement(statement_lines: &[String]) -> bool {
	first_significant_statement_line(statement_lines)
		.map(|first| Regex::new(r"^return\b").unwrap().is_match(&first))
		.unwrap_or(false)
}

fn extract_top_level_brace_blocks_in_span(
	lines: &[String],
	span_start: usize,
	span_end: usize,
) -> Vec<(usize, usize)> {
	let mut blocks = Vec::new();
	let mut depth = 0_i32;
	let mut current_start: Option<usize> = None;

	for idx in span_start..=span_end {
		let code = strip_string_and_line_comment(&lines[idx], false).0;
		for ch in code.chars() {
			if ch == '{' {
				depth += 1;
				if depth == 1 {
					current_start = Some(idx);
				}
			} else if ch == '}' {
				if depth == 1 {
					if let Some(start) = current_start {
						blocks.push((start, idx));
						current_start = None;
					}
				}
				depth = (depth - 1).max(0);
			}
		}
	}

	blocks
}

fn is_data_like_brace_block(lines: &[String], block_start: usize, block_end: usize) -> bool {
	let mut content = Vec::new();
	for line in lines.iter().take(block_end).skip(block_start + 1) {
		let code = strip_string_and_line_comment(line, false).0;
		let code = code.trim().to_owned();
		if code.is_empty() || code.starts_with('#') {
			continue;
		}
		content.push(code);
	}

	if content.is_empty() {
		return true;
	}

	for line in &content {
		if line.contains("=>") || line.contains(';') {
			return false;
		}
		if Regex::new(r"^(if|if\s+let|match|for|while|loop|return|let)\b").unwrap().is_match(line) {
			return false;
		}
	}

	for line in &content {
		if Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*\s*:\s*.+,?$").unwrap().is_match(line) {
			continue;
		}
		if line.ends_with(',') {
			continue;
		}
		return false;
	}

	true
}

fn check_vertical_spacing(ctx: &FileContext, violations: &mut Vec<Violation>) {
	let mut visited_blocks: HashSet<(usize, usize)> = HashSet::new();

	fn check_block(
		ctx: &FileContext,
		violations: &mut Vec<Violation>,
		visited_blocks: &mut HashSet<(usize, usize)>,
		start: usize,
		end: usize,
	) {
		if end <= start {
			return;
		}
		if !visited_blocks.insert((start, end)) {
			return;
		}

		let statements = extract_top_level_statements(&ctx.lines, start, end);
		if statements.is_empty() {
			return;
		}

		let (last_start, last_end, _) = statements[statements.len() - 1].clone();
		let final_is_return_or_tail =
			is_return_or_tail_statement(&ctx.lines[last_start..=last_end]);

		let mut return_like_indices: BTreeSet<usize> = BTreeSet::new();
		for (idx, (stmt_start, stmt_end, _)) in statements.iter().enumerate() {
			if is_explicit_return_statement(&ctx.lines[*stmt_start..=*stmt_end]) {
				return_like_indices.insert(idx);
			}
		}
		if final_is_return_or_tail {
			return_like_indices.insert(statements.len() - 1);
		}

		for idx in 0..statements.len().saturating_sub(1) {
			let (_curr_start, curr_end, curr_type) = &statements[idx];
			let (next_start, _next_end, next_type) = &statements[idx + 1];

			if return_like_indices.contains(&(idx + 1)) {
				continue;
			}

			let between = &ctx.lines[curr_end + 1..*next_start];
			let blank_count = between.iter().filter(|line| line.trim().is_empty()).count();

			if curr_type == next_type {
				if blank_count != 0 {
					push_violation(
						violations,
						ctx,
						next_start + 1,
						"RUST-STYLE-SPACE-003",
						"Do not insert blank lines within the same statement type.",
						false,
					);
				}
			} else if blank_count != 1 {
				push_violation(
					violations,
					ctx,
					next_start + 1,
					"RUST-STYLE-SPACE-003",
					"Insert exactly one blank line between different statement types.",
					false,
				);
			}
		}

		for idx in return_like_indices {
			if idx == 0 {
				continue;
			}
			let (_prev_start, prev_end, _) = &statements[idx - 1];
			let (ret_start, ret_end, _) = &statements[idx];
			let between = &ctx.lines[prev_end + 1..*ret_start];
			let blank_count = between.iter().filter(|line| line.trim().is_empty()).count();
			if blank_count != 1 {
				let stmt_lines = &ctx.lines[*ret_start..=*ret_end];
				let message = if is_explicit_return_statement(stmt_lines) {
					"Insert exactly one blank line before each return statement."
				} else {
					"Insert exactly one blank line before the final tail expression."
				};
				push_violation(
					violations,
					ctx,
					ret_start + 1,
					"RUST-STYLE-SPACE-004",
					message,
					false,
				);
			}
		}

		for (stmt_start, stmt_end, _) in statements {
			for (child_start, child_end) in
				extract_top_level_brace_blocks_in_span(&ctx.lines, stmt_start, stmt_end)
			{
				if child_start == start && child_end == end {
					continue;
				}
				if is_data_like_brace_block(&ctx.lines, child_start, child_end) {
					continue;
				}
				check_block(ctx, violations, visited_blocks, child_start, child_end);
			}
		}
	}

	for (start, end) in function_ranges(ctx) {
		check_block(ctx, violations, &mut visited_blocks, start, end);
	}
}

fn apply_edits(text: &mut String, mut edits: Vec<Edit>) -> Result<usize> {
	if edits.is_empty() {
		return Ok(0);
	}

	edits.sort_by(|a, b| a.start.cmp(&b.start).then(a.end.cmp(&b.end)).then(a.rule.cmp(b.rule)));
	let mut filtered = Vec::new();
	let mut last_end = 0_usize;
	for edit in edits {
		if edit.start < last_end {
			continue;
		}
		last_end = edit.end;
		filtered.push(edit);
	}

	if filtered.is_empty() {
		return Ok(0);
	}

	for edit in filtered.iter().rev() {
		if edit.end > text.len() || edit.start > edit.end {
			return Err(eyre::eyre!(
				"Invalid edit range {}..{} for text length {}.",
				edit.start,
				edit.end,
				text.len()
			));
		}
		text.replace_range(edit.start..edit.end, &edit.replacement);
	}

	Ok(filtered.len())
}

#[cfg(test)]
mod tests {
	// std
	use std::{
		collections::BTreeSet,
		fs,
		path::{Path, PathBuf},
		process::Command,
		time::{SystemTime, UNIX_EPOCH},
	};

	// self
	use super::*;

	#[test]
	fn suffix_rewrite_works() {
		let text = "let x = 10f32;\n";
		let ctx = read_file_context_from_text(Path::new("a.rs"), text.to_owned())
			.expect("context")
			.expect("has ctx");
		let (violations, edits) = collect_violations(&ctx, true);
		assert!(!violations.is_empty());
		assert!(edits.iter().any(|edit| edit.rule == "RUST-STYLE-NUM-001"));
	}

	#[test]
	fn detects_cfg_test_super_use() {
		let text = "#[cfg(test)]\nmod tests {\n\t#[test]\n\tfn sample_case() {}\n}\n";
		let ctx = read_file_context_from_text(Path::new("b.rs"), text.to_owned())
			.expect("context")
			.expect("has ctx");
		let (violations, _) = collect_violations(&ctx, false);
		assert!(violations.iter().any(|violation| violation.rule == "RUST-STYLE-MOD-007"));
	}

	#[test]
	fn ab_rules_match_python_for_common_fixture() {
		let fixture = r#"fn main() {
	let x = 10000;
	let y = 10f32;
	std::println!("{x} {y}");
	let _z = Some(1).unwrap();
	tracing::info!("User {x} logged in.");
}
"#;

		let file = write_fixture("ab-common.rs", fixture);
		let python_rules = python_rules_for_file(&file);
		let rust_rules = rust_rules_for_file(&file);
		assert_eq!(python_rules, rust_rules);
	}

	fn rust_rules_for_file(file: &Path) -> BTreeSet<String> {
		let summary = run_check(&[file.to_path_buf()]).expect("run rust check");
		parse_rule_ids(summary.output_lines.into_iter())
	}

	fn python_rules_for_file(file: &Path) -> BTreeSet<String> {
		let script_path =
			Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts").join("rust-style-check.py");
		let output = Command::new("python3")
			.arg(script_path)
			.arg("--check")
			.arg(file)
			.output()
			.expect("run python checker");
		let stdout = String::from_utf8(output.stdout).expect("python stdout");
		let stderr = String::from_utf8(output.stderr).expect("python stderr");
		parse_rule_ids(stdout.lines().chain(stderr.lines()).map(ToOwned::to_owned))
	}

	fn parse_rule_ids(lines: impl Iterator<Item = String>) -> BTreeSet<String> {
		let mut rules = BTreeSet::new();
		let re = Regex::new(r"\[(RUST-STYLE-[A-Z0-9-]+)\]").expect("rule regex");
		for line in lines {
			if let Some(caps) = re.captures(&line) {
				if let Some(matched) = caps.get(1) {
					rules.insert(matched.as_str().to_owned());
				}
			}
		}
		rules
	}

	fn write_fixture(name: &str, content: &str) -> PathBuf {
		let unique = SystemTime::now().duration_since(UNIX_EPOCH).expect("duration").as_nanos();
		let dir = std::env::temp_dir().join(format!("vibe-style-{unique}"));
		fs::create_dir_all(&dir).expect("create dir");
		let path = dir.join(name);
		fs::write(&path, content).expect("write fixture");
		path
	}
}
