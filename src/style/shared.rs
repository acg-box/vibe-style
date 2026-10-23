use std::{
	collections::HashSet,
	fs,
	path::{Path, PathBuf},
	sync::{LazyLock, OnceLock},
};

use color_eyre::Result;
use ra_ap_syntax::{
	AstNode, Edition, SourceFile, TextRange,
	ast::{self, HasAttrs, HasModuleItem, HasName, HasVisibility},
};
use regex::Regex;

pub(crate) const STYLE_RULE_IDS: [&str; 42] = [
	"RUST-STYLE-FILE-001",
	"RUST-STYLE-MOD-001",
	"RUST-STYLE-MOD-002",
	"RUST-STYLE-MOD-003",
	"RUST-STYLE-MOD-004",
	"RUST-STYLE-MOD-005",
	"RUST-STYLE-MOD-007",
	"RUST-STYLE-IMPORT-001",
	"RUST-STYLE-IMPORT-002",
	"RUST-STYLE-IMPORT-003",
	"RUST-STYLE-IMPORT-004",
	"RUST-STYLE-IMPORT-005",
	"RUST-STYLE-IMPORT-006",
	"RUST-STYLE-IMPORT-007",
	"RUST-STYLE-IMPORT-008",
	"RUST-STYLE-IMPORT-009",
	"RUST-STYLE-IMPORT-010",
	"RUST-STYLE-IMPORT-011",
	"RUST-STYLE-IMPORT-012",
	"RUST-STYLE-IMPL-001",
	"RUST-STYLE-IMPL-003",
	"RUST-STYLE-GENERICS-001",
	"RUST-STYLE-GENERICS-002",
	"RUST-STYLE-GENERICS-003",
	"RUST-STYLE-TYPE-001",
	"RUST-STYLE-LET-001",
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
	"SWIFT-STYLE-FILE-001",
	"SWIFT-STYLE-IMPORT-004",
	"SWIFT-STYLE-TYPE-001",
	"SWIFT-STYLE-RUNTIME-001",
	"SWIFT-STYLE-NUM-002",
	"SWIFT-STYLE-READ-002",
];

pub(crate) static SNAKE_CASE_RE: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"^[a-z][a-z0-9_]*$").expect("Compile snake_case validation regex.")
});

