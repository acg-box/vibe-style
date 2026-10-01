use std::{borrow::Cow, collections::HashSet};

use color_eyre::{Result, eyre};
use ra_ap_syntax::{AstNode, Edition, SourceFile, ast::Use};

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

fn is_lifetime_prefix(bytes: &[u8], start: usize) -> bool {
	if start + 1 >= bytes.len() {
		return false;
	}

	let next = bytes[start + 1];

	if !(next.is_ascii_alphabetic() || next == b'_') {
		return false;
	}
	if start + 2 >= bytes.len() {
		return true;
	}

	bytes[start + 2] != b'\''
}

fn literal_ranges(text: &str) -> Vec<(usize, usize)> {
	let bytes = text.as_bytes();
	let mut out = Vec::new();
	let mut idx = 0_usize;

	while idx < bytes.len() {
		if let Some(next) = skip_line_comment(bytes, idx) {
			idx = next;

			continue;
		}
		if let Some(next) = skip_block_comment(bytes, idx) {
			idx = next;

			continue;
		}
		if let Some((start, end)) = consume_string_like_literal(bytes, idx) {
			out.push((start, end));

			idx = end;

			continue;
		}
		if let Some((start, end)) = consume_char_literal(bytes, idx) {
			out.push((start, end));

			idx = end;

			continue;
		}

		idx += 1;
	}

	out
}

fn skip_line_comment(bytes: &[u8], idx: usize) -> Option<usize> {
	if !(bytes.get(idx) == Some(&b'/') && bytes.get(idx + 1) == Some(&b'/')) {
		return None;
	}

	let mut cursor = idx + 2;

	while cursor < bytes.len() && bytes[cursor] != b'\n' {
		cursor += 1;
	}

	Some(cursor)
}

fn skip_block_comment(bytes: &[u8], idx: usize) -> Option<usize> {
	if !(bytes.get(idx) == Some(&b'/') && bytes.get(idx + 1) == Some(&b'*')) {
		return None;
	}

	let mut cursor = idx + 2;
	let mut depth = 1_i32;

	while cursor + 1 < bytes.len() && depth > 0 {
		if bytes[cursor] == b'/' && bytes[cursor + 1] == b'*' {
			depth += 1;
			cursor += 2;

			continue;
		}
		if bytes[cursor] == b'*' && bytes[cursor + 1] == b'/' {
			depth -= 1;
			cursor += 2;

			continue;
		}

		cursor += 1;
	}

	Some(cursor)
}

fn consume_string_like_literal(bytes: &[u8], start: usize) -> Option<(usize, usize)> {
	let prefix_len = byte_string_prefix_len(bytes, start);
	let raw_start = start + prefix_len;

	if let Some(end) = consume_raw_string_literal(bytes, start, raw_start) {
		return Some((start, end));
	}
	if let Some(end) = consume_quoted_string_literal(bytes, start, raw_start) {
		return Some((start, end));
	}

	None
}

fn byte_string_prefix_len(bytes: &[u8], start: usize) -> usize {
	if bytes.get(start) == Some(&b'b') && matches!(bytes.get(start + 1), Some(b'"' | b'r')) {
		1
	} else {
		0
	}
}

fn consume_raw_string_literal(bytes: &[u8], start: usize, raw_start: usize) -> Option<usize> {
	if bytes.get(raw_start) != Some(&b'r') {
		return None;
	}

	let mut cursor = raw_start + 1;

	while cursor < bytes.len() && bytes[cursor] == b'#' {
		cursor += 1;
	}

	if bytes.get(cursor) != Some(&b'"') {
		return None;
	}

	let hash_count = cursor.saturating_sub(raw_start + 1);

	cursor += 1;

	while cursor < bytes.len() {
		if bytes[cursor] != b'"' {
			cursor += 1;

			continue;
		}
		if raw_hash_suffix_matches(bytes, cursor + 1, hash_count) {
			return Some(cursor + 1 + hash_count);
		}

		cursor += 1;
	}

	let _ = start;

	None
}

fn raw_hash_suffix_matches(bytes: &[u8], start: usize, hash_count: usize) -> bool {
	for offset in 0..hash_count {
		let pos = start + offset;

		if bytes.get(pos) != Some(&b'#') {
			return false;
		}
	}

	true
}

fn consume_quoted_string_literal(bytes: &[u8], start: usize, raw_start: usize) -> Option<usize> {
	if bytes.get(raw_start) != Some(&b'"') {
		return None;
	}

	let mut cursor = raw_start + 1;
	let mut escaped = false;

	while cursor < bytes.len() {
		let ch = bytes[cursor];

		if escaped {
			escaped = false;
			cursor += 1;

			continue;
		}
		if ch == b'\\' {
			escaped = true;
			cursor += 1;

			continue;
		}
		if ch == b'"' {
			return Some(cursor + 1);
		}

		cursor += 1;
	}

	let _ = start;

	None
}

fn consume_char_literal(bytes: &[u8], idx: usize) -> Option<(usize, usize)> {
	if bytes.get(idx) != Some(&b'\'') || is_lifetime_prefix(bytes, idx) {
		return None;
	}

	let mut cursor = idx + 1;
	let mut escaped = false;

	while cursor < bytes.len() {
		let ch = bytes[cursor];

		if escaped {
			escaped = false;
			cursor += 1;

			continue;
		}
		if ch == b'\\' {
			escaped = true;
			cursor += 1;

			continue;
		}
		if ch == b'\'' {
			return Some((idx, cursor + 1));
		}

		cursor += 1;
	}

	None
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
