use super::shared::Edit;
use crate::prelude::*;

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
		if bytes[idx] == b'/' && idx + 1 < bytes.len() && bytes[idx + 1] == b'/' {
			idx += 2;
			while idx < bytes.len() && bytes[idx] != b'\n' {
				idx += 1;
			}
			continue;
		}
		if bytes[idx] == b'/' && idx + 1 < bytes.len() && bytes[idx + 1] == b'*' {
			idx += 2;
			let mut depth = 1_i32;
			while idx + 1 < bytes.len() && depth > 0 {
				if bytes[idx] == b'/' && bytes[idx + 1] == b'*' {
					depth += 1;
					idx += 2;
					continue;
				}
				if bytes[idx] == b'*' && bytes[idx + 1] == b'/' {
					depth -= 1;
					idx += 2;
					continue;
				}
				idx += 1;
			}
			continue;
		}

		let start = idx;
		let mut prefix_len = 0_usize;
		if bytes[idx] == b'b' && idx + 1 < bytes.len() && matches!(bytes[idx + 1], b'"' | b'r') {
			prefix_len = 1;
		}

		let raw_start = idx + prefix_len;
		if raw_start < bytes.len() && bytes[raw_start] == b'r' {
			let mut cursor = raw_start + 1;
			while cursor < bytes.len() && bytes[cursor] == b'#' {
				cursor += 1;
			}
			if cursor < bytes.len() && bytes[cursor] == b'"' {
				let hash_count = cursor.saturating_sub(raw_start + 1);
				cursor += 1;
				while cursor < bytes.len() {
					if bytes[cursor] != b'"' {
						cursor += 1;
						continue;
					}
					let mut ok = true;
					for offset in 0..hash_count {
						let pos = cursor + 1 + offset;
						if pos >= bytes.len() || bytes[pos] != b'#' {
							ok = false;
							break;
						}
					}
					if ok {
						let end = cursor + 1 + hash_count;
						out.push((start, end));
						idx = end;
						break;
					}
					cursor += 1;
				}
				if idx != start {
					continue;
				}
			}
		}

		if raw_start < bytes.len() && bytes[raw_start] == b'"' {
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
					let end = cursor + 1;
					out.push((start, end));
					idx = end;
					break;
				}
				cursor += 1;
			}
			if idx != start {
				continue;
			}
		}

		if bytes[idx] == b'\'' {
			if is_lifetime_prefix(bytes, idx) {
				idx += 1;
				continue;
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
					let end = cursor + 1;
					out.push((idx, end));
					idx = end;
					break;
				}
				cursor += 1;
			}
			if idx != start {
				continue;
			}
		}

		idx += 1;
	}

	out
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
	matches!(rule, "RUST-STYLE-MOD-002" | "RUST-STYLE-MOD-003")
}

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