#[derive(Clone, Debug)]
pub(crate) struct Violation {
	pub(crate) file: PathBuf,
	pub(crate) line: usize,
	pub(crate) rule: &'static str,
	pub(crate) message: String,
	pub(crate) fixable: bool,
}
impl Violation {
	pub(crate) fn format(&self) -> String {
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

#[derive(Clone, Debug)]
pub(crate) struct Edit {
	pub(crate) start: usize,
	pub(crate) end: usize,
	pub(crate) replacement: String,
	pub(crate) rule: &'static str,
}

#[derive(Clone, Debug)]
pub(crate) struct RunSummary {
	pub(crate) file_count: usize,
	pub(crate) violation_count: usize,
	pub(crate) unfixable_count: usize,
	pub(crate) applied_fix_count: usize,
	pub(crate) output_lines: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct CargoOptions {
	pub(crate) language: StyleLanguage,
	pub(crate) workspace: bool,
	pub(crate) packages: Vec<String>,
	pub(crate) features: Vec<String>,
	pub(crate) all_features: bool,
	pub(crate) no_default_features: bool,
}
impl CargoOptions {
	#[cfg(test)]
	pub(crate) fn new(language: StyleLanguage) -> Self {
		Self {
			language,
			workspace: false,
			packages: Vec::new(),
			features: Vec::new(),
			all_features: false,
			no_default_features: false,
		}
	}
}

#[derive(Clone, Debug)]
pub(crate) struct TopItem {
	pub(crate) kind: TopKind,
	pub(crate) name: Option<String>,
	pub(crate) line: usize,
	pub(crate) start_line: usize,
	pub(crate) end_line: usize,
	pub(crate) start_offset: usize,
	pub(crate) end_offset: usize,
	pub(crate) is_pub: bool,
	pub(crate) visibility: String,
	pub(crate) is_async: bool,
	pub(crate) attrs: Vec<String>,
	pub(crate) impl_target: Option<String>,
	/// Precomputed `use` tree text for top-level `use` items, with all whitespace removed.
	pub(crate) use_path: Option<String>,
	pub(crate) raw: String,
}

#[derive(Debug)]
pub(crate) struct FileContext {
	pub(crate) path: PathBuf,
	pub(crate) text: String,
	pub(crate) lines: Vec<String>,
	pub(crate) line_starts: Vec<usize>,
	pub(crate) source_file: SourceFile,
	pub(crate) top_items: Vec<TopItem>,
	pub(crate) declared_module_path: OnceLock<Option<Vec<String>>>,
	pub(crate) external_module_references: OnceLock<Option<HashSet<String>>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StyleLanguage {
	Rust,
	Swift,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum TopKind {
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

pub(crate) fn push_violation(
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

pub(crate) fn read_file_context(path: &Path) -> Result<Option<FileContext>> {
	let text = match fs::read_to_string(path) {
		Ok(text) => text,
		Err(_) => return Ok(None),
	};

	read_file_context_from_text(path, text)
}

pub(crate) fn read_file_context_from_text(
	path: &Path,
	text: String,
) -> Result<Option<FileContext>> {
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
		declared_module_path: OnceLock::new(),
		external_module_references: OnceLock::new(),
	}))
}

pub(crate) fn line_from_offset(line_starts: &[usize], offset: usize) -> usize {
	match line_starts.binary_search(&offset) {
		Ok(pos) => pos + 1,
		Err(pos) => pos,
	}
}

pub(crate) fn offset_from_line(line_starts: &[usize], line_one_based: usize) -> Option<usize> {
	if line_one_based == 0 {
		return None;
	}

	line_starts.get(line_one_based - 1).copied()
}

pub(crate) fn text_range_to_lines(line_starts: &[usize], range: TextRange) -> (usize, usize) {
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

pub(crate) fn extract_impl_target_name(ty_text: &str) -> Option<String> {
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

pub(crate) fn strip_string_and_line_comment(line: &str, mut in_str: bool) -> (String, bool) {
	let chars = line.chars().collect::<Vec<_>>();
	let mut out = String::with_capacity(line.len());
	let mut escape = false;
	let mut idx = 0;

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

pub(crate) fn normalize_path(path: &Path) -> PathBuf {
	match fs::canonicalize(path) {
		Ok(canonical) => canonical,
		Err(_) => path.to_path_buf(),
	}
}

pub(crate) fn classify_top_kind(item: &ra_ap_syntax::ast::Item) -> TopKind {
	match item {
		ast::Item::Module(_) => TopKind::Mod,
		ast::Item::Use(_) => TopKind::Use,
		ast::Item::MacroRules(_) => TopKind::MacroRules,
		ast::Item::MacroCall(_) => TopKind::MacroRules,
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

pub(crate) fn item_visibility_key(item: &ra_ap_syntax::ast::Item) -> String {
	item_visibility_text(item)
		.map(|text| text.chars().filter(|ch| !ch.is_whitespace()).collect::<String>())
		.unwrap_or_default()
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

fn collect_top_items(source_file: &SourceFile, line_starts: &[usize]) -> Vec<TopItem> {
	let mut items = Vec::new();

	for item in source_file.items() {
		let kind = classify_top_kind(&item);
		let name = item_name(&item);
		let is_pub = item_visibility_is_pub(&item);
		let visibility = item_visibility_key(&item);
		let is_async = matches!(&item, ast::Item::Fn(func) if func.async_token().is_some());
		let attrs = item.attrs().map(|attr| attr.syntax().text().to_string()).collect::<Vec<_>>();
		let impl_target = if let ast::Item::Impl(impl_item) = &item {
			impl_item
				.self_ty()
				.and_then(|ty| extract_impl_target_name(&ty.syntax().text().to_string()))
		} else {
			None
		};
		let use_path = if let ast::Item::Use(use_item) = &item {
			use_item.use_tree().map(|tree| tree.syntax().text().to_string())
		} else {
			None
		};
		let raw = item.syntax().text().to_string();
		let text_range = item.syntax().text_range();
		let (start_line, end_line) = text_range_to_lines(line_starts, text_range);

		items.push(TopItem {
			kind,
			name,
			line: start_line,
			start_line,
			end_line,
			start_offset: usize::from(text_range.start()),
			end_offset: usize::from(text_range.end()),
			is_pub,
			visibility,
			is_async,
			attrs,
			impl_target,
			use_path,
			raw,
		});
	}

	items
}

fn item_name(item: &ra_ap_syntax::ast::Item) -> Option<String> {
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

fn item_visibility_is_pub(item: &ra_ap_syntax::ast::Item) -> bool {
	item_visibility_text(item).is_some()
}

fn item_visibility_text(item: &ra_ap_syntax::ast::Item) -> Option<String> {
	match item {
		ast::Item::Module(node) =>
			node.visibility().map(|visibility| visibility.syntax().text().to_string()),
		ast::Item::Use(node) =>
			node.visibility().map(|visibility| visibility.syntax().text().to_string()),
		ast::Item::TypeAlias(node) =>
			node.visibility().map(|visibility| visibility.syntax().text().to_string()),
		ast::Item::Const(node) =>
			node.visibility().map(|visibility| visibility.syntax().text().to_string()),
		ast::Item::Static(node) =>
			node.visibility().map(|visibility| visibility.syntax().text().to_string()),
		ast::Item::Trait(node) =>
			node.visibility().map(|visibility| visibility.syntax().text().to_string()),
		ast::Item::Enum(node) =>
			node.visibility().map(|visibility| visibility.syntax().text().to_string()),
		ast::Item::Struct(node) =>
			node.visibility().map(|visibility| visibility.syntax().text().to_string()),
		ast::Item::Fn(node) =>
			node.visibility().map(|visibility| visibility.syntax().text().to_string()),
		ast::Item::Impl(node) =>
			node.visibility().map(|visibility| visibility.syntax().text().to_string()),
		_ => None,
	}
}

#[cfg(test)]
mod tests {
	use std::{
		collections::{HashMap, HashSet},
		path::Path,
	};

	use crate::style::shared::STYLE_RULE_IDS;

	#[test]
	fn import_paths_ignore_use_examples_in_documentation() {
		let text = "/// Do not use wrong::Example; here.\nuse std::path::Path;\n";
		let ctx = super::read_file_context_from_text(Path::new("documented.rs"), text.to_owned())
			.expect("Parse source.")
			.expect("Source context.");

		assert_eq!(ctx.top_items[0].use_path.as_deref(), Some("std::path::Path"));
	}

	#[test]
	fn style_rule_ids_are_unique() {
		let mut seen = HashSet::new();

		for rule in STYLE_RULE_IDS {
			assert!(seen.insert(rule), "Duplicate style rule id: {rule}.");
		}
	}

	#[test]
	fn style_rule_ids_are_contiguous_per_prefix_and_sorted() {
		let mut prefix_max = HashMap::<String, usize>::new();
		let mut finished_prefixes = HashSet::<String>::new();
		let mut current_prefix = String::new();

		for rule in STYLE_RULE_IDS {
			let (language, without_head) =
				if let Some(without_head) = rule.strip_prefix("RUST-STYLE-") {
					("RUST", without_head)
				} else if let Some(without_head) = rule.strip_prefix("SWIFT-STYLE-") {
					("SWIFT", without_head)
				} else {
					panic!("Rule IDs must start with `RUST-STYLE-` or `SWIFT-STYLE-`.");
				};
			let (prefix, serial) = without_head
				.rsplit_once('-')
				.expect("Rule IDs must end with a three-digit serial.");
			let serial = serial.parse::<usize>().expect("Rule serial must be numeric.");
			let prefix = format!("{language}-{prefix}");

			if prefix != current_prefix {
				if !current_prefix.is_empty() {
					finished_prefixes.insert(current_prefix.clone());
				}

				assert!(
					!finished_prefixes.contains(&prefix),
					"Rule prefix `{prefix}` must be in a single contiguous run.",
				);

				current_prefix = prefix.clone();
			}

			let max = prefix_max.entry(prefix.clone()).or_insert(0);

			assert!(
				serial > *max,
				"Rule IDs for prefix `{prefix}` must be strictly increasing (found {serial:03} after {max:03}).",
			);

			*max = serial;
		}
	}
}
