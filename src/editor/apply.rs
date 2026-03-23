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
    command::{EditorCommand, TextObjectKind},
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
                    let end   = state.selection.end();
                    // Yank via shared helper (handles both same-line and multi-line).
                    state.yank_register = yank_visual_char(start, end, buf);
                    if start.line == end.line {
                        // Same-line: delete slice [start.col, end.col).
                        buf.delete_range(start.line, start.col, end.col);
                    } else {
                        // Cross-line: trim start tail, trim end head, join middle lines.
                        let start_line_len = buf.line(start.line).len();
                        buf.delete_range(start.line, start.col, start_line_len);
                        buf.delete_range(end.line, 0, end.col);
                        for _ in start.line + 1..end.line {
                            buf.join_with_next(start.line);
                        }
                        buf.join_with_next(start.line);
                    }
                    state.move_cursor_to(start);
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
                    let end   = state.selection.end();
                    state.yank_register = yank_visual_char(start, end, buf);
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
        // All motions use `navigate` which extends the selection anchor in
        // Visual mode and collapses it to a point in Normal/Insert.
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
            navigate(&mut state, new_pos);
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
            navigate(&mut state, new_pos);
            (state, None)
        }

        MoveUp => {
            let pos = state.cursor();
            if pos.line > 0 {
                let new_line = pos.line - 1;
                let new_col = pos.col.min(buf.line(new_line).len());
                navigate(&mut state, Pos::new(new_line, new_col));
            }
            (state, None)
        }

        MoveDown => {
            let pos = state.cursor();
            if pos.line + 1 < buf.line_count() {
                let new_line = pos.line + 1;
                let new_col = pos.col.min(buf.line(new_line).len());
                navigate(&mut state, Pos::new(new_line, new_col));
            }
            (state, None)
        }

        MoveStartOfLine => {
            let line = state.cursor().line;
            navigate(&mut state, Pos::new(line, 0));
            (state, None)
        }

        MoveEndOfLine => {
            let pos = state.cursor();
            let end = buf.line(pos.line).len();
            navigate(&mut state, Pos::new(pos.line, end));
            (state, None)
        }

        MoveStartOfDocument => {
            navigate(&mut state, Pos::new(0, 0));
            (state, None)
        }

        MoveEndOfDocument => {
            let last = buf.line_count() - 1;
            let end = buf.line(last).len();
            navigate(&mut state, Pos::new(last, end));
            (state, None)
        }

        MoveWordForward => {
            let pos = state.cursor();
            let new_pos = word_forward(pos, buf);
            navigate(&mut state, new_pos);
            (state, None)
        }

        MoveWordBackward => {
            let pos = state.cursor();
            let new_pos = word_backward(pos, buf);
            navigate(&mut state, new_pos);
            (state, None)
        }

        MoveWordEnd => {
            let pos = state.cursor();
            let new_pos = word_end_forward(pos, buf);
            navigate(&mut state, new_pos);
            (state, None)
        }

        MoveWORDForward => {
            let pos = state.cursor();
            let new_pos = word_forward_whitespace(pos, buf);
            navigate(&mut state, new_pos);
            (state, None)
        }

        MoveWORDBackward => {
            let pos = state.cursor();
            let new_pos = word_backward_whitespace(pos, buf);
            navigate(&mut state, new_pos);
            (state, None)
        }

        MoveWORDEnd => {
            let pos = state.cursor();
            let new_pos = word_end_forward_whitespace(pos, buf);
            navigate(&mut state, new_pos);
            (state, None)
        }

        MoveFirstNonWhitespace => {
            let line = state.cursor().line;
            let s = buf.line(line);
            let col: usize = s
                .chars()
                .take_while(|c| c.is_whitespace())
                .map(|c| c.len_utf8())
                .sum();
            navigate(&mut state, Pos::new(line, col));
            (state, None)
        }

        ScrollHalfDown => {
            let pos = state.cursor();
            let total = buf.line_count();
            let new_line = (pos.line + 20).min(total - 1);
            let new_col = pos.col.min(buf.line(new_line).len());
            navigate(&mut state, Pos::new(new_line, new_col));
            (state, None)
        }

        ScrollHalfUp => {
            let pos = state.cursor();
            let new_line = pos.line.saturating_sub(20);
            let new_col = pos.col.min(buf.line(new_line).len());
            navigate(&mut state, Pos::new(new_line, new_col));
            (state, None)
        }

        ReplaceChar(ch) => {
            let pos = state.cursor();
            let line_str = buf.line(pos.line);
            if pos.col < line_str.len() {
                let next = next_char_boundary(line_str, pos.col);
                buf.delete_range(pos.line, pos.col, next);
                buf.insert(pos.line, pos.col, &ch);
                state.is_dirty = true;
                (state, BufferChanged)
            } else {
                (state, None)
            }
        }

        DeleteWordBefore => {
            let pos = state.cursor();
            let s = buf.line(pos.line).to_owned();
            if pos.col == 0 && pos.line > 0 {
                // At line start: join with previous line.
                let prev_len = buf.line(pos.line - 1).len();
                buf.join_with_next(pos.line - 1);
                state.move_cursor_to(Pos::new(pos.line - 1, prev_len));
                state.is_dirty = true;
                (state, BufferChanged)
            } else {
                let prefix = &s[..pos.col];
                // Skip trailing whitespace.
                let ws: usize = prefix
                    .chars()
                    .rev()
                    .take_while(|c| c.is_whitespace())
                    .map(|c| c.len_utf8())
                    .sum();
                let mid = pos.col - ws;
                // Skip word characters.
                let word: usize = s[..mid]
                    .chars()
                    .rev()
                    .take_while(|c| !c.is_whitespace())
                    .map(|c| c.len_utf8())
                    .sum();
                let del_start = mid - word;
                if del_start < pos.col {
                    buf.delete_range(pos.line, del_start, pos.col);
                    state.move_cursor_to(Pos::new(pos.line, del_start));
                    state.is_dirty = true;
                    (state, BufferChanged)
                } else {
                    (state, None)
                }
            }
        }

        CollapseSelection => {
            if let Mode::Visual(_) = state.mode {
                let cursor_pos = state.selection.cursor;
                state.mode = Mode::Normal;
                state.move_cursor_to(cursor_pos);
            }
            (state, None)
        }

        TrimSelection => {
            match state.mode {
                Mode::Normal => {
                    // In Normal mode behave like `^`: jump to first non-whitespace.
                    let line = state.cursor().line;
                    let s = buf.line(line);
                    let col: usize = s
                        .chars()
                        .take_while(|c| c.is_whitespace())
                        .map(|c| c.len_utf8())
                        .sum();
                    navigate(&mut state, Pos::new(line, col));
                }
                Mode::Visual(_) => {
                    let mut start = state.selection.start();
                    let mut end   = state.selection.end();

                    // ── Advance start past leading whitespace ──────────────
                    'fwd: loop {
                        let s = buf.line(start.line);
                        // Skip whitespace chars from start.col onwards.
                        let mut col = start.col;
                        for ch in s[col..].chars() {
                            if !ch.is_whitespace() {
                                break 'fwd;
                            }
                            col += ch.len_utf8();
                        }
                        // Entire remainder of this line is whitespace; move to next.
                        if start.line >= end.line {
                            // Selection is all whitespace — leave it alone.
                            break 'fwd;
                        }
                        start = Pos::new(start.line + 1, 0);
                    }
                    // Clamp start to actual content.
                    {
                        let s = buf.line(start.line);
                        let ws: usize = s[start.col..]
                            .chars()
                            .take_while(|c| c.is_whitespace())
                            .map(|c| c.len_utf8())
                            .sum();
                        start.col += ws;
                    }

                    // ── Retreat end past trailing whitespace ───────────────
                    'bwd: loop {
                        let s = buf.line(end.line);
                        // In Visual Line the "end col" is the full line length;
                        // in Visual Char it is exclusive, so clamp to len.
                        let bound = end.col.min(s.len());
                        let chars_before: Vec<char> = s[..bound].chars().collect();
                        let trim: usize = chars_before
                            .iter()
                            .rev()
                            .take_while(|c| c.is_whitespace())
                            .map(|c| c.len_utf8())
                            .sum();
                        if trim < bound {
                            end.col = bound - trim;
                            break 'bwd;
                        }
                        // Entire line is whitespace; retreat to previous line.
                        if end.line <= start.line {
                            break 'bwd;
                        }
                        end = Pos::new(end.line - 1, buf.line(end.line - 1).len());
                    }

                    // Rebuild selection: original anchor/cursor direction is
                    // preserved — whichever endpoint was the anchor stays the anchor.
                    let original_start_was_anchor =
                        state.selection.anchor <= state.selection.cursor;
                    state.selection = if original_start_was_anchor {
                        Selection { anchor: start, cursor: end }
                    } else {
                        Selection { anchor: end, cursor: start }
                    };
                    // Switch to Visual Char so the trimmed bounds make sense.
                    state.mode = Mode::Visual(VisualKind::Char);
                }
                Mode::Insert => {} // no-op in Insert mode
            }
            (state, None)
        }

        // ── Visual-mode entry ──────────────────────────────────────────────
        EnterVisualChar => {
            let pos = state.cursor();
            // Anchor at current position; cursor on the same char (single-char selection).
            state.selection = Selection {
                anchor: pos,
                cursor: pos,
            };
            state.mode = Mode::Visual(VisualKind::Char);
            (state, None)
        }

        EnterVisualLine => {
            let line = state.cursor().line;
            let line_len = buf.line(line).len();
            state.selection = Selection {
                anchor: Pos::new(line, 0),
                cursor: Pos::new(line, line_len),
            };
            state.mode = Mode::Visual(VisualKind::Line);
            (state, None)
        }

        EnterVisualBlock => {
            let pos = state.cursor();
            state.selection = Selection {
                anchor: pos,
                cursor: pos,
            };
            state.mode = Mode::Visual(VisualKind::Block);
            (state, None)
        }

        ReselectLastVisual => {
            if let Some((sel, kind)) = state.last_visual_selection.clone() {
                state.selection = sel;
                state.mode = Mode::Visual(kind);
            }
            (state, None)
        }

        // ── Change visual selection ────────────────────────────────────────
        ChangeSelection => {
            // Delete the selection content then enter Insert mode.
            let (mut new_state, effect) = apply(EditorCommand::DeleteSelection, state, buf);
            new_state.mode = Mode::Insert;
            (new_state, effect)
        }

        // ── Indent / dedent ────────────────────────────────────────────────
        IndentLines => {
            let (start_line, end_line) = visual_line_range(&state);
            for l in start_line..=end_line {
                buf.insert(l, 0, "  ");
            }
            state.is_dirty = true;
            (state, BufferChanged)
        }

        DedentLines => {
            let (start_line, end_line) = visual_line_range(&state);
            for l in start_line..=end_line {
                let line = buf.line(l).to_owned();
                let spaces = line.chars().take_while(|&c| c == ' ').count().min(2);
                if spaces > 0 {
                    buf.delete_range(l, 0, spaces);
                }
            }
            state.is_dirty = true;
            (state, BufferChanged)
        }

        // ── Find-char motions ──────────────────────────────────────────────
        FindChar(c) => {
            let pos = state.cursor();
            let line = buf.line(pos.line);
            // Search forward from pos.col+1
            if let Some(next_col) = find_char_forward(line, pos.col, c, false) {
                navigate(&mut state, Pos::new(pos.line, next_col));
            }
            (state, None)
        }
        FindCharBack(c) => {
            let pos = state.cursor();
            let line = buf.line(pos.line);
            if let Some(prev_col) = find_char_backward(line, pos.col, c, false) {
                navigate(&mut state, Pos::new(pos.line, prev_col));
            }
            (state, None)
        }
        TillChar(c) => {
            let pos = state.cursor();
            let line = buf.line(pos.line);
            if let Some(next_col) = find_char_forward(line, pos.col, c, true) {
                navigate(&mut state, Pos::new(pos.line, next_col));
            }
            (state, None)
        }
        TillCharBack(c) => {
            let pos = state.cursor();
            let line = buf.line(pos.line);
            if let Some(prev_col) = find_char_backward(line, pos.col, c, true) {
                navigate(&mut state, Pos::new(pos.line, prev_col));
            }
            (state, None)
        }

        // ── Paragraph navigation ───────────────────────────────────────────
        MoveParagraphBack => {
            let pos = state.cursor();
            let new_pos = paragraph_back(pos, buf);
            navigate(&mut state, new_pos);
            (state, None)
        }
        MoveParagraphForward => {
            let pos = state.cursor();
            let new_pos = paragraph_forward(pos, buf);
            navigate(&mut state, new_pos);
            (state, None)
        }

        // ── Miscellaneous ──────────────────────────────────────────────────
        SelectWholeFile => {
            let last_line = buf.line_count() - 1;
            let last_col  = buf.line(last_line).len();
            state.selection = Selection {
                anchor: Pos::new(0, 0),
                cursor: Pos::new(last_line, last_col),
            };
            state.mode = Mode::Visual(VisualKind::Char);
            (state, None)
        }

        MatchBracket => {
            let pos = state.cursor();
            let line = buf.line(pos.line);
            // Find a bracket at or after the cursor on the same line.
            let bracket_pos = line[pos.col..].char_indices().find_map(|(i, c)| {
                if "(){}[]<>".contains(c) { Some((pos.col + i, c)) } else { Option::None }
            });
            if let Some((bcol, bchar)) = bracket_pos {
                let (open, close, forward) = match bchar {
                    '(' => ('(', ')', true),
                    ')' => ('(', ')', false),
                    '{' => ('{', '}', true),
                    '}' => ('{', '}', false),
                    '[' => ('[', ']', true),
                    ']' => ('[', ']', false),
                    '<' => ('<', '>', true),
                    '>' => ('<', '>', false),
                    _   => return (state, None),
                };
                let search_from = Pos::new(pos.line, bcol);
                let maybe = if forward {
                    obj_surround(buf, search_from, open, close, false)
                        .map(|(_, e)| Pos::new(e.line, e.col.saturating_sub(close.len_utf8())))
                } else {
                    obj_surround(buf, search_from, open, close, false)
                        .map(|(s, _)| s)
                };
                if let Some(dest) = maybe {
                    navigate(&mut state, dest);
                }
            }
            (state, None)
        }

        SwitchCase => {
            match state.mode {
                Mode::Visual(VisualKind::Char) => {
                    let start = state.selection.start();
                    let end   = state.selection.end();
                    if start.line == end.line {
                        let chunk = buf.line(start.line)[start.col..end.col].to_owned();
                        let toggled = toggle_case(&chunk);
                        buf.delete_range(start.line, start.col, end.col);
                        buf.insert(start.line, start.col, &toggled);
                    } else {
                        // Multi-line: toggle each line's slice.
                        let start_chunk = buf.line(start.line)[start.col..].to_owned();
                        let toggled_s = toggle_case(&start_chunk);
                        let start_end = buf.line(start.line).len();
                        buf.delete_range(start.line, start.col, start_end);
                        buf.insert(start.line, start.col, &toggled_s);
                        for l in start.line + 1..end.line {
                            let chunk = buf.line(l).to_owned();
                            let toggled = toggle_case(&chunk);
                            buf.delete_range(l, 0, chunk.len());
                            buf.insert(l, 0, &toggled);
                        }
                        let end_chunk = buf.line(end.line)[..end.col].to_owned();
                        let toggled_e = toggle_case(&end_chunk);
                        buf.delete_range(end.line, 0, end.col);
                        buf.insert(end.line, 0, &toggled_e);
                    }
                    state.mode = Mode::Normal;
                    state.move_cursor_to(start);
                    state.is_dirty = true;
                    (state, BufferChanged)
                }
                _ => {
                    // Normal mode: toggle char at cursor.
                    let pos = state.cursor();
                    let line = buf.line(pos.line);
                    if pos.col < line.len() {
                        let ch = line[pos.col..].chars().next().unwrap();
                        let new_ch = if ch.is_uppercase() {
                            ch.to_lowercase().to_string()
                        } else {
                            ch.to_uppercase().to_string()
                        };
                        let end_col = pos.col + ch.len_utf8();
                        buf.delete_range(pos.line, pos.col, end_col);
                        buf.insert(pos.line, pos.col, &new_ch);
                        // Advance cursor one char (Helix behaviour).
                        let line2 = buf.line(pos.line);
                        let next = next_char_boundary(line2, pos.col);
                        state.move_cursor_to(Pos::new(pos.line, next.min(line2.len())));
                        state.is_dirty = true;
                        (state, BufferChanged)
                    } else {
                        (state, None)
                    }
                }
            }
        }

        ScrollPageDown => {
            let pos = state.cursor();
            let new_line = (pos.line + 40).min(buf.line_count() - 1);
            let new_col = pos.col.min(buf.line(new_line).len());
            navigate(&mut state, Pos::new(new_line, new_col));
            (state, None)
        }

        ScrollPageUp => {
            let pos = state.cursor();
            let new_line = pos.line.saturating_sub(40);
            let new_col = pos.col.min(buf.line(new_line).len());
            navigate(&mut state, Pos::new(new_line, new_col));
            (state, None)
        }

        RepeatLastChange => {
            if let Some(seq) = state.last_change.clone() {
                for cmd in seq {
                    let prev = std::mem::take(&mut state);
                    let (new_state, _) = apply(cmd, prev, buf);
                    state = new_state;
                }
                state.is_dirty = true;
                (state, BufferChanged)
            } else {
                (state, None)
            }
        }

        // ── Text object selection ──────────────────────────────────────────
        SelectObject { inner, kind } => {
            let pos = state.cursor();
            let maybe = match kind {
                TextObjectKind::Word      => obj_word(buf.line(pos.line), pos.col, inner, false)
                    .map(|(s, e)| (Pos::new(pos.line, s), Pos::new(pos.line, e))),
                TextObjectKind::WORD      => obj_word(buf.line(pos.line), pos.col, inner, true)
                    .map(|(s, e)| (Pos::new(pos.line, s), Pos::new(pos.line, e))),
                TextObjectKind::Paragraph => obj_paragraph(buf, pos, inner),
                TextObjectKind::Paren     => obj_surround(buf, pos, '(', ')', inner),
                TextObjectKind::Brace     => obj_surround(buf, pos, '{', '}', inner),
                TextObjectKind::Bracket | TextObjectKind::TypstContent
                                          => obj_surround(buf, pos, '[', ']', inner),
                TextObjectKind::Angle     => obj_surround(buf, pos, '<', '>', inner),
                TextObjectKind::DoubleQuote => obj_quote(buf.line(pos.line), pos.col, '"', inner)
                    .map(|(s, e)| (Pos::new(pos.line, s), Pos::new(pos.line, e))),
                TextObjectKind::SingleQuote => obj_quote(buf.line(pos.line), pos.col, '\'', inner)
                    .map(|(s, e)| (Pos::new(pos.line, s), Pos::new(pos.line, e))),
                TextObjectKind::Backtick    => obj_quote(buf.line(pos.line), pos.col, '`', inner)
                    .map(|(s, e)| (Pos::new(pos.line, s), Pos::new(pos.line, e))),
                TextObjectKind::InlineMath  => obj_quote(buf.line(pos.line), pos.col, '$', inner)
                    .map(|(s, e)| (Pos::new(pos.line, s), Pos::new(pos.line, e))),
            };
            if let Some((start, end)) = maybe {
                state.selection = Selection { anchor: start, cursor: end };
                state.mode = Mode::Visual(VisualKind::Char);
            }
            (state, None)
        }

        // ── Mode transitions ───────────────────────────────────────────────
        EnterNormal => {
            // Save visual selection for `gv` before leaving Visual mode.
            if let Mode::Visual(kind) = state.mode {
                state.last_visual_selection = Some((state.selection, kind));
                // Return cursor to the start of the selection.
                let start = state.selection.start();
                state.move_cursor_to(start);
            }
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

// ── Visual selection helpers ──────────────────────────────────────────────────

/// Returns (start_line, end_line) of the current selection, or the cursor line
/// if not in Visual mode.
fn visual_line_range(state: &EditorState) -> (usize, usize) {
    match state.mode {
        Mode::Visual(_) => (state.selection.start().line, state.selection.end().line),
        _ => {
            let l = state.cursor().line;
            (l, l)
        }
    }
}

// ── Helper functions ──────────────────────────────────────────────────────────

/// Move the cursor to `pos`.
/// In Visual mode the anchor stays fixed (the selection extends).
/// In Normal / Insert mode the selection collapses to a point.
#[inline]
fn navigate(state: &mut EditorState, pos: Pos) {
    if matches!(state.mode, Mode::Visual(_)) {
        state.selection.cursor = pos;
    } else {
        state.move_cursor_to(pos);
    }
}

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

/// Move to the end of the current word (or next word if at a word boundary).
///
/// Always advances at least one character so repeated presses make progress.
/// Lands on the last byte of the last character in the word.
fn word_end_forward<B: Buffer>(pos: Pos, buf: &B) -> Pos {
    let total = buf.line_count();

    // Advance at least one character first.
    let (mut line, mut col) = {
        let s = buf.line(pos.line);
        if pos.col < s.len() {
            (pos.line, next_char_boundary(s, pos.col))
        } else if pos.line + 1 < total {
            (pos.line + 1, 0)
        } else {
            return pos;
        }
    };

    loop {
        let s = buf.line(line);

        // Skip leading whitespace.
        let ws: usize = s[col..]
            .chars()
            .take_while(|c| c.is_whitespace())
            .map(|c| c.len_utf8())
            .sum();
        col += ws;

        if col >= s.len() {
            // Line exhausted — move to next.
            if line + 1 < total {
                line += 1;
                col = 0;
                continue;
            }
            return Pos::new(line, s.len().saturating_sub(1));
        }

        // Walk through non-whitespace chars; land on the last one.
        let mut last = col;
        let mut cur = col;
        for c in s[col..].chars() {
            if c.is_whitespace() {
                break;
            }
            last = cur;
            cur += c.len_utf8();
        }
        return Pos::new(line, last);
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

// ── Selection helpers ─────────────────────────────────────────────────────────

/// Collect the text covered by a Visual Char selection into a `String`.
///
/// Handles both single-line and multi-line selections.  The returned string
/// does **not** end with `\n` unless the selection crosses a line boundary.
fn yank_visual_char<B: Buffer>(start: crate::editor::state::Pos, end: crate::editor::state::Pos, buf: &B) -> String {
    if start.line == end.line {
        buf.line(start.line)[start.col..end.col].to_string()
    } else {
        let mut out = buf.line(start.line)[start.col..].to_string();
        out.push('\n');
        for l in start.line + 1..end.line {
            out.push_str(buf.line(l));
            out.push('\n');
        }
        out.push_str(&buf.line(end.line)[..end.col]);
        out
    }
}

// ── WORD motions (whitespace-only boundaries; W / B / E) ──────────────────────

/// Move forward one WORD: skip non-whitespace, then skip whitespace.
/// Identical boundary rule to `word_forward` — both only split on whitespace —
/// but provided as a distinct function so the intent is explicit in the dispatch.
fn word_forward_whitespace<B: Buffer>(pos: Pos, buf: &B) -> Pos {
    word_forward(pos, buf)
}

/// Move backward one WORD: find the start of the previous whitespace-delimited run.
fn word_backward_whitespace<B: Buffer>(pos: Pos, buf: &B) -> Pos {
    word_backward(pos, buf)
}

/// Move to the end of the current/next WORD (whitespace-delimited).
fn word_end_forward_whitespace<B: Buffer>(pos: Pos, buf: &B) -> Pos {
    word_end_forward(pos, buf)
}

// ── Find-char helpers ─────────────────────────────────────────────────────────

/// Find the next occurrence of `c` after `col` on `line`.
/// If `till`, returns the position just before `c`; otherwise on `c`.
fn find_char_forward(line: &str, col: usize, c: char, till: bool) -> Option<usize> {
    // Start scanning one byte past the current char.
    let start = if col < line.len() {
        col + line[col..].chars().next().map(|ch| ch.len_utf8()).unwrap_or(1)
    } else {
        return None;
    };
    for (i, ch) in line[start..].char_indices() {
        if ch == c {
            let found = start + i;
            return if till {
                // Position just before: last char boundary before `found`.
                if found > 0 { Some(prev_char_boundary(line, found)) } else { None }
            } else {
                Some(found)
            };
        }
    }
    None
}

/// Find the previous occurrence of `c` before `col` on `line`.
/// If `till`, returns the position just after `c`.
fn find_char_backward(line: &str, col: usize, c: char, till: bool) -> Option<usize> {
    if col == 0 { return None; }
    let scan = &line[..col];
    // Walk backwards through chars.
    let chars: Vec<(usize, char)> = scan.char_indices().collect();
    for &(i, ch) in chars.iter().rev() {
        if ch == c {
            return if till {
                Some(i + ch.len_utf8())
            } else {
                Some(i)
            };
        }
    }
    None
}

// ── Paragraph navigation helpers ──────────────────────────────────────────────

/// Move to the start of the previous paragraph (blank-line boundary).
fn paragraph_back<B: Buffer>(pos: Pos, buf: &B) -> Pos {
    let is_blank = |l: usize| buf.line(l).trim().is_empty();
    let mut line = pos.line;

    // If already at top, stay.
    if line == 0 { return Pos::new(0, 0); }

    // Step over any blank lines directly above.
    while line > 0 && is_blank(line) {
        line -= 1;
    }
    // Now step over the text block above us.
    while line > 0 && !is_blank(line - 1) {
        line -= 1;
    }
    Pos::new(line, 0)
}

/// Move to the start of the next paragraph (blank-line boundary).
fn paragraph_forward<B: Buffer>(pos: Pos, buf: &B) -> Pos {
    let total = buf.line_count();
    let is_blank = |l: usize| buf.line(l).trim().is_empty();
    let mut line = pos.line;

    if line + 1 >= total { return Pos::new(line, buf.line(line).len()); }

    // Step over non-blank lines first (skip current block).
    while line + 1 < total && !is_blank(line) {
        line += 1;
    }
    // Step over blank lines to reach the next block.
    while line + 1 < total && is_blank(line) {
        line += 1;
    }
    Pos::new(line, 0)
}

// ── Case helper ───────────────────────────────────────────────────────────────

fn toggle_case(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_uppercase() { c.to_lowercase().collect::<String>() }
            else { c.to_uppercase().collect::<String>() }
        })
        .collect()
}

// ── Text object finders ───────────────────────────────────────────────────────
//
// All single-line helpers return `Option<(start_col, end_col)>` where
// `end_col` is the **exclusive** byte past the last selected character —
// matching the convention used by `delete_range` and `yank_visual_char`.

/// Returns `true` if `c` is a "word" character (alphanumeric or `_`).
#[inline]
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Select inner/around **word** or **WORD** on a single line.
///
/// `whitespace_word`: if `true` treat any non-whitespace run as the "WORD".
///
/// Strategy:
/// - If cursor is on a word char: expand to full word.
/// - If cursor is on whitespace: select the whitespace run.
/// - `around` for a word adds trailing whitespace (or leading if at line end).
fn obj_word(line: &str, col: usize, inner: bool, whitespace_word: bool) -> Option<(usize, usize)> {
    if line.is_empty() {
        return None;
    }
    let col = col.min(line.len().saturating_sub(1));
    let is_target: fn(char) -> bool = if whitespace_word {
        |c: char| !c.is_whitespace()
    } else {
        is_word_char
    };

    // What kind of run is the cursor on?
    let cur_char = line[col..].chars().next().unwrap_or(' ');
    let on_target = is_target(cur_char);

    if on_target {
        // Scan left for start of run.
        let mut start = col;
        for (i, c) in line[..col].char_indices().rev() {
            if is_target(c) { start = i; } else { break; }
        }
        // Scan right for end of run (exclusive).
        let mut end = col;
        for (i, c) in line[col..].char_indices() {
            if is_target(c) { end = col + i + c.len_utf8(); } else { break; }
        }
        if inner {
            Some((start, end))
        } else {
            // Add trailing whitespace; fall back to leading.
            let trail: usize = line[end..].chars()
                .take_while(|c| c.is_whitespace())
                .map(|c| c.len_utf8())
                .sum();
            if trail > 0 {
                Some((start, end + trail))
            } else {
                let lead: usize = line[..start].chars().rev()
                    .take_while(|c| c.is_whitespace())
                    .map(|c| c.len_utf8())
                    .sum();
                Some((start - lead, end))
            }
        }
    } else {
        // Cursor on whitespace — select the whitespace run.
        let mut start = col;
        for (i, c) in line[..col].char_indices().rev() {
            if c.is_whitespace() { start = i; } else { break; }
        }
        let mut end = col;
        for (i, c) in line[col..].char_indices() {
            if c.is_whitespace() { end = col + i + c.len_utf8(); } else { break; }
        }
        Some((start, end))
    }
}

/// Find enclosing matched bracket pair across lines.
///
/// Scans backward from `pos` for `open` and forward for `close`, tracking
/// nesting depth so inner pairs are skipped.
/// Returns `(start, end)` in exclusive-end convention.
fn obj_surround<B: Buffer>(
    buf: &B,
    pos: Pos,
    open: char,
    close: char,
    inner: bool,
) -> Option<(Pos, Pos)> {
    // ── Scan backward for the outer open ─────────────────────────────────────
    let (open_line, open_col) = {
        let mut depth: i32 = 0;
        let mut found = None;
        'bwd: for ln in (0..=pos.line).rev() {
            let s = buf.line(ln);
            let scan_end = if ln == pos.line { pos.col } else { s.len() };
            // Collect char positions so we can iterate in reverse.
            let chars: Vec<(usize, char)> = s[..scan_end].char_indices().collect();
            for &(ci, ch) in chars.iter().rev() {
                if ch == close {
                    depth += 1;
                } else if ch == open {
                    if depth == 0 {
                        found = Some((ln, ci));
                        break 'bwd;
                    }
                    depth -= 1;
                }
            }
        }
        found?
    };

    // ── Scan forward for the matching close ──────────────────────────────────
    let (close_line, close_col) = {
        let mut depth: i32 = 0;
        let mut found = None;
        'fwd: for ln in pos.line..buf.line_count() {
            let s = buf.line(ln);
            let scan_start = if ln == pos.line { pos.col } else { 0 };
            for (rel, ch) in s[scan_start..].char_indices() {
                let ci = scan_start + rel;
                if ch == open {
                    depth += 1;
                } else if ch == close {
                    if depth == 0 {
                        found = Some((ln, ci));
                        break 'fwd;
                    }
                    depth -= 1;
                }
            }
        }
        found?
    };

    if inner {
        // Content after `open` up to (but not including) `close`.
        let after_open = open_col + open.len_utf8();
        let (start_line, start_col) = if after_open <= buf.line(open_line).len() {
            (open_line, after_open)
        } else {
            (open_line + 1, 0)
        };
        Some((Pos::new(start_line, start_col), Pos::new(close_line, close_col)))
    } else {
        // Include both delimiters.
        let after_close = close_col + close.len_utf8();
        Some((Pos::new(open_line, open_col), Pos::new(close_line, after_close)))
    }
}

