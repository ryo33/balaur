//! Paths, URIs and positions, in the two spellings the seam needs.
//!
//! The language server counts lines from zero and columns in UTF-16 units;
//! scripts and the editor count lines from one and columns in characters.
//! Everything crossing the seam goes through here.

use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};

use lsp_types::Position;

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

/// A `file://` URI for `path`, escaped the way `eure-ls` escapes its own.
pub(crate) fn path_to_uri(path: &Path) -> String {
    let text = path.to_string_lossy();
    let mut escaped = String::with_capacity(text.len());
    for byte in text.bytes() {
        let keep = byte.is_ascii_graphic() && !matches!(byte, b'#' | b'?' | b'%');
        if keep {
            escaped.push(char::from(byte));
        } else {
            let _ = write!(escaped, "%{byte:02X}");
        }
    }
    if escaped.starts_with('/') {
        format!("file://{escaped}")
    } else {
        format!("file:///{escaped}")
    }
}

/// The path a `file://` URI names; a Windows drive keeps its letter.
pub(crate) fn uri_to_path(uri: &str) -> PathBuf {
    let raw = if let Some(rest) = uri.strip_prefix("file:///") {
        if rest.as_bytes().get(1) == Some(&b':') {
            rest.to_string()
        } else {
            format!("/{rest}")
        }
    } else {
        uri.strip_prefix("file://").unwrap_or(uri).to_string()
    };
    PathBuf::from(percent_decode(&raw))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let decoded = (bytes[i] == b'%' && i + 2 < bytes.len())
            .then(|| std::str::from_utf8(&bytes[i + 1..i + 3]).ok())
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        if let Some(byte) = decoded {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The text of 0-based `line`, without its terminator; empty past the end.
fn line_text(text: &str, line: usize) -> &str {
    text.split('\n')
        .nth(line)
        .map_or("", |l| l.strip_suffix('\r').unwrap_or(l))
}

/// UTF-16 units in the first `chars` characters of `line`.
fn utf16_of_chars(line: &str, chars: usize) -> u32 {
    line.chars().take(chars).map(|c| c.len_utf16() as u32).sum()
}

/// Characters in the first `units` UTF-16 units of `line`, clamped.
fn chars_of_utf16(line: &str, units: u32) -> usize {
    let mut seen = 0u32;
    let mut count = 0usize;
    for c in line.chars() {
        if seen >= units {
            break;
        }
        seen += c.len_utf16() as u32;
        count += 1;
    }
    count
}

/// The server's position for the editor's, against the document `text`.
pub(crate) fn to_lsp(text: &str, at: Span) -> Position {
    let line = at.line.saturating_sub(1);
    let character = utf16_of_chars(line_text(text, line), at.column.saturating_sub(1));
    Position {
        line: line as u32,
        character,
    }
}

/// The editor's position for the server's, against the document `text`.
pub(crate) fn from_lsp(text: &str, at: Position) -> Span {
    let line = at.line as usize;
    Span {
        line: line + 1,
        column: chars_of_utf16(line_text(text, line), at.character) + 1,
    }
}

/// Characters covered by `length` UTF-16 units starting at `start` on `line`.
pub(crate) fn char_length(text: &str, line: u32, start: u32, length: u32) -> usize {
    let line = line_text(text, line as usize);
    chars_of_utf16(line, start + length) - chars_of_utf16(line, start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_uri_round_trips_a_path_with_spaces_and_hashes() {
        let path = PathBuf::from("/tmp/my game/scene #1.eure");
        assert_eq!(uri_to_path(&path_to_uri(&path)), path);
    }

    #[test]
    fn positions_count_utf16_units_for_the_server_and_chars_for_the_editor() {
        let text = "a\n😀b c\n";
        let at = Span { line: 2, column: 3 };
        let lsp = to_lsp(text, at);
        assert_eq!((lsp.line, lsp.character), (1, 3));
        assert_eq!(from_lsp(text, lsp), at);
        assert_eq!(char_length(text, 1, 0, 3), 2);
    }

    #[test]
    fn absolute_folds_dot_and_dot_dot() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(absolute("./a/../b"), cwd.join("b"));
    }
}
