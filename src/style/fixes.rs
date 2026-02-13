use color_eyre::{Result, eyre};

use super::shared::Edit;

pub(crate) fn apply_edits(text: &mut String, mut edits: Vec<Edit>) -> Result<usize> {
	if edits.is_empty() {
		return Ok(0);
	}

	let literal_ranges = literal_ranges(text);

	edits.sort_by(|a, b| a.start.cmp(&b.start).then(a.end.cmp(&b.end)).then(a.rule.cmp(b.rule)));

	let mut filtered = Vec::new();
	let mut last_end = 0_usize;

	for edit in edits {
		if !allows_literal_overlap(edit.rule) && intersects_literal_range(&edit, &literal_ranges) {
			continue;
		}
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
	matches!(
		rule,
		"RUST-STYLE-MOD-001"
			| "RUST-STYLE-MOD-002"
			| "RUST-STYLE-MOD-003"
			| "RUST-STYLE-MOD-005"
			| "RUST-STYLE-SERDE-001"
			| "RUST-STYLE-RUNTIME-002"
	)
}
