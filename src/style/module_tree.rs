use std::{
	collections::{BTreeSet, HashSet},
	fs,
	path::{Path, PathBuf},
};

use ra_ap_syntax::{
	AstNode, AstToken, Edition, SourceFile, SyntaxNode,
	ast::{self, HasAttrs, HasName, Module},
};

use crate::style::workspace;

pub(crate) fn declared_file_is_test_only(path: &Path) -> bool {
	declared_file_module_contexts(path)
		.is_some_and(|contexts| !contexts.is_empty() && contexts.iter().all(|(_, test)| *test))
}

pub(crate) fn find_crate_dir(path: &Path) -> Option<PathBuf> {
	let mut dir = path.parent()?.to_path_buf();

	loop {
		if dir.join("Cargo.toml").is_file() {
			return Some(dir);
		}
		if !dir.pop() {
			break;
		}
	}

	None
}

pub(crate) fn declared_file_module_path(path: &Path) -> Option<Vec<String>> {
	let Some(contexts) = declared_file_module_contexts(path) else {
		return Some(file_module_path_segments(path));
	};
	let matches = contexts.into_iter().map(|(owner, _)| owner).collect::<BTreeSet<_>>();

	match matches.len() {
		0 => Some(file_module_path_segments(path)),
		1 => matches.into_iter().next(),
		_ => None,
	}
}

pub(crate) fn declared_child_file(
	module: &Module,
	path_directory: &Path,
	directory: &Path,
	name: &str,
) -> PathBuf {
	let explicit = module.attrs().find_map(|attr| {
		if attr.path()?.syntax().text() != "path" {
			return None;
		}

		let token = attr
			.syntax()
			.descendants_with_tokens()
			.filter_map(|part| part.into_token())
			.find_map(ast::String::cast)?;

		token.value().ok().map(|value| path_directory.join(value.as_ref()))
	});

	explicit.unwrap_or_else(|| {
		let flat = directory.join(format!("{name}.rs"));

		if flat.is_file() { flat } else { directory.join(name).join("mod.rs") }
	})
}

fn declared_file_module_contexts(path: &Path) -> Option<BTreeSet<(Vec<String>, bool)>> {
	let crate_dir = find_crate_dir(path)?;
	let target = path.canonicalize().ok()?;
	let roots = workspace::package_target_roots(&crate_dir)
		.ok()
		.flatten()
		.unwrap_or_else(|| vec![crate_dir.join("src/lib.rs"), crate_dir.join("src/main.rs")]);
	let mut matches = BTreeSet::new();
	let mut stack = HashSet::new();

	for root in roots {
		collect_declared_module_paths(
			&root,
			&(Vec::new(), false),
			&target,
			&mut stack,
			&mut matches,
		);
	}

	Some(matches)
}

fn collect_declared_module_paths(
	file: &Path,
	context: &(Vec<String>, bool),
	target: &Path,
	stack: &mut HashSet<PathBuf>,
	matches: &mut BTreeSet<(Vec<String>, bool)>,
) {
	let Ok(file) = file.canonicalize() else {
		return;
	};

	if !stack.insert(file.clone()) {
		return;
	}
	if file == target {
		matches.insert(context.clone());
	}

	if let Ok(text) = fs::read_to_string(&file) {
		let parsed = SourceFile::parse(&text, Edition::CURRENT);
		let parent = file.parent().unwrap_or(&file);
		let directory = if matches!(
			file.file_name().and_then(|name| name.to_str()),
			Some("lib.rs" | "main.rs" | "mod.rs")
		) {
			parent.to_path_buf()
		} else {
			parent.join(file.file_stem().unwrap_or_default())
		};

		collect_module_declarations(
			parsed.tree().syntax(),
			parent,
			&directory,
			context,
			target,
			stack,
			matches,
		);
	}

	stack.remove(&file);
}

fn collect_module_declarations(
	syntax: &SyntaxNode,
	path_directory: &Path,
	directory: &Path,
	owner: &(Vec<String>, bool),
	target: &Path,
	stack: &mut HashSet<PathBuf>,
	matches: &mut BTreeSet<(Vec<String>, bool)>,
) {
	for module in syntax.children().filter_map(Module::cast) {
		let Some(name) = module.name().map(|name| name.text().to_string()) else {
			continue;
		};
		let test_only = owner.1
			|| module.attrs().any(|attr| {
				attr.syntax().text().to_string().split_whitespace().collect::<String>()
					== "#[cfg(test)]"
			});
		let mut module_path = owner.0.clone();

		module_path.push(name.clone());

		let context = (module_path, test_only);

		if let Some(items) = module.item_list() {
			let nested = directory.join(&name);

			collect_module_declarations(
				items.syntax(),
				&nested,
				&nested,
				&context,
				target,
				stack,
				matches,
			);

			continue;
		}

		let file = declared_child_file(&module, path_directory, directory, &name);

		collect_declared_module_paths(&file, &context, target, stack, matches);
	}
}

fn file_module_path_segments(path: &Path) -> Vec<String> {
	let Some(crate_dir) = find_crate_dir(path) else {
		return Vec::new();
	};
	let src_dir = crate_dir.join("src");
	let Ok(relative) = path.strip_prefix(src_dir) else {
		return Vec::new();
	};
	let mut components = relative
		.iter()
		.map(|component| component.to_string_lossy().to_string())
		.collect::<Vec<_>>();
	let Some(file_name) = components.pop() else {
		return Vec::new();
	};

	match file_name.as_str() {
		"lib.rs" | "main.rs" => Vec::new(),
		"mod.rs" => components,
		_ => {
			let stem = file_name.strip_suffix(".rs").unwrap_or(file_name.as_str());

			components.push(stem.to_owned());

			components
		},
	}
}
