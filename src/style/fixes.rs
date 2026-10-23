use std::{borrow::Cow, collections::HashSet};

use color_eyre::{Result, eyre};
use ra_ap_syntax::{AstNode, Edition, SourceFile, SyntaxKind, ast::Use};

use crate::style::shared::Edit;

pub(crate) fn apply_edits(text: &mut String, mut edits: Vec<Edit>) -> Result<usize> {
	if edits.is_empty() {
		return Ok(0);
	}

	edits = edits
		.into_iter()
		.map(|edit| trim_spacing_edit_edges(text, edit))
		.filter(|edit| edit.start != edit.end || !edit.replacement.is_empty())
		.collect();

	let literal_ranges = literal_ranges(text);

	edits.sort_by(|a, b| a.start.cmp(&b.start).then(a.end.cmp(&b.end)).then(a.rule.cmp(b.rule)));
	edits.dedup_by(|a, b| {
		a.start == b.start && a.end == b.end && a.rule == b.rule && a.replacement == b.replacement
	});

	let mut blocked_import_rules = HashSet::new();

	// Shortening and full qualification are competing plans. Apply unambiguous
	// shortening first, then reconsider qualification against the updated source.
	if edits.iter().any(|edit| edit.rule == "RUST-STYLE-IMPORT-008") {
		blocked_import_rules.insert("RUST-STYLE-IMPORT-009");
	}

	let filtered = loop {
		let mut selected = Vec::new();
		let mut last_end = 0;
		let mut partial_import_rule = None;

		for edit in &edits {
			if blocked_import_rules.contains(edit.rule)
				|| (!allows_literal_overlap(edit.rule)
					&& intersects_literal_range(edit, &literal_ranges))
			{
				continue;
			}
			if edit.start < last_end {
				if edit.rule.starts_with("RUST-STYLE-IMPORT-") {
					partial_import_rule = Some(edit.rule);

					break;
				}

				continue;
			}

			last_end = edit.end;

			selected.push(edit);
		}

		if let Some(rule) = partial_import_rule {
			// Import and reference edits form one transaction. Re-plan without a
			// conflicting rule instead of applying only its surviving fragments.
			blocked_import_rules.insert(rule);
		} else {
			break selected;
		}
	};

	if filtered.is_empty() {
		return Ok(0);
	}

	let mut inserted_imports = HashSet::new();

	for edit in filtered.iter().rev() {
		if edit.end > text.len() || edit.start > edit.end {
			return Err(eyre::eyre!(
				"Invalid edit range {}..{} for text length {}.",
				edit.start,
				edit.end,
				text.len()
			));
		}

		let replacement = deduplicated_import_insertion(edit, &mut inserted_imports);

		text.replace_range(edit.start..edit.end, &replacement);
	}

	Ok(filtered.len())
}

fn trim_spacing_edit_edges(text: &str, mut edit: Edit) -> Edit {
	if !matches!(edit.rule, "RUST-STYLE-SPACE-003" | "RUST-STYLE-SPACE-004") {
		return edit;
	}

	let Some(original) = text.get(edit.start..edit.end) else {
		return edit;
	};
	let prefix = original
		.chars()
		.zip(edit.replacement.chars())
		.take_while(|(left, right)| left == right)
		.map(|(character, _)| character.len_utf8())
		.sum::<usize>();
	let suffix = original[prefix..]
		.chars()
		.rev()
		.zip(edit.replacement[prefix..].chars().rev())
		.take_while(|(left, right)| left == right)
		.map(|(character, _)| character.len_utf8())
		.sum::<usize>();

	edit.start += prefix;
	edit.end -= suffix;
	edit.replacement = edit.replacement[prefix..edit.replacement.len() - suffix].to_owned();

	edit
}

fn deduplicated_import_insertion<'a>(
	edit: &'a Edit,
	inserted_imports: &mut HashSet<(usize, String)>,
) -> Cow<'a, str> {
	if edit.start != edit.end || !edit.rule.starts_with("RUST-STYLE-IMPORT-") {
		return Cow::Borrowed(&edit.replacement);
	}

	// All owning rule transactions have been selected before shared imports are removed.
	let parsed = SourceFile::parse(&edit.replacement, Edition::CURRENT);
	let mut duplicate_ranges = Vec::new();

	for item in parsed.tree().syntax().children().filter_map(Use::cast) {
		let key = (edit.start, item.syntax().text().to_string());

		if !inserted_imports.insert(key) {
			duplicate_ranges.push(item.syntax().text_range());
		}
	}

	if duplicate_ranges.is_empty() {
		return Cow::Borrowed(&edit.replacement);
	}

	let mut replacement = edit.replacement.clone();

	for range in duplicate_ranges.into_iter().rev() {
		replacement.replace_range(usize::from(range.start())..usize::from(range.end()), "");
	}

	if replacement.trim().is_empty() {
		replacement.clear();
	}

	Cow::Owned(replacement)
}

fn literal_ranges(text: &str) -> Vec<(usize, usize)> {
	SourceFile::parse(text, Edition::CURRENT)
		.tree()
		.syntax()
		.descendants_with_tokens()
		.filter_map(|part| part.into_token())
		.filter(|token| {
			matches!(
				token.kind(),
				SyntaxKind::STRING
					| SyntaxKind::BYTE_STRING
					| SyntaxKind::C_STRING
					| SyntaxKind::CHAR
					| SyntaxKind::BYTE
			)
		})
		.map(|token| {
			let range = token.text_range();

			(usize::from(range.start()), usize::from(range.end()))
		})
		.collect()
}

fn intersects_literal_range(edit: &Edit, literal_ranges: &[(usize, usize)]) -> bool {
	literal_ranges.iter().any(|(start, end)| {
		if edit.start == edit.end {
			*start <= edit.start && edit.start < *end
		} else {
			edit.start < *end && edit.end > *start
		}
	})
}

fn allows_literal_overlap(rule: &str) -> bool {
	rule.starts_with("RUST-STYLE-IMPORT-")
		|| matches!(
			rule,
			"RUST-STYLE-MOD-001"
				| "RUST-STYLE-MOD-002"
				| "RUST-STYLE-MOD-003"
				| "RUST-STYLE-MOD-005"
				| "RUST-STYLE-IMPL-003"
				| "RUST-STYLE-LET-001"
		)
}
