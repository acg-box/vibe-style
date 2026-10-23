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

use crate::style::shared::{self, CargoOptions, StyleLanguage};

type StyleFilesCacheKey = (PathBuf, PathBuf);

type StyleFilesCache = BTreeMap<StyleFilesCacheKey, Vec<PathBuf>>;

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
struct WorkspacePackageInfo {
	name: String,
	snake_name: String,
	root: PathBuf,
	target_roots: Vec<PathBuf>,
}

#[derive(Clone, Debug)]
struct WorkspaceLayout {
	workspace_root: PathBuf,
	workspace_packages: Vec<WorkspacePackageInfo>,
	default_roots: Vec<PathBuf>,
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
		let absolute = shared::normalize_path(&cwd.join(&relative));
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
		let abs = shared::normalize_path(&cwd.join(file));
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

pub(crate) fn package_target_roots(crate_dir: &Path) -> Result<Option<Vec<PathBuf>>> {
	let root = shared::normalize_path(crate_dir);
	let layout = workspace_layout_for_dir(&root)?;

	Ok(layout
		.workspace_packages
		.iter()
		.find(|package| package.root == root)
		.map(|package| package.target_roots.clone()))
}

pub(crate) fn package_rust_files_for_path(path: &Path) -> Result<Option<(PathBuf, Vec<PathBuf>)>> {
	let absolute = if path.is_absolute() {
		shared::normalize_path(path)
	} else {
		let cwd = current_dir_normalized()?;

		shared::normalize_path(&cwd.join(path))
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
			files.push(shared::normalize_path(&path));
		}
	}

	Ok(())
}

fn gitignore_visible_style_sources(workspace_root: &Path) -> Result<Vec<PathBuf>> {
	let cwd = current_dir_normalized()?;
	let workspace_root = shared::normalize_path(workspace_root);
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
		.args(["check-ignore", "--no-index", "--stdin", "-z"])
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
			write!(stdin, "{}\0", path.display())
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

	for line in stdout.split('\0') {
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
	let normalized = shared::normalize_path(path);

	normalized.strip_prefix(cwd).map_or(normalized.clone(), Path::to_path_buf)
}

fn workspace_layout() -> Result<WorkspaceLayout> {
	let cwd = current_dir_normalized()?;

	workspace_layout_for_dir(&cwd)
}

fn workspace_layout_for_dir(dir: &Path) -> Result<WorkspaceLayout> {
	let cwd = shared::normalize_path(dir);

	if let Some(layout) =
		WORKSPACE_LAYOUT_CACHE.lock().expect("Lock workspace layout cache.").get(&cwd).cloned()
	{
		return Ok(layout);
	}

	let mut cmd = MetadataCommand::new();

	cmd.no_deps();
	cmd.current_dir(&cwd);

	let metadata = cmd.exec().map_err(|err| eyre::eyre!("Failed to run cargo metadata: {err}."))?;
	let workspace_root = shared::normalize_path(&PathBuf::from(metadata.workspace_root.as_str()));
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
	let target_roots = package
		.targets
		.iter()
		.map(|target| shared::normalize_path(Path::new(target.src_path.as_str())))
		.collect();

	Some(WorkspacePackageInfo { name, snake_name, root, target_roots })
}

fn workspace_package_root(package: &Package) -> Option<PathBuf> {
	let manifest = PathBuf::from(package.manifest_path.as_str());
	let root = manifest.parent()?;

	Some(shared::normalize_path(root))
}

fn workspace_package_for_path<'a>(
	layout: &'a WorkspaceLayout,
	absolute: &Path,
) -> Option<&'a WorkspacePackageInfo> {
	layout.workspace_packages.iter().find(|package| absolute.starts_with(&package.root))
}

fn current_dir_normalized() -> Result<PathBuf> {
	let cwd = env::current_dir().map_err(|err| eyre::eyre!("Failed to resolve cwd: {err}."))?;

	Ok(shared::normalize_path(&cwd))
}

fn should_skip_package_scan_dir(name: &str) -> bool {
	name == "target" || name.starts_with('.')
}
