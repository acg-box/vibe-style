use std::{
	collections::HashSet,
	fs,
	path::{Path, PathBuf},
	process::Command,
};

use ast::Item;
use cargo_metadata::{MetadataCommand, TargetKind};
use once_cell::sync::Lazy;
use ra_ap_syntax::{
	AstNode, Edition, SourceFile, TextRange,
	ast::{self, HasAttrs, HasModuleItem, HasName, HasVisibility},
};
use regex::Regex;

use crate::prelude::*;

pub(crate) const STYLE_RULE_IDS: [&str; 29] = [
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
	"RUST-STYLE-IMPORT-008",
	"RUST-STYLE-IMPORT-009",
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

pub(crate) static USE_RE: Lazy<Regex> = Lazy::new(|| {
	Regex::new(r"^\s*(pub\s+)?use\s+(.+);\s*$").expect("Expected operation to succeed.")
});
pub(crate) static INLINE_BOUNDS_RE: Lazy<Regex> = Lazy::new(|| {
	Regex::new(
		r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:fn|impl|struct|enum|trait)\b[^\n{;]*<[^>{}]*\b(?:[A-Za-z_][A-Za-z0-9_]*|'[A-Za-z_][A-Za-z0-9_]*)\s*:[^>{}]*>",
	)
	.expect("Expected operation to succeed.")
});
pub(crate) static SNAKE_CASE_RE: Lazy<Regex> =
	Lazy::new(|| Regex::new(r"^[a-z][a-z0-9_]*$").expect("Expected operation to succeed."));
pub(crate) static WORKSPACE_IMPORT_ROOTS: Lazy<HashSet<String>> = Lazy::new(|| {
	let mut roots = HashSet::new();

	let pkg_name = env!("CARGO_PKG_NAME");
	roots.insert(pkg_name.to_owned());
	roots.insert(pkg_name.replace('-', "_"));

	let mut cmd = MetadataCommand::new();
	cmd.no_deps();
	let metadata = cmd.exec();
	if let Ok(metadata) = metadata {
		for package in metadata.packages {
			let package_name = package.name.to_string();
			roots.insert(package_name.clone());
			roots.insert(package_name.replace('-', "_"));
			for target in package.targets {
				let is_library_target = target
					.kind
					.iter()
					.any(|kind| matches!(kind, TargetKind::Lib | TargetKind::ProcMacro));
				if !is_library_target {
					continue;
				}
				roots.insert(target.name.clone());
				roots.insert(target.name.replace('-', "_"));
			}
		}
	}

	roots
});

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
pub(crate) struct Edit {
	pub(crate) start: usize,
	pub(crate) end: usize,
	pub(crate) replacement: String,
	pub(crate) rule: &'static str,
}

#[derive(Debug, Clone)]
pub(crate) struct RunSummary {
	pub(crate) file_count: usize,
	pub(crate) violation_count: usize,
	pub(crate) unfixable_count: usize,
	pub(crate) applied_fix_count: usize,
	pub(crate) output_lines: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CargoOptions {
	pub(crate) workspace: bool,
	pub(crate) packages: Vec<String>,
	pub(crate) features: Vec<String>,
	pub(crate) all_features: bool,
	pub(crate) no_default_features: bool,
}
impl CargoOptions {
	pub(crate) fn has_package_filter(&self) -> bool {
		self.workspace || !self.packages.is_empty()
	}
}

#[derive(Debug, Clone)]
pub(crate) struct TopItem {
	pub(crate) kind: TopKind,
	pub(crate) name: Option<String>,
	pub(crate) line: usize,
	pub(crate) start_line: usize,
	pub(crate) end_line: usize,
	pub(crate) is_pub: bool,
	pub(crate) is_async: bool,
	pub(crate) attrs: Vec<String>,
	pub(crate) impl_target: Option<String>,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

pub(crate) fn resolve_files(
	requested_files: &[PathBuf],
	cargo_options: &CargoOptions,
) -> Result<Vec<PathBuf>> {
	if !requested_files.is_empty() {
		let mut files = Vec::new();

		for file in requested_files {
			if file.extension().is_some_and(|ext| ext == "rs") {
				files.push(file.clone());
			}
		}

		return Ok(files);
	}

	let git_files = git_ls_files_rs()?;

	if !cargo_options.has_package_filter() {
		return Ok(git_files);
	}

	let mut cmd = MetadataCommand::new();

	cmd.no_deps();

	let metadata = cmd.exec().map_err(|err| eyre::eyre!("Failed to run cargo metadata: {err}."))?;
	let workspace_member_ids = metadata.workspace_members.iter().cloned().collect::<HashSet<_>>();
	let workspace_packages = metadata
		.packages
		.into_iter()
		.filter(|package| workspace_member_ids.contains(&package.id))
		.collect::<Vec<_>>();
	let mut selected_roots = HashSet::new();

	if cargo_options.workspace {
		for package in &workspace_packages {
			let manifest = PathBuf::from(package.manifest_path.as_str());
			let Some(root) = manifest.parent() else {
				continue;
			};

			selected_roots.insert(normalize_path(root));
		}
	}
	if !cargo_options.packages.is_empty() {
		let mut missing = cargo_options.packages.iter().cloned().collect::<HashSet<_>>();

		for package in &workspace_packages {
			let package_name = package.name.as_str();
			let package_name_snake = package_name.replace('-', "_");
			let target_names =
				package.targets.iter().map(|target| target.name.as_str()).collect::<Vec<_>>();
			let mut matched = false;

			for requested in &cargo_options.packages {
				let requested_snake = requested.replace('-', "_");
				let hit = requested == package_name
					|| requested_snake == package_name_snake
					|| target_names.iter().any(|name| {
						*name == requested
							|| name.replace('-', "_") == requested_snake
							|| requested.replace('-', "_") == name.replace('-', "_")
					});

				if hit {
					missing.remove(requested);

					matched = true;
				}
			}

			if matched {
				let manifest = PathBuf::from(package.manifest_path.as_str());
				let Some(root) = manifest.parent() else {
					continue;
				};

				selected_roots.insert(normalize_path(root));
			}
		}

		if !missing.is_empty() {
			let mut missing = missing.into_iter().collect::<Vec<_>>();

			missing.sort();

			return Err(eyre::eyre!(
				"Requested package(s) not found in workspace: {}.",
				missing.join(", ")
			));
		}
	}
	if selected_roots.is_empty() {
		return Ok(Vec::new());
	}

	let cwd =
		std::env::current_dir().map_err(|err| eyre::eyre!("Failed to resolve cwd: {err}."))?;
	let mut files = Vec::new();

	for relative in git_files {
		let absolute = normalize_path(&cwd.join(&relative));

		if selected_roots.iter().any(|root| absolute.starts_with(root)) {
			files.push(relative);
		}
	}

	Ok(files)
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

fn git_ls_files_rs() -> Result<Vec<PathBuf>> {
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

fn normalize_path(path: &Path) -> PathBuf {
	match fs::canonicalize(path) {
		Ok(canonical) => canonical,
		Err(_) => path.to_path_buf(),
	}
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

fn classify_top_kind(item: &Item) -> TopKind {
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

fn item_name(item: &Item) -> Option<String> {
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

fn item_visibility_is_pub(item: &Item) -> bool {
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
