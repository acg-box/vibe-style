use std::{
	collections::{BTreeMap, HashSet},
	env, fs,
	io::Write as _,
	path::{Path, PathBuf},
	process::{Command, Stdio},
	sync::{LazyLock, Mutex},
};

use cargo_metadata::{MetadataCommand, Package, TargetKind};
use color_eyre::{Result, eyre};
use ra_ap_syntax::{
	AstNode, Edition, SourceFile, TextRange,
	ast::{self, HasAttrs, HasModuleItem, HasName, HasVisibility, Item},
};
use regex::Regex;

type StyleFilesCacheKey = (PathBuf, PathBuf);
type StyleFilesCache = BTreeMap<StyleFilesCacheKey, Vec<PathBuf>>;

pub(crate) const STYLE_RULE_IDS: [&str; 43] = [
	"RUST-STYLE-FILE-001",
	"RUST-STYLE-MOD-001",
	"RUST-STYLE-MOD-002",
	"RUST-STYLE-MOD-003",
	"RUST-STYLE-MOD-004",
	"RUST-STYLE-MOD-005",
	"RUST-STYLE-MOD-007",
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
pub(crate) static WORKSPACE_IMPORT_ROOTS: LazyLock<HashSet<String>> = LazyLock::new(|| {
	let pkg_name = env!("CARGO_PKG_NAME");

	let mut roots = HashSet::new();
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

static STYLE_FILES_CACHE: LazyLock<Mutex<StyleFilesCache>> =
	LazyLock::new(|| Mutex::new(BTreeMap::new()));
static WORKSPACE_LAYOUT_CACHE: LazyLock<Mutex<BTreeMap<PathBuf, WorkspaceLayout>>> =
	LazyLock::new(|| Mutex::new(BTreeMap::new()));

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StyleLanguage {
	Rust,
	Swift,
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
}

#[derive(Clone, Debug)]
struct WorkspacePackageInfo {
	name: String,
	snake_name: String,
	root: PathBuf,
}

#[derive(Clone, Debug)]
struct WorkspaceLayout {
	workspace_root: PathBuf,
	workspace_packages: Vec<WorkspacePackageInfo>,
	default_roots: Vec<PathBuf>,
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

pub(crate) fn resolve_files(cargo_options: &CargoOptions) -> Result<Vec<PathBuf>> {
	let workspace_layout = workspace_layout()?;
	let mut selected_rust_roots = HashSet::new();
	let mut selected_swift_roots = HashSet::new();

	if cargo_options.workspace {
		match cargo_options.language {
			StyleLanguage::Rust =>
				for package in &workspace_layout.workspace_packages {
					selected_rust_roots.insert(package.root.clone());
				},
			StyleLanguage::Swift => {
				selected_swift_roots.insert(workspace_layout.workspace_root.clone());
			},
		}
	}
	if !cargo_options.workspace && cargo_options.packages.is_empty() {
		for root in &workspace_layout.default_roots {
			match cargo_options.language {
				StyleLanguage::Rust => {
					selected_rust_roots.insert(root.clone());
				},
				StyleLanguage::Swift => {
					selected_swift_roots.insert(root.clone());
				},
			}
		}
	}
	if !cargo_options.packages.is_empty() {
		let mut missing = cargo_options.packages.iter().cloned().collect::<HashSet<_>>();

		for package in &workspace_layout.workspace_packages {
			let package_name = package.name.as_str();
			let mut matched = false;

			for requested in &cargo_options.packages {
				let requested_snake = requested.replace('-', "_");
				let hit = requested == package_name || requested_snake == package.snake_name;

				if hit {
					missing.remove(requested);

					matched = true;
				}
			}

			if matched {
				match cargo_options.language {
					StyleLanguage::Rust => {
						selected_rust_roots.insert(package.root.clone());
					},
					StyleLanguage::Swift => {
						selected_swift_roots.insert(package.root.clone());
					},
				}
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
	if selected_rust_roots.is_empty() && selected_swift_roots.is_empty() {
		return Ok(Vec::new());
	}

	let cwd = current_dir_normalized()?;
	let style_files = gitignore_visible_style_sources(&workspace_layout.workspace_root)?;
	let mut files = Vec::new();

	for relative in style_files {
		let absolute = normalize_path(&cwd.join(&relative));
		let selected_roots =
			if is_swift_file(&relative) { &selected_swift_roots } else { &selected_rust_roots };

		if selected_roots.iter().any(|root| absolute.starts_with(root)) {
			files.push(relative);
		}
	}

	Ok(files)
}

pub(crate) fn is_swift_file(path: &Path) -> bool {
	path.extension().and_then(|ext| ext.to_str()) == Some("swift")
}

pub(crate) fn package_file_map_for_files(
	files: &[PathBuf],
) -> Result<Option<BTreeMap<String, Vec<PathBuf>>>> {
	if files.is_empty() {
		return Ok(Some(BTreeMap::new()));
	}

	let cwd = current_dir_normalized()?;
	let workspace_layout = workspace_layout()?;
	let mut packages = BTreeMap::<String, Vec<PathBuf>>::new();

	for file in files {
		let abs = normalize_path(&cwd.join(file));
		let Some(package) = workspace_package_for_path(&workspace_layout, &abs) else {
			return Ok(None);
		};

		packages.entry(package.name.clone()).or_default().push(file.clone());
	}

	Ok(Some(packages))
}

pub(crate) fn package_names_for_files(files: &[PathBuf]) -> Result<Option<Vec<String>>> {
	if files.is_empty() {
		return Ok(Some(Vec::new()));
	}

	let Some(packages) = package_file_map_for_files(files)? else {
		return Ok(None);
	};

	Ok(Some(packages.into_keys().collect()))
}

pub(crate) fn package_rust_files_for_path(path: &Path) -> Result<Option<(PathBuf, Vec<PathBuf>)>> {
	let absolute = if path.is_absolute() {
		normalize_path(path)
	} else {
		let cwd = current_dir_normalized()?;

		normalize_path(&cwd.join(path))
	};

	if !absolute.is_file() {
		return Ok(None);
	}

	let workspace_layout =
		workspace_layout_for_dir(absolute.parent().unwrap_or_else(|| Path::new("/")))?;
	let Some(package) = workspace_package_for_path(&workspace_layout, &absolute) else {
		return Ok(None);
	};
	let mut files = Vec::new();

	collect_package_rust_files(&package.root, &mut files)?;

	files.sort();
	files.dedup();

	Ok(Some((absolute, files)))
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

fn collect_package_rust_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
	let entries = fs::read_dir(dir).map_err(|err| {
		eyre::eyre!("Failed to read package directory `{}`: {err}.", dir.display())
	})?;

	for entry in entries {
		let entry =
			entry.map_err(|err| eyre::eyre!("Failed to read package directory entry: {err}."))?;
		let path = entry.path();
		let file_type = entry.file_type().map_err(|err| {
			eyre::eyre!("Failed to read file type for package path `{}`: {err}.", path.display())
		})?;
		let name = entry.file_name();
		let name = name.to_string_lossy();

		if file_type.is_dir() {
			if should_skip_package_scan_dir(name.as_ref()) {
				continue;
			}

			collect_package_rust_files(&path, files)?;

			continue;
		}
		if file_type.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
			files.push(normalize_path(&path));
		}
	}

	Ok(())
}

fn gitignore_visible_style_sources(workspace_root: &Path) -> Result<Vec<PathBuf>> {
	let cwd = current_dir_normalized()?;
	let workspace_root = normalize_path(workspace_root);
	let cache_key = (cwd.clone(), workspace_root.clone());

	if let Some(files) =
		STYLE_FILES_CACHE.lock().expect("Lock style files cache.").get(&cache_key).cloned()
	{
		return Ok(files);
	}

	let mut files = Vec::new();

	collect_gitignore_visible_style_sources(&workspace_root, &cwd, &mut files)?;

	files.sort();
	files.dedup();
	STYLE_FILES_CACHE.lock().expect("Lock style files cache.").insert(cache_key, files.clone());

	Ok(files)
}

fn collect_gitignore_visible_style_sources(
	dir: &Path,
	cwd: &Path,
	files: &mut Vec<PathBuf>,
) -> Result<()> {
	let entries = fs::read_dir(dir)
		.map_err(|err| eyre::eyre!("Failed to read style directory `{}`: {err}.", dir.display()))?;
	let mut entries = entries
		.map(|entry| {
			let entry =
				entry.map_err(|err| eyre::eyre!("Failed to read style directory entry: {err}."))?;
			let path = entry.path();
			let file_type = entry.file_type().map_err(|err| {
				eyre::eyre!("Failed to read file type for style path `{}`: {err}.", path.display())
			})?;

			Ok((path, file_type, entry.file_name()))
		})
		.collect::<Result<Vec<_>>>()?;

	entries.sort_by(|left, right| left.0.cmp(&right.0));

	let ignored = gitignored_paths(entries.iter().map(|(path, _, _)| path.as_path()), cwd)?;

	for (path, file_type, name) in entries {
		let relative = path_for_cwd(&path, cwd);
		let is_ignored = ignored.contains(&relative);
		let name = name.to_string_lossy();

		if file_type.is_dir() {
			if name == ".git" || is_ignored {
				continue;
			}

			collect_gitignore_visible_style_sources(&path, cwd, files)?;

			continue;
		}
		if file_type.is_file() && is_style_source_file(&path) && !is_ignored {
			files.push(relative);
		}
	}

	Ok(())
}

fn gitignored_paths<'a>(
	paths: impl IntoIterator<Item = &'a Path>,
	cwd: &Path,
) -> Result<HashSet<PathBuf>> {
	let paths = paths.into_iter().map(|path| path_for_cwd(path, cwd)).collect::<Vec<_>>();

	if paths.is_empty() {
		return Ok(HashSet::new());
	}

	let mut child = Command::new("git")
		.current_dir(cwd)
		.args(["check-ignore", "--no-index", "--stdin"])
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.spawn()
		.map_err(|err| eyre::eyre!("Failed to run git check-ignore: {err}."))?;

	{
		let mut stdin = child
			.stdin
			.take()
			.ok_or_else(|| eyre::eyre!("Failed to open git check-ignore stdin."))?;

		for path in &paths {
			writeln!(stdin, "{}", path.display())
				.map_err(|err| eyre::eyre!("Failed to write git check-ignore input: {err}."))?;
		}
	}

	let output = child
		.wait_with_output()
		.map_err(|err| eyre::eyre!("Failed to wait for git check-ignore: {err}."))?;

	match output.status.code() {
		Some(0) | Some(1) => {},
		_ => return Err(eyre::eyre!("git check-ignore failed with status {}.", output.status)),
	}

	let stdout = String::from_utf8(output.stdout)?;
	let mut ignored = HashSet::new();

	for line in stdout.lines() {
		if !line.is_empty() {
			ignored.insert(PathBuf::from(line));
		}
	}

	Ok(ignored)
}

fn is_style_source_file(path: &Path) -> bool {
	matches!(path.extension().and_then(|ext| ext.to_str()), Some("rs" | "swift"))
}

fn path_for_cwd(path: &Path, cwd: &Path) -> PathBuf {
	let normalized = normalize_path(path);

	normalized.strip_prefix(cwd).map_or(normalized.clone(), Path::to_path_buf)
}

fn workspace_layout() -> Result<WorkspaceLayout> {
	let cwd = current_dir_normalized()?;

	workspace_layout_for_dir(&cwd)
}

fn workspace_layout_for_dir(dir: &Path) -> Result<WorkspaceLayout> {
	let cwd = normalize_path(dir);

	if let Some(layout) =
		WORKSPACE_LAYOUT_CACHE.lock().expect("Lock workspace layout cache.").get(&cwd).cloned()
	{
		return Ok(layout);
	}

	let mut cmd = MetadataCommand::new();

	cmd.no_deps();
	cmd.current_dir(&cwd);

	let metadata = cmd.exec().map_err(|err| eyre::eyre!("Failed to run cargo metadata: {err}."))?;
	let workspace_root = normalize_path(&PathBuf::from(metadata.workspace_root.as_str()));
	let workspace_member_ids = metadata.workspace_members.iter().cloned().collect::<HashSet<_>>();
	let mut workspace_packages = metadata
		.packages
		.iter()
		.filter(|package| workspace_member_ids.contains(&package.id))
		.filter_map(workspace_package_info)
		.collect::<Vec<_>>();

	workspace_packages.sort_by(|left, right| {
		let left_len = left.root.as_os_str().to_string_lossy().len();
		let right_len = right.root.as_os_str().to_string_lossy().len();

		right_len.cmp(&left_len)
	});

	let default_packages = if metadata.workspace_default_members.is_available() {
		metadata.workspace_default_packages()
	} else if let Some(root_package) = metadata.root_package() {
		vec![root_package]
	} else {
		metadata.workspace_packages()
	};
	let mut default_roots =
		default_packages.into_iter().filter_map(workspace_package_root).collect::<Vec<_>>();

	default_roots.sort();
	default_roots.dedup();

	let layout = WorkspaceLayout { workspace_root, workspace_packages, default_roots };

	WORKSPACE_LAYOUT_CACHE
		.lock()
		.expect("Lock workspace layout cache.")
		.insert(cwd, layout.clone());

	Ok(layout)
}

fn workspace_package_info(package: &Package) -> Option<WorkspacePackageInfo> {
	let name = package.name.to_string();
	let snake_name = name.replace('-', "_");
	let root = workspace_package_root(package)?;

	Some(WorkspacePackageInfo { name, snake_name, root })
}

fn workspace_package_root(package: &Package) -> Option<PathBuf> {
	let manifest = PathBuf::from(package.manifest_path.as_str());
	let root = manifest.parent()?;

	Some(normalize_path(root))
}

fn workspace_package_for_path<'a>(
	layout: &'a WorkspaceLayout,
	absolute: &Path,
) -> Option<&'a WorkspacePackageInfo> {
	layout.workspace_packages.iter().find(|package| absolute.starts_with(&package.root))
}

fn current_dir_normalized() -> Result<PathBuf> {
	let cwd = env::current_dir().map_err(|err| eyre::eyre!("Failed to resolve cwd: {err}."))?;

	Ok(normalize_path(&cwd))
}

fn normalize_path(path: &Path) -> PathBuf {
	match fs::canonicalize(path) {
		Ok(canonical) => canonical,
		Err(_) => path.to_path_buf(),
	}
}

fn should_skip_package_scan_dir(name: &str) -> bool {
	name == "target" || name.starts_with('.')
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
			extract_use_path_from_item_text(&use_item.syntax().text().to_string())
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

fn extract_use_path_from_item_text(text: &str) -> Option<String> {
	find_use_path_range(text)
		.and_then(|(start, end)| text.get(start..end).map(|s| s.trim().to_owned()))
}

fn find_use_path_range(text: &str) -> Option<(usize, usize)> {
	for (idx, _) in text.match_indices("use") {
		let prev = text[..idx].chars().next_back();
		let next = text.get(idx + 3..).and_then(|tail| tail.chars().next());
		let is_prev_boundary = prev.is_none_or(|ch| !(ch.is_ascii_alphanumeric() || ch == '_'));
		let is_next_whitespace = next.is_some_and(char::is_whitespace);

		if !is_prev_boundary || !is_next_whitespace {
			continue;
		}

		let bytes = text.as_bytes();
		let mut start = idx + 3;

		while start < bytes.len() && bytes[start].is_ascii_whitespace() {
			start += 1;
		}

		let tail = text.get(start..)?;
		let semi = tail.find(';')?;

		return Some((start, start + semi));
	}

	None
}

fn classify_top_kind(item: &Item) -> TopKind {
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
	item_visibility_text(item).is_some()
}

fn item_visibility_key(item: &Item) -> String {
	item_visibility_text(item)
		.map(|text| text.chars().filter(|ch| !ch.is_whitespace()).collect::<String>())
		.unwrap_or_default()
}

fn item_visibility_text(item: &Item) -> Option<String> {
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
	use std::collections::{HashMap, HashSet};

	use crate::style::shared::STYLE_RULE_IDS;

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
