//! The editor state machine: `apply(command, state, buffer) -> (state, SideEffect)`.
//!
//! This is the only place that mutates `EditorState` and `Buffer`.
//! It contains no I/O, no rendering calls, and no async operations.
//! Given the same inputs it always produces the same outputs (pure).
//!
//! Side effects (save, open file, etc.) are described by the returned
//! `SideEffect` value — the caller decides how to execute them.

use crate::editor::{
    buffer::Buffer,
    command::EditorCommand,
    state::{EditorState, Mode, Pos, Selection, VisualKind},
};

/// Something the editor wants to happen outside of its own state.
/// The caller is responsible for executing these — never the state machine.
#[derive(Debug, Clone, PartialEq)]
pub enum SideEffect {
    None,
    /// Persist the buffer to the path in `EditorState::path`.
    SaveFile,
    /// The buffer was modified — the typst compiler should recompile.
    BufferChanged,
}

/// Apply one command to `(state, buffer)` and return `(new_state, side_effect)`.
///
/// `state` is consumed and a new (or modified-in-place) state is returned to
/// keep the API honest: callers must not hold a stale reference.
pub fn apply<B: Buffer>(
    cmd: EditorCommand,
    mut state: EditorState,
    buf: &mut B,
) -> (EditorState, SideEffect) {
    use EditorCommand::*;
    use SideEffect::*;

    match cmd {
        // ── Text insertion ─────────────────────────────────────────────────
        Insert(text) => {
            let pos = state.cursor();
            buf.insert(pos.line, pos.col, &text);
            let new_col = pos.col + text.len();
            state.move_cursor_to(Pos::new(pos.line, new_col));
            state.is_dirty = true;
            (state, BufferChanged)
        }

        InsertNewline => {
            let pos = state.cursor();
            buf.split_line(pos.line, pos.col);
            state.move_cursor_to(Pos::new(pos.line + 1, 0));
            state.is_dirty = true;
            (state, BufferChanged)
        }

        PasteFromClipboard(text) => {
            let pos = state.cursor();
            buf.insert(pos.line, pos.col, &text);
            // Advance cursor to end of inserted text (single-line case).
            // For multi-line pastes the cursor lands on the last inserted line.
            let lines: Vec<&str> = text.split('\n').collect();
            let new_pos = if lines.len() == 1 {
                Pos::new(pos.line, pos.col + text.len())
            } else {
                let last_line = pos.line + lines.len() - 1;
                Pos::new(last_line, lines.last().map(|l| l.len()).unwrap_or(0))
            };
            state.move_cursor_to(new_pos);
            state.is_dirty = true;
            (state, BufferChanged)
        }

        // ── Single-character deletion ──────────────────────────────────────
        DeleteCharBefore => {
            let pos = state.cursor();
            if pos.col > 0 {
                let line_str = buf.line(pos.line);
                let prev_col = prev_char_boundary(line_str, pos.col);
                buf.delete_range(pos.line, prev_col, pos.col);
                state.move_cursor_to(Pos::new(pos.line, prev_col));
            } else if pos.line > 0 {
                let prev_line_len = buf.line(pos.line - 1).len();
                buf.join_with_next(pos.line - 1);
                state.move_cursor_to(Pos::new(pos.line - 1, prev_line_len));
            }
            state.is_dirty = true;
            (state, BufferChanged)
        }

        DeleteCharAt => {
            let pos = state.cursor();
            let line_len = buf.line(pos.line).len();
            if pos.col < line_len {
                let line_str = buf.line(pos.line);
                let next_col = next_char_boundary(line_str, pos.col);
                buf.delete_range(pos.line, pos.col, next_col);
            }
            let new_len = buf.line(pos.line).len();
            if pos.col > new_len {
                state.move_cursor_to(Pos::new(pos.line, new_len));
            }
            state.is_dirty = true;
            (state, BufferChanged)
        }

        // ── Line deletion ──────────────────────────────────────────────────
        DeleteLine => {
            let line = state.cursor().line;
            // Save to yank register (with trailing newline to mark as line-type).
            state.yank_register = buf.line(line).to_string() + "\n";

            let total = buf.line_count();
            if total == 1 {
                // Only line: clear it but keep the buffer non-empty.
                let len = buf.line(0).len();
                buf.delete_range(0, 0, len);
                state.move_cursor_to(Pos::new(0, 0));
            } else if line + 1 < total {
                // Not the last line: clear content, then absorb the next line.
                buf.delete_range(line, 0, buf.line(line).len());
                buf.join_with_next(line);
                let new_line = line.min(buf.line_count() - 1);
                state.move_cursor_to(Pos::new(new_line, 0));
            } else {
                // Last line (not the only one): join into the previous line.
                buf.delete_range(line, 0, buf.line(line).len());
                buf.join_with_next(line - 1);
                state.move_cursor_to(Pos::new(line - 1, 0));
            }

            state.mode = Mode::Normal;
            state.is_dirty = true;
            (state, BufferChanged)
        }

        DeleteToLineEnd => {
            let pos = state.cursor();
            let line_len = buf.line(pos.line).len();
            let deleted = buf.line(pos.line)[pos.col..].to_string();
            state.yank_register = deleted;
            buf.delete_range(pos.line, pos.col, line_len);
            // Clamp cursor.
            let new_len = buf.line(pos.line).len();
            if pos.col > new_len {
                let clamped = new_len.saturating_sub(1).max(0);
                state.move_cursor_to(Pos::new(pos.line, clamped));
            }
            state.is_dirty = true;
            (state, BufferChanged)
        }

        // ── Yank (copy) ────────────────────────────────────────────────────
        YankLine => {
            let line = state.cursor().line;
            state.yank_register = buf.line(line).to_string() + "\n";
            // Cursor stays; no buffer change.
            (state, None)
        }

        YankToLineEnd => {
            let pos = state.cursor();
            state.yank_register = buf.line(pos.line)[pos.col..].to_string();
            (state, None)
        }

        // ── Paste ──────────────────────────────────────────────────────────
        PasteAfter => {
            let pos = state.cursor();
            if state.yank_register.ends_with('\n') {
                // Line-type paste: insert as a new line below cursor.
                let text = state.yank_register.trim_end_matches('\n').to_string();
                let line_end = buf.line(pos.line).len();
                buf.split_line(pos.line, line_end);
                buf.insert(pos.line + 1, 0, &text);
                state.move_cursor_to(Pos::new(pos.line + 1, 0));
            } else if !state.yank_register.is_empty() {
                // Character-type paste: insert after cursor.
                let text = state.yank_register.clone();
                let line_str = buf.line(pos.line);
                let insert_col = if pos.col < line_str.len() {
                    next_char_boundary(line_str, pos.col)
                } else {
                    pos.col
                };
                buf.insert(pos.line, insert_col, &text);
                state.move_cursor_to(Pos::new(pos.line, insert_col + text.len()));
            }
            state.is_dirty = true;
            (state, BufferChanged)
        }

        PasteBefore => {
            let pos = state.cursor();
            if state.yank_register.ends_with('\n') {
                // Line-type paste: insert as a new line above cursor.
                let text = state.yank_register.trim_end_matches('\n').to_string();
                buf.split_line(pos.line, 0);
                buf.insert(pos.line, 0, &text);
                state.move_cursor_to(Pos::new(pos.line, 0));
            } else if !state.yank_register.is_empty() {
                // Character-type paste: insert at cursor.
                let text = state.yank_register.clone();
                buf.insert(pos.line, pos.col, &text);
                state.move_cursor_to(Pos::new(pos.line, pos.col + text.len()));
            }
            state.is_dirty = true;
            (state, BufferChanged)
        }

        // ── Insert-mode entry variants ─────────────────────────────────────
        EnterInsert => {
            state.mode = Mode::Insert;
            (state, None)
        }

        AppendAfterCursor => {
            let pos = state.cursor();
            let line_str = buf.line(pos.line);
            let new_col = if pos.col < line_str.len() {
                next_char_boundary(line_str, pos.col)
            } else {
                pos.col
            };
            state.move_cursor_to(Pos::new(pos.line, new_col));
            state.mode = Mode::Insert;
            (state, None)
        }

        InsertLineStart => {
            let line = state.cursor().line;
            state.move_cursor_to(Pos::new(line, 0));
            state.mode = Mode::Insert;
            (state, None)
        }

        InsertLineEnd => {
            let line = state.cursor().line;
            let end = buf.line(line).len();
            state.move_cursor_to(Pos::new(line, end));
            state.mode = Mode::Insert;
            (state, None)
        }

        OpenLineBelow => {
            let line = state.cursor().line;
            let line_end = buf.line(line).len();
            buf.split_line(line, line_end);
            state.move_cursor_to(Pos::new(line + 1, 0));
            state.mode = Mode::Insert;
            state.is_dirty = true;
            (state, BufferChanged)
        }

        OpenLineAbove => {
            let line = state.cursor().line;
            buf.split_line(line, 0);
            state.move_cursor_to(Pos::new(line, 0));
            state.mode = Mode::Insert;
            state.is_dirty = true;
            (state, BufferChanged)
        }

        // ── Change operators ───────────────────────────────────────────────
        ChangeLine => {
            let line = state.cursor().line;
            let len = buf.line(line).len();
            buf.delete_range(line, 0, len);
            state.move_cursor_to(Pos::new(line, 0));
            state.mode = Mode::Insert;
            state.is_dirty = true;
            (state, BufferChanged)
        }

        ChangeToLineEnd => {
            let pos = state.cursor();
            let line_len = buf.line(pos.line).len();
            buf.delete_range(pos.line, pos.col, line_len);
            state.mode = Mode::Insert;
            state.is_dirty = true;
            (state, BufferChanged)
        }

        // ── Helix-style selection ──────────────────────────────────────────
        SelectCurrentLine => {
            let line = state.cursor().line;
            let line_len = buf.line(line).len();
            state.selection = Selection {
                anchor: Pos::new(line, 0),
                cursor: Pos::new(line, line_len),
            };
            state.mode = Mode::Visual(VisualKind::Line);
            (state, None)
        }

        DeleteSelection => {
            match state.mode {
                Mode::Visual(VisualKind::Line) => {
                    let start_line = state.selection.start().line;
                    let end_line = state.selection.end().line;
                    // Yank selected lines.
                    let mut yanked = String::new();
                    for l in start_line..=end_line {
                        yanked.push_str(buf.line(l));
                        yanked.push('\n');
                    }
                    state.yank_register = yanked;
                    // Delete lines from bottom to top to keep indices valid.
                    for l in (start_line..=end_line).rev() {
                        let total = buf.line_count();
                        if total == 1 {
                            buf.delete_range(0, 0, buf.line(0).len());
                        } else if l + 1 < total {
                            buf.delete_range(l, 0, buf.line(l).len());
                            buf.join_with_next(l);
                        } else {
                            buf.delete_range(l, 0, buf.line(l).len());
                            buf.join_with_next(l - 1);
                        }
                    }
                    let new_line = start_line.min(buf.line_count() - 1);
                    state.move_cursor_to(Pos::new(new_line, 0));
                }
                Mode::Visual(VisualKind::Char) => {
                    let start = state.selection.start();
                    let end = state.selection.end();
                    if start.line == end.line {
                        let deleted = buf.line(start.line)[start.col..end.col].to_string();
                        state.yank_register = deleted;
                        buf.delete_range(start.line, start.col, end.col);
                        state.move_cursor_to(start);
                    }
                    // Multi-line char selection: defer to a future story.
                }
                _ => {
                    // In Normal mode, fall back to DeleteLine.
                    return apply(EditorCommand::DeleteLine, state, buf);
                }
            }
            state.mode = Mode::Normal;
            state.is_dirty = true;
            (state, BufferChanged)
        }

        YankSelection => {
            match state.mode {
                Mode::Visual(VisualKind::Line) => {
                    let start_line = state.selection.start().line;
                    let end_line = state.selection.end().line;
                    let mut yanked = String::new();
                    for l in start_line..=end_line {
                        yanked.push_str(buf.line(l));
                        yanked.push('\n');
                    }
                    state.yank_register = yanked;
                }
                Mode::Visual(VisualKind::Char) => {
                    let start = state.selection.start();
                    let end = state.selection.end();
                    if start.line == end.line {
                        state.yank_register = buf.line(start.line)[start.col..end.col].to_string();
                    }
                }
                _ => {
                    return apply(EditorCommand::YankLine, state, buf);
                }
            }
            state.mode = Mode::Normal;
            state.move_cursor_to(state.selection.start());
            (state, None)
        }

        // ── Cursor movement ────────────────────────────────────────────────
        MoveLeft => {
            let pos = state.cursor();
            let new_pos = if pos.col > 0 {
                let line_str = buf.line(pos.line);
                Pos::new(pos.line, prev_char_boundary(line_str, pos.col))
            } else if pos.line > 0 {
                Pos::new(pos.line - 1, buf.line(pos.line - 1).len())
            } else {
                pos
            };
            state.move_cursor_to(new_pos);
            (state, None)
        }

        MoveRight => {
            let pos = state.cursor();
            let line_str = buf.line(pos.line);
            let new_pos = if pos.col < line_str.len() {
                Pos::new(pos.line, next_char_boundary(line_str, pos.col))
            } else if pos.line + 1 < buf.line_count() {
                Pos::new(pos.line + 1, 0)
            } else {
                pos
            };
            state.move_cursor_to(new_pos);
            (state, None)
        }

        MoveUp => {
            let pos = state.cursor();
            if pos.line > 0 {
                let new_line = pos.line - 1;
                let new_col = pos.col.min(buf.line(new_line).len());
                state.move_cursor_to(Pos::new(new_line, new_col));
            }
            (state, None)
        }

        MoveDown => {
            let pos = state.cursor();
            if pos.line + 1 < buf.line_count() {
                let new_line = pos.line + 1;
                let new_col = pos.col.min(buf.line(new_line).len());
                state.move_cursor_to(Pos::new(new_line, new_col));
            }
            (state, None)
        }

        MoveStartOfLine => {
            let line = state.cursor().line;
            state.move_cursor_to(Pos::new(line, 0));
            (state, None)
        }

        MoveEndOfLine => {
            let pos = state.cursor();
            let end = buf.line(pos.line).len();
            state.move_cursor_to(Pos::new(pos.line, end));
            (state, None)
        }

        MoveStartOfDocument => {
            state.move_cursor_to(Pos::new(0, 0));
            (state, None)
        }

        MoveEndOfDocument => {
            let last = buf.line_count() - 1;
            let end = buf.line(last).len();
            state.move_cursor_to(Pos::new(last, end));
            (state, None)
        }

        MoveWordForward => {
            let pos = state.cursor();
            let new_pos = word_forward(pos, buf);
            state.move_cursor_to(new_pos);
            (state, None)
        }

        MoveWordBackward => {
            let pos = state.cursor();
            let new_pos = word_backward(pos, buf);
            state.move_cursor_to(new_pos);
            (state, None)
        }

        // ── Mode transitions ───────────────────────────────────────────────
        EnterNormal => {
            state.mode = Mode::Normal;
            // In Normal mode the cursor must not sit past the last character.
            let pos = state.cursor();
            let line_len = buf.line(pos.line).len();
            if pos.col > 0 && pos.col >= line_len {
                let prev = prev_char_boundary(buf.line(pos.line), line_len);
                state.move_cursor_to(Pos::new(pos.line, prev));
            }
            (state, None)
        }

        // OpenPalette is handled in the UI layer before reaching apply().
        OpenPalette => (state, None),
        Noop => (state, None),
    }
}

