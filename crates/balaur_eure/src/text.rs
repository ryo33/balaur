//! Paths and positions, in the two spellings the seam needs.
//!
//! The language's queries count byte offsets into a file; scripts and the
//! editor count lines from one and columns in characters. Everything
//! crossing the seam goes through here.

use std::path::{Component, PathBuf};

/// A 1-based line and a 1-based character column, the editor's spelling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub line: usize,
    pub column: usize,
}

/// `path` made absolute against the working directory, with `.` and `..`
/// folded away, so two spellings of one file compare equal as prefixes.
pub(crate) fn absolute(path: &str) -> PathBuf {
    let joined = std::path::absolute(path).unwrap_or_else(|_| PathBuf::from(path));
    let mut out = PathBuf::new();
    for part in joined.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The editor's position of byte `offset` in `text`, clamped to the end.
pub(crate) fn span_at(text: &str, offset: usize) -> Span {
    let offset = offset.min(text.len());
    let before = &text[..floor_char_boundary(text, offset)];
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    Span {
        line: before.matches('\n').count() + 1,
        column: before[line_start..].chars().count() + 1,
    }
}

/// The byte offset of the editor's position in `text`, clamped to the end
/// of its line and to the end of the text.
pub(crate) fn offset_of(text: &str, at: Span) -> usize {
    let mut start = 0;
    for (n, line) in text.split('\n').enumerate() {
        if n + 1 == at.line {
            return start
                + line
                    .char_indices()
                    .nth(at.column.saturating_sub(1))
                    .map_or(line.len(), |(i, _)| i);
        }
        start += line.len() + 1;
    }
    text.len()
}

/// Characters between two byte offsets of `text`.
pub(crate) fn char_length(text: &str, start: usize, end: usize) -> usize {
    let start = floor_char_boundary(text, start.min(text.len()));
    let end = floor_char_boundary(text, end.min(text.len()));
    text[start..end.max(start)].chars().count()
}

fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut i = index.min(text.len());
    while !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_count_bytes_for_the_queries_and_chars_for_the_editor() {
        let text = "a\n😀b c\n";
        let at = Span { line: 2, column: 3 };
        let offset = offset_of(text, at);
        assert_eq!(offset, 2 + "😀b".len());
        assert_eq!(span_at(text, offset), at);
        assert_eq!(char_length(text, 2, offset), 2);
    }

    #[test]
    fn a_column_past_the_line_end_stops_at_the_newline() {
        let text = "ab\ncd";
        assert_eq!(offset_of(text, Span { line: 1, column: 9 }), 2);
        assert_eq!(offset_of(text, Span { line: 9, column: 1 }), 5);
        assert_eq!(span_at(text, 99), Span { line: 2, column: 3 });
    }

    #[test]
    fn absolute_folds_dot_and_dot_dot() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(absolute("./a/../b"), cwd.join("b"));
    }
}
