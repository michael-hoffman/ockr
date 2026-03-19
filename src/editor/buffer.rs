//! Buffer trait and its in-memory implementation.
//!
//! The buffer is a content store for a single document. The `Buffer` trait is
//! intentionally minimal so that alternative implementations (e.g. a
//! rope-backed buffer, a CRDT buffer for Phase 3 collaborative editing) can be
//! swapped in without touching any editor logic.
//!
//! **Implementation note**: The in-memory buffer uses a `Vec<String>` of lines,
//! which is the simplest representation for Phase 1. A rope (e.g. `ropey`) will
//! be the right call once per-character random access inside large documents
//! becomes a bottleneck. The trait design already accommodates that swap.

/// Minimal interface for a text buffer.
///
/// All coordinates are 0-indexed: `(line, col)` where `col` is a byte offset
/// within the line's UTF-8 string. Callers are responsible for keeping
/// coordinates within bounds; implementations may panic on out-of-range access.
pub trait Buffer: Send + Sync + 'static {
    /// Total number of lines in the buffer. Always ≥ 1 (empty document = 1 empty line).
    fn line_count(&self) -> usize;

    /// Borrow the contents of `line` (0-indexed, no trailing newline).
    fn line(&self, line: usize) -> &str;

    /// Insert `text` at byte position `col` on `line`.
    ///
    /// Newlines inside `text` split the line accordingly.
    fn insert(&mut self, line: usize, col: usize, text: &str);

    /// Delete the byte range `col_start..col_end` on `line`.
    fn delete_range(&mut self, line: usize, col_start: usize, col_end: usize);

    /// Delete from `(line, col)` to end-of-line and join with the next line
    /// (i.e. delete the newline character that terminates `line`).
    /// No-op if `line` is the last line.
    fn join_with_next(&mut self, line: usize);

    /// Split `line` at byte offset `col` by inserting a newline.
    fn split_line(&mut self, line: usize, col: usize);

    /// Full document text (lines joined with `\n`).
    fn text(&self) -> String {
        let n = self.line_count();
        let mut out = String::new();
        for i in 0..n {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(self.line(i));
        }
        out
    }
}

/// Simple `Vec<String>` line buffer — the Phase 1 concrete implementation.
///
/// Not optimised for large documents. Suitable for notes up to a few thousand
/// lines; a rope-backed replacement arrives when benchmarks flag it.
pub struct InMemoryBuffer {
    lines: Vec<String>,
}

impl InMemoryBuffer {
    /// Create a buffer pre-populated with `text`.
    pub fn from_text(text: &str) -> Self {
        let lines: Vec<String> = if text.is_empty() {
            vec![String::new()]
        } else {
            text.split('\n').map(|l| l.to_owned()).collect()
        };
        Self { lines }
    }

    /// Create an empty buffer (single empty line, like a blank document).
    pub fn empty() -> Self {
        Self {
            lines: vec![String::new()],
        }
    }
}

impl Buffer for InMemoryBuffer {
    fn line_count(&self) -> usize {
        self.lines.len()
    }

    fn line(&self, line: usize) -> &str {
        &self.lines[line]
    }

    fn insert(&mut self, line: usize, col: usize, text: &str) {
        if !text.contains('\n') {
            self.lines[line].insert_str(col, text);
            return;
        }
        // Text contains newlines — split into parts and splice into self.lines.
        let original = self.lines[line].clone();
        let (before, after) = original.split_at(col);
        let parts: Vec<&str> = text.split('\n').collect();
        // Prepend what was before the insertion point to the first part.
        let first = format!("{}{}", before, parts[0]);
        // Append what was after to the last part.
        let last_idx = parts.len() - 1;
        let last = format!("{}{}", parts[last_idx], after);

        let mut new_lines: Vec<String> = Vec::with_capacity(parts.len());
        new_lines.push(first);
        for p in &parts[1..last_idx] {
            new_lines.push(p.to_string());
        }
        if parts.len() > 1 {
            new_lines.push(last);
        }

        self.lines.splice(line..=line, new_lines);
    }

    fn delete_range(&mut self, line: usize, col_start: usize, col_end: usize) {
        let s = &mut self.lines[line];
        s.drain(col_start..col_end);
    }

    fn join_with_next(&mut self, line: usize) {
        if line + 1 >= self.lines.len() {
            return;
        }
        let next = self.lines.remove(line + 1);
        self.lines[line].push_str(&next);
    }

    fn split_line(&mut self, line: usize, col: usize) {
        let tail = self.lines[line].split_off(col);
        self.lines.insert(line + 1, tail);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_buffer_has_one_line() {
        let b = InMemoryBuffer::empty();
        assert_eq!(b.line_count(), 1);
        assert_eq!(b.line(0), "");
    }

    #[test]
    fn from_text_splits_on_newlines() {
        let b = InMemoryBuffer::from_text("hello\nworld");
        assert_eq!(b.line_count(), 2);
        assert_eq!(b.line(0), "hello");
        assert_eq!(b.line(1), "world");
    }

    #[test]
    fn insert_simple() {
        let mut b = InMemoryBuffer::from_text("hello world");
        b.insert(0, 5, " brave");
        assert_eq!(b.line(0), "hello brave world");
    }

    #[test]
    fn insert_with_newline_splits_line() {
        let mut b = InMemoryBuffer::from_text("helloworld");
        b.insert(0, 5, "\n");
        assert_eq!(b.line_count(), 2);
        assert_eq!(b.line(0), "hello");
        assert_eq!(b.line(1), "world");
    }

    #[test]
    fn delete_range() {
        let mut b = InMemoryBuffer::from_text("hello world");
        b.delete_range(0, 5, 11);
        assert_eq!(b.line(0), "hello");
    }

    #[test]
    fn join_with_next() {
        let mut b = InMemoryBuffer::from_text("hello\nworld");
        b.join_with_next(0);
        assert_eq!(b.line_count(), 1);
        assert_eq!(b.line(0), "helloworld");
    }

    #[test]
    fn split_line() {
        let mut b = InMemoryBuffer::from_text("helloworld");
        b.split_line(0, 5);
        assert_eq!(b.line_count(), 2);
        assert_eq!(b.line(0), "hello");
        assert_eq!(b.line(1), "world");
    }

    #[test]
    fn text_roundtrip() {
        let src = "line one\nline two\nline three";
        let b = InMemoryBuffer::from_text(src);
        assert_eq!(b.text(), src);
    }
}