// ── Helper functions ──────────────────────────────────────────────────────────

/// Byte offset of the previous UTF-8 character boundary before `col`.
fn prev_char_boundary(s: &str, col: usize) -> usize {
    let mut b = col.saturating_sub(1);
    while b > 0 && !s.is_char_boundary(b) {
        b -= 1;
    }
    b
}

/// Byte offset of the next UTF-8 character boundary after `col`.
fn next_char_boundary(s: &str, col: usize) -> usize {
    let mut b = col + 1;
    while b <= s.len() && !s.is_char_boundary(b) {
        b += 1;
    }
    b
}

/// Move forward to the start of the next word (whitespace-delimited).
fn word_forward<B: Buffer>(pos: Pos, buf: &B) -> Pos {
    let mut line = pos.line;
    let mut col = pos.col;

    loop {
        let s = buf.line(line);
        let rest = &s[col..];
        let after_word = col + rest.chars()
            .take_while(|c| !c.is_whitespace())
            .map(|c| c.len_utf8())
            .sum::<usize>();
        let rest2 = &s[after_word..];
        let after_ws = after_word + rest2.chars()
            .take_while(|c| c.is_whitespace())
            .map(|c| c.len_utf8())
            .sum::<usize>();

        if after_ws > col || after_ws == s.len() && line + 1 >= buf.line_count() {
            return Pos::new(line, after_ws.min(s.len()));
        }
        if after_ws < s.len() {
            return Pos::new(line, after_ws);
        }
        if line + 1 < buf.line_count() {
            line += 1;
            col = 0;
        } else {
            return Pos::new(line, s.len());
        }
    }
}