/// Find an enclosing symmetric quote pair (`"`, `'`, `` ` ``, `$`) on one line.
///
/// Scans the line for all occurrences of `quote`, forms consecutive pairs,
/// and returns the pair whose range contains `col`.
fn obj_quote(line: &str, col: usize, quote: char, inner: bool) -> Option<(usize, usize)> {
    // Collect all positions of unescaped `quote` on this line.
    let positions: Vec<usize> = {
        let mut v = Vec::new();
        let mut prev_backslash = false;
        for (i, c) in line.char_indices() {
            if c == quote && !prev_backslash {
                v.push(i);
            }
            prev_backslash = c == '\\' && !prev_backslash;
        }
        v
    };

    // Pair them up: [0,1], [2,3], …
    for pair in positions.chunks(2) {
        if let [s, e] = *pair {
            // col must be inside or on the delimiters.
            if col >= s && col <= e {
                return if inner {
                    Some((s + quote.len_utf8(), e))
                } else {
                    Some((s, e + quote.len_utf8()))
                };
            }
        }
    }
    None
}

/// Find enclosing paragraph (blank-line-delimited).
fn obj_paragraph<B: Buffer>(buf: &B, pos: Pos, inner: bool) -> Option<(Pos, Pos)> {
    let total = buf.line_count();
    let is_blank = |l: usize| buf.line(l).trim().is_empty();

    // Walk up to find the first non-blank line of this paragraph.
    let mut start_line = pos.line;
    // If cursor is already on a blank line, there is no paragraph.
    if is_blank(start_line) {
        return None;
    }
    while start_line > 0 && !is_blank(start_line - 1) {
        start_line -= 1;
    }

    // Walk down to find the last non-blank line.
    let mut end_line = pos.line;
    while end_line + 1 < total && !is_blank(end_line + 1) {
        end_line += 1;
    }

    let end_col = buf.line(end_line).len();

    if inner {
        Some((Pos::new(start_line, 0), Pos::new(end_line, end_col)))
    } else {
        // `around`: include one following blank line (or preceding if none after).
        if end_line + 1 < total && is_blank(end_line + 1) {
            let blank_end = buf.line(end_line + 1).len();
            Some((Pos::new(start_line, 0), Pos::new(end_line + 1, blank_end)))
        } else if start_line > 0 && is_blank(start_line - 1) {
            Some((Pos::new(start_line - 1, 0), Pos::new(end_line, end_col)))
        } else {
            Some((Pos::new(start_line, 0), Pos::new(end_line, end_col)))
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

    // ── Story 13: Visual mode ─────────────────────────────────────────────────

    fn visual_char_state() -> EditorState {
        let mut s = EditorState::new();
        s.mode = Mode::Visual(VisualKind::Char);
        s
    }

    #[test]
    fn enter_visual_char_anchors_at_cursor() {
        let mut b = buf("hello");
        let mut s = normal_state();
        s.move_cursor_to(Pos::new(0, 2));
        let (s2, _) = apply(EnterVisualChar, s, &mut b);
        assert_eq!(s2.mode, Mode::Visual(VisualKind::Char));
        assert_eq!(s2.selection.anchor, Pos::new(0, 2));
        assert_eq!(s2.selection.cursor, Pos::new(0, 2));
    }

    #[test]
    fn enter_visual_line_selects_whole_line() {
        let mut b = buf("hello world");
        let mut s = normal_state();
        s.move_cursor_to(Pos::new(0, 3));
        let (s2, _) = apply(EnterVisualLine, s, &mut b);
        assert_eq!(s2.mode, Mode::Visual(VisualKind::Line));
        assert_eq!(s2.selection.anchor.col, 0);
        assert_eq!(s2.selection.cursor.col, 11);
    }

    #[test]
    fn motion_extends_selection_in_visual_mode() {
        let mut b = buf("hello");
        let mut s = visual_char_state();
        s.selection = Selection { anchor: Pos::new(0, 0), cursor: Pos::new(0, 0) };
        let (s2, _) = apply(MoveRight, s, &mut b);
        // Anchor stays, cursor moves.
        assert_eq!(s2.selection.anchor, Pos::new(0, 0));
        assert_eq!(s2.selection.cursor, Pos::new(0, 1));
        assert_eq!(s2.mode, Mode::Visual(VisualKind::Char));
    }

    #[test]
    fn motion_collapses_selection_in_normal_mode() {
        let mut b = buf("hello");
        let mut s = normal_state();
        s.selection = Selection { anchor: Pos::new(0, 0), cursor: Pos::new(0, 0) };
        let (s2, _) = apply(MoveRight, s, &mut b);
        assert_eq!(s2.selection.anchor, Pos::new(0, 1));
        assert_eq!(s2.selection.cursor, Pos::new(0, 1));
    }

    #[test]
    fn escape_from_visual_saves_last_selection() {
        let mut b = buf("hello");
        let mut s = visual_char_state();
        s.selection = Selection { anchor: Pos::new(0, 1), cursor: Pos::new(0, 3) };
        let (s2, _) = apply(EnterNormal, s, &mut b);
        assert_eq!(s2.mode, Mode::Normal);
        assert!(s2.last_visual_selection.is_some());
        let (saved_sel, kind) = s2.last_visual_selection.as_ref().unwrap();
        assert_eq!(kind, &VisualKind::Char);
        assert_eq!(saved_sel.anchor, Pos::new(0, 1));
    }

    #[test]
    fn reselect_last_visual() {
        let mut b = buf("hello");
        let mut s = visual_char_state();
        s.selection = Selection { anchor: Pos::new(0, 1), cursor: Pos::new(0, 4) };
        let (s2, _) = apply(EnterNormal, s, &mut b); // saves last visual
        let (s3, _) = apply(ReselectLastVisual, s2, &mut b);
        assert_eq!(s3.mode, Mode::Visual(VisualKind::Char));
        assert_eq!(s3.selection.anchor, Pos::new(0, 1));
        assert_eq!(s3.selection.cursor, Pos::new(0, 4));
    }

    #[test]
    fn indent_lines_adds_two_spaces() {
        let mut b = buf("aaa\nbbb\nccc");
        let mut s = visual_char_state();
        s.selection = Selection { anchor: Pos::new(0, 0), cursor: Pos::new(1, 2) };
        let (_, effect) = apply(IndentLines, s, &mut b);
        assert_eq!(b.line(0), "  aaa");
        assert_eq!(b.line(1), "  bbb");
        assert_eq!(b.line(2), "ccc"); // untouched
        assert_eq!(effect, SideEffect::BufferChanged);
    }

    #[test]
    fn dedent_lines_removes_two_spaces() {
        let mut b = buf("  aaa\n  bbb");
        let mut s = visual_char_state();
        s.selection = Selection { anchor: Pos::new(0, 0), cursor: Pos::new(1, 0) };
        let (_, _) = apply(DedentLines, s, &mut b);
        assert_eq!(b.line(0), "aaa");
        assert_eq!(b.line(1), "bbb");
    }

    #[test]
    fn change_selection_enters_insert() {
        let mut b = buf("hello world");
        let mut s = visual_char_state();
        s.selection = Selection { anchor: Pos::new(0, 0), cursor: Pos::new(0, 5) };
        let (s2, _) = apply(ChangeSelection, s, &mut b);
        assert_eq!(s2.mode, Mode::Insert);
    }

    // ── Story: helix parity — new motion tests ────────────────────────────────

    #[test]
    fn move_word_end_advances_to_word_end() {
        let mut b = buf("hello world foo");
        let mut s = normal_state();
        s.move_cursor_to(Pos::new(0, 0));
        let (s2, _) = apply(MoveWordEnd, s, &mut b);
        // "hello" ends at col 4
        assert_eq!(s2.cursor(), Pos::new(0, 4));
    }

    #[test]
    fn move_word_end_skips_whitespace_to_next_word() {
        let mut b = buf("hello world");
        let mut s = normal_state();
        s.move_cursor_to(Pos::new(0, 4)); // at end of "hello"
        let (s2, _) = apply(MoveWordEnd, s, &mut b);
        // Should jump to end of "world" = col 10
        assert_eq!(s2.cursor(), Pos::new(0, 10));
    }

    #[test]
    fn move_first_non_whitespace_skips_leading_spaces() {
        let mut b = buf("   hello");
        let s = normal_state();
        let (s2, _) = apply(MoveFirstNonWhitespace, s, &mut b);
        assert_eq!(s2.cursor(), Pos::new(0, 3));
    }

    #[test]
    fn move_first_non_whitespace_no_indent() {
        let mut b = buf("hello");
        let s = normal_state();
        let (s2, _) = apply(MoveFirstNonWhitespace, s, &mut b);
        assert_eq!(s2.cursor(), Pos::new(0, 0));
    }

    #[test]
    fn replace_char_replaces_and_stays_normal() {
        let mut b = buf("abc");
        let s = normal_state();
        let (s2, effect) = apply(ReplaceChar("X".into()), s, &mut b);
        assert_eq!(b.line(0), "Xbc");
        assert_eq!(s2.cursor(), Pos::new(0, 0));
        assert_eq!(effect, SideEffect::BufferChanged);
    }

    #[test]
    fn delete_word_before_removes_previous_word() {
        let mut b = buf("hello world");
        let mut s = EditorState::new();
        s.mode = Mode::Insert;
        s.move_cursor_to(Pos::new(0, 11)); // end of "world"
        let (s2, effect) = apply(DeleteWordBefore, s, &mut b);
        assert_eq!(b.line(0), "hello ");
        assert_eq!(s2.cursor(), Pos::new(0, 6));
        assert_eq!(effect, SideEffect::BufferChanged);
    }

    #[test]
    fn collapse_selection_exits_visual_to_cursor() {
        let mut b = buf("hello world");
        let mut s = visual_char_state();
        s.selection = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(0, 5),
        };
        let (s2, _) = apply(CollapseSelection, s, &mut b);
        assert_eq!(s2.mode, Mode::Normal);
        assert_eq!(s2.cursor(), Pos::new(0, 5));
    }

    #[test]
    fn scroll_half_down_moves_cursor_20_lines() {
        let text = (0..40).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let mut b = InMemoryBuffer::from_text(&text);
        let s = normal_state();
        let (s2, _) = apply(ScrollHalfDown, s, &mut b);
        assert_eq!(s2.cursor().line, 20);
    }

    #[test]
    fn scroll_half_up_moves_cursor_20_lines() {
        let text = (0..40).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let mut b = InMemoryBuffer::from_text(&text);
        let mut s = normal_state();
        s.move_cursor_to(Pos::new(30, 0));
        let (s2, _) = apply(ScrollHalfUp, s, &mut b);
        assert_eq!(s2.cursor().line, 10);
    }

    // ── WORD motion tests ──────────────────────────────────────────────────────

    #[test]
    fn move_word_forward_whitespace_advances_over_punct() {
        // "foo.bar baz" — W skips "foo.bar" as a single WORD, then "baz"
        let mut b = buf("foo.bar baz");
        let s = normal_state(); // col 0
        let (s2, _) = apply(MoveWORDForward, s, &mut b);
        // Next WORD starts at col 8 ("baz")
        assert_eq!(s2.cursor(), Pos::new(0, 8));
    }

    #[test]
    fn move_word_backward_whitespace_jumps_over_punct() {
        let mut b = buf("foo.bar baz");
        let mut s = normal_state();
        s.move_cursor_to(Pos::new(0, 9)); // inside "baz"
        let (s2, _) = apply(MoveWORDBackward, s, &mut b);
        // Previous WORD starts at col 8 ("baz")
        assert_eq!(s2.cursor(), Pos::new(0, 8));
    }

    #[test]
    fn move_word_end_whitespace_lands_on_last_char_of_word() {
        let mut b = buf("foo.bar baz");
        let s = normal_state(); // col 0
        let (s2, _) = apply(MoveWORDEnd, s, &mut b);
        // End of first WORD "foo.bar" is col 6
        assert_eq!(s2.cursor(), Pos::new(0, 6));
    }

    // ── TrimSelection tests ────────────────────────────────────────────────────

    #[test]
    fn trim_selection_normal_moves_to_first_nonws() {
        let mut b = buf("   hello world");
        let s = normal_state(); // col 0
        let (s2, _) = apply(TrimSelection, s, &mut b);
        assert_eq!(s2.cursor(), Pos::new(0, 3));
        assert_eq!(s2.mode, Mode::Normal);
    }

    #[test]
    fn trim_selection_visual_line_strips_leading_and_trailing_ws() {
        // Line 0: "  hello  " → content at 2..7 (col 2 to col 7, exclusive)
        let mut b = buf("  hello  ");
        let mut s = visual_char_state();
        // Select the whole line manually via anchor/cursor
        s.selection = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(0, 9), // past end of "  hello  "
        };
        let (s2, _) = apply(TrimSelection, s, &mut b);
        // Should trim to col 2 (start of "hello") and col 7 (end of "hello")
        let sel_start = s2.selection.start();
        let sel_end   = s2.selection.end();
        assert_eq!(sel_start, Pos::new(0, 2));
        assert_eq!(sel_end,   Pos::new(0, 7));
        assert_eq!(s2.mode, Mode::Visual(VisualKind::Char));
    }

    #[test]
    fn trim_selection_visual_already_trimmed_is_noop() {
        let mut b = buf("hello");
        let mut s = visual_char_state();
        s.selection = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(0, 5),
        };
        let (s2, _) = apply(TrimSelection, s, &mut b);
        assert_eq!(s2.selection.start(), Pos::new(0, 0));
        assert_eq!(s2.selection.end(),   Pos::new(0, 5));
    }

    // ── Text object tests ──────────────────────────────────────────────────────

    fn select_obj(text: &str, col: usize, inner: bool, kind: TextObjectKind)
        -> Option<(usize, usize)>
    {
        let mut b = buf(text);
        let mut s = normal_state();
        s.move_cursor_to(Pos::new(0, col));
        let (s2, _) = apply(SelectObject { inner, kind }, s, &mut b);
        if let Mode::Visual(VisualKind::Char) = s2.mode {
            Some((s2.selection.start().col, s2.selection.end().col))
        } else {
            None
        }
    }

    #[test]
    fn obj_inner_word_from_middle() {
        // "hello world" — cursor on 'l' at col 2
        assert_eq!(select_obj("hello world", 2, true, TextObjectKind::Word), Some((0, 5)));
    }

    #[test]
    fn obj_around_word_adds_trailing_space() {
        // "hello world" — cursor on 'h' (col 0), around = word + space
        assert_eq!(select_obj("hello world", 0, false, TextObjectKind::Word), Some((0, 6)));
    }

    #[test]
    fn obj_inner_word_whitespace_cursor() {
        // "foo  bar" — cursor on first space (col 3), inner = whitespace run
        assert_eq!(select_obj("foo  bar", 3, true, TextObjectKind::Word), Some((3, 5)));
    }

    #[test]
    fn obj_inner_paren() {
        // "(hello)" — cursor on 'e' (col 2), inner = "hello" (cols 1-5 exclusive)
        assert_eq!(select_obj("(hello)", 2, true, TextObjectKind::Paren), Some((1, 6)));
    }

    #[test]
    fn obj_around_paren() {
        // "(hello)" — around = whole string "(hello)" 0-7 exclusive
        assert_eq!(select_obj("(hello)", 2, false, TextObjectKind::Paren), Some((0, 7)));
    }

    #[test]
    fn obj_inner_double_quote() {
        // `say "hi" now` — cursor inside quotes (col 5)
        assert_eq!(select_obj("say \"hi\" now", 5, true, TextObjectKind::DoubleQuote), Some((5, 7)));
    }

    #[test]
    fn obj_around_double_quote() {
        assert_eq!(select_obj("say \"hi\" now", 5, false, TextObjectKind::DoubleQuote), Some((4, 8)));
    }

    #[test]
    fn obj_inner_inline_math() {
        // "$x + 1$" — cursor inside (col 2), inner = "x + 1" (cols 1-6 exclusive)
        assert_eq!(select_obj("$x + 1$", 2, true, TextObjectKind::InlineMath), Some((1, 6)));
    }

    #[test]
    fn obj_around_inline_math() {
        assert_eq!(select_obj("$x + 1$", 2, false, TextObjectKind::InlineMath), Some((0, 7)));
    }

    #[test]
    fn obj_inner_brace_nested() {
        // "{a{b}c}" — cursor on 'a' (col 1), inner = "a{b}c" cols 1-6
        assert_eq!(select_obj("{a{b}c}", 1, true, TextObjectKind::Brace), Some((1, 6)));
    }

    #[test]
    fn obj_select_object_enters_visual_char() {
        let mut b = buf("hello world");
        let mut s = normal_state();
        s.move_cursor_to(Pos::new(0, 2));
        let (s2, _) = apply(SelectObject { inner: true, kind: TextObjectKind::Word }, s, &mut b);
        assert!(matches!(s2.mode, Mode::Visual(VisualKind::Char)));
    }

    // ── Find-char tests ────────────────────────────────────────────────────────

    #[test]
    fn find_char_moves_to_next_occurrence() {
        let mut b = buf("hello world");
        let mut s = normal_state(); // col 0
        let (s2, _) = apply(FindChar('o'), s, &mut b);
        assert_eq!(s2.cursor(), Pos::new(0, 4)); // 'o' in "hello"
    }

    #[test]
    fn find_char_no_match_stays_put() {
        let mut b = buf("hello");
        let s = normal_state();
        let (s2, _) = apply(FindChar('z'), s, &mut b);
        assert_eq!(s2.cursor(), Pos::new(0, 0));
    }

    #[test]
    fn till_char_stops_before_target() {
        let mut b = buf("hello world");
        let mut s = normal_state();
        let (s2, _) = apply(TillChar('o'), s, &mut b);
        assert_eq!(s2.cursor(), Pos::new(0, 3)); // just before 'o' at col 4
    }

    #[test]
    fn find_char_back_moves_to_prev_occurrence() {
        let mut b = buf("hello world");
        let mut s = normal_state();
        s.move_cursor_to(Pos::new(0, 7)); // 'o' in "world"
        let (s2, _) = apply(FindCharBack('l'), s, &mut b);
        assert_eq!(s2.cursor(), Pos::new(0, 3)); // 'l' in "hello" at col 3
    }

    // ── Paragraph navigation tests ─────────────────────────────────────────────

    #[test]
    fn paragraph_forward_jumps_to_next_block() {
        let mut b = buf("line1\nline2\n\nline3\nline4");
        let mut s = normal_state(); // col 0, line 0
        let (s2, _) = apply(MoveParagraphForward, s, &mut b);
        assert_eq!(s2.cursor().line, 3); // "line3"
    }

    #[test]
    fn paragraph_back_jumps_to_start_of_block() {
        let mut b = buf("line1\nline2\n\nline3\nline4");
        let mut s = normal_state();
        s.move_cursor_to(Pos::new(4, 0)); // "line4"
        let (s2, _) = apply(MoveParagraphBack, s, &mut b);
        assert_eq!(s2.cursor().line, 3); // start of second block
    }

    // ── SwitchCase tests ───────────────────────────────────────────────────────

    #[test]
    fn switch_case_normal_mode_uppercases() {
        let mut b = buf("hello");
        let s = normal_state();
        let (s2, _) = apply(SwitchCase, s, &mut b);
        assert_eq!(b.line(0), "Hello");
        assert_eq!(s2.cursor().col, 1); // advanced one char
    }

    #[test]
    fn switch_case_visual_toggles_selection() {
        let mut b = buf("Hello World");
        let mut s = visual_char_state();
        s.selection = Selection { anchor: Pos::new(0, 0), cursor: Pos::new(0, 5) };
        let (_, _) = apply(SwitchCase, s, &mut b);
        assert_eq!(&b.line(0)[..5], "hELLO");
    }

    // ── SelectWholeFile tests ──────────────────────────────────────────────────

    #[test]
    fn select_whole_file_enters_visual_char() {
        let mut b = buf("hello\nworld");
        let s = normal_state();
        let (s2, _) = apply(SelectWholeFile, s, &mut b);
        assert!(matches!(s2.mode, Mode::Visual(VisualKind::Char)));
        assert_eq!(s2.selection.start(), Pos::new(0, 0));
        assert_eq!(s2.selection.end(), Pos::new(1, 5));
    }

    #[test]
    fn obj_no_match_stays_normal() {
        // No quotes on the line — mode should stay Normal.
        let mut b = buf("hello world");
        let mut s = normal_state();
        let (s2, _) = apply(SelectObject { inner: true, kind: TextObjectKind::DoubleQuote }, s, &mut b);
        assert_eq!(s2.mode, Mode::Normal);
    }
}