/// Move backward to the start of the current or previous word.
fn word_backward<B: Buffer>(pos: Pos, buf: &B) -> Pos {
    let mut line = pos.line;
    let mut col = pos.col;

    loop {
        let s = buf.line(line);
        let chars_before: Vec<char> = s[..col].chars().collect();
        let ws_count = chars_before.iter().rev().take_while(|c| c.is_whitespace()).count();
        let after_ws_skip = chars_before.len() - ws_count;
        let word_start_idx = chars_before[..after_ws_skip]
            .iter()
            .rev()
            .take_while(|c| !c.is_whitespace())
            .count();
        let new_char_idx = after_ws_skip.saturating_sub(word_start_idx);
        let new_col = s.char_indices()
            .nth(new_char_idx)
            .map(|(b, _)| b)
            .unwrap_or(0);

        if new_col < col || new_col == 0 {
            return Pos::new(line, new_col);
        }
        if line > 0 {
            line -= 1;
            col = buf.line(line).len();
        } else {
            return Pos::new(0, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{buffer::InMemoryBuffer, command::EditorCommand::*};

    fn state() -> EditorState {
        EditorState::new()
    }

    fn buf(text: &str) -> InMemoryBuffer {
        InMemoryBuffer::from_text(text)
    }

    fn normal_state() -> EditorState {
        let mut s = EditorState::new();
        s.mode = Mode::Normal;
        s
    }

    #[test]
    fn insert_advances_cursor() {
        let mut b = buf("");
        let (s, effect) = apply(Insert("hello".into()), state(), &mut b);
        assert_eq!(b.line(0), "hello");
        assert_eq!(s.cursor(), Pos::new(0, 5));
        assert_eq!(effect, SideEffect::BufferChanged);
        assert!(s.is_dirty);
    }

    #[test]
    fn insert_newline_splits_line() {
        let mut b = buf("helloworld");
        let mut s = state();
        s.move_cursor_to(Pos::new(0, 5));
        let (s2, _) = apply(InsertNewline, s, &mut b);
        assert_eq!(b.line_count(), 2);
        assert_eq!(b.line(0), "hello");
        assert_eq!(b.line(1), "world");
        assert_eq!(s2.cursor(), Pos::new(1, 0));
    }

    #[test]
    fn delete_char_before_at_start_joins_lines() {
        let mut b = buf("hello\nworld");
        let mut s = state();
        s.move_cursor_to(Pos::new(1, 0));
        let (s2, _) = apply(DeleteCharBefore, s, &mut b);
        assert_eq!(b.line_count(), 1);
        assert_eq!(b.line(0), "helloworld");
        assert_eq!(s2.cursor(), Pos::new(0, 5));
    }

    #[test]
    fn delete_line_middle() {
        let mut b = buf("aaa\nbbb\nccc");
        let mut s = normal_state();
        s.move_cursor_to(Pos::new(1, 0));
        let (s2, _) = apply(DeleteLine, s, &mut b);
        assert_eq!(b.line_count(), 2);
        assert_eq!(b.line(0), "aaa");
        assert_eq!(b.line(1), "ccc");
        assert_eq!(s2.cursor().line, 1);
        assert_eq!(s2.yank_register, "bbb\n");
    }

    #[test]
    fn delete_line_last() {
        let mut b = buf("aaa\nbbb");
        let mut s = normal_state();
        s.move_cursor_to(Pos::new(1, 0));
        let (s2, _) = apply(DeleteLine, s, &mut b);
        assert_eq!(b.line_count(), 1);
        assert_eq!(b.line(0), "aaa");
        assert_eq!(s2.cursor().line, 0);
    }

    #[test]
    fn yank_and_paste_line() {
        let mut b = buf("aaa\nbbb");
        let mut s = normal_state();
        s.move_cursor_to(Pos::new(0, 0));
        let (s2, _) = apply(YankLine, s, &mut b);
        assert_eq!(s2.yank_register, "aaa\n");
        // Paste below line 0 → inserts as line 1.
        let (s3, _) = apply(PasteAfter, s2, &mut b);
        assert_eq!(b.line_count(), 3);
        assert_eq!(b.line(1), "aaa");
        assert_eq!(s3.cursor().line, 1);
    }

    #[test]
    fn open_line_below() {
        let mut b = buf("hello");
        let (s, _) = apply(OpenLineBelow, normal_state(), &mut b);
        assert_eq!(b.line_count(), 2);
        assert_eq!(b.line(1), "");
        assert_eq!(s.cursor(), Pos::new(1, 0));
        assert_eq!(s.mode, Mode::Insert);
    }

    #[test]
    fn open_line_above() {
        let mut b = buf("hello");
        let (s, _) = apply(OpenLineAbove, normal_state(), &mut b);
        assert_eq!(b.line_count(), 2);
        assert_eq!(b.line(0), "");
        assert_eq!(b.line(1), "hello");
        assert_eq!(s.cursor(), Pos::new(0, 0));
        assert_eq!(s.mode, Mode::Insert);
    }

    #[test]
    fn select_line_then_delete() {
        let mut b = buf("aaa\nbbb\nccc");
        let mut s = normal_state();
        s.move_cursor_to(Pos::new(1, 0));
        let (s2, _) = apply(SelectCurrentLine, s, &mut b);
        assert_eq!(s2.mode, Mode::Visual(VisualKind::Line));
        let (s3, _) = apply(DeleteSelection, s2, &mut b);
        assert_eq!(b.line_count(), 2);
        assert_eq!(b.line(0), "aaa");
        assert_eq!(b.line(1), "ccc");
        assert_eq!(s3.mode, Mode::Normal);
    }

    #[test]
    fn move_left_wraps_to_prev_line() {
        let mut b = buf("hello\nworld");
        let mut s = state();
        s.move_cursor_to(Pos::new(1, 0));
        let (s2, _) = apply(MoveLeft, s, &mut b);
        assert_eq!(s2.cursor(), Pos::new(0, 5));
    }

    #[test]
    fn move_right_wraps_to_next_line() {
        let mut b = buf("hello\nworld");
        let mut s = state();
        s.move_cursor_to(Pos::new(0, 5));
        let (s2, _) = apply(MoveRight, s, &mut b);
        assert_eq!(s2.cursor(), Pos::new(1, 0));
    }

    #[test]
    fn enter_normal_clamps_cursor() {
        let mut b = buf("hello");
        let mut s = state();
        s.move_cursor_to(Pos::new(0, 5));
        let (s2, _) = apply(EnterNormal, s, &mut b);
        assert_eq!(s2.cursor().col, 4);
        assert_eq!(s2.mode, Mode::Normal);
    }

    #[test]
    fn noop_is_identity() {
        let mut b = buf("hello");
        let s = state();
        let (s2, eff) = apply(Noop, s.clone(), &mut b);
        assert_eq!(s2.cursor(), s.cursor());
        assert_eq!(eff, SideEffect::None);
    }

    #[test]
    fn move_start_end_of_line() {
        let mut b = buf("hello world");
        let mut s = state();
        s.move_cursor_to(Pos::new(0, 5));
        let (s2, _) = apply(MoveStartOfLine, s, &mut b);
        assert_eq!(s2.cursor().col, 0);
        let (s3, _) = apply(MoveEndOfLine, s2, &mut b);
        assert_eq!(s3.cursor().col, 11);
    }
}
