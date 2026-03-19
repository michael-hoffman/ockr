//! Text editor pane — keyboard-driven, pure-state-machine backed.
//!
//! ## Architecture
//!
//! `EditorPane` owns:
//! - An `InMemoryBuffer` (text content).
//! - An `EditorState` (cursor, mode, dirty flag, yank register).
//! - An `undo_history` stack of `(buffer_snapshot, cursor_pos)` pairs.
//! - A `CompilerHandle` for the background compiler thread.
//!
//! Every keystroke → `EditorCommand` → `apply(cmd, state, buf)` → `(state, SideEffect)`.
//! Undo is handled outside the pure state machine (snapshots stored here).
//!
//! ## Key map (Story 07 — Vi/Helix hybrid)
//!
//! Normal mode:
//!   h/j/k/l     — movement          w/b          — word motion
//!   0/$         — line start/end    g/G          — doc start/end
//!   x           — select line (Helix `x`)
//!   d           — delete selection / delete line (dd analogue)
//!   y           — yank line (yy)    p/P          — paste after/before
//!   i/a/I/A     — enter insert      o/O          — open line below/above
//!   c/C         — change line / to EOL
//!   u           — undo
//!   Escape      — enter Normal
//!
//! Insert mode:
//!   printable   — insert char       Backspace    — delete before
//!   Enter       — newline           Escape       — Normal
//!   arrows / Home / End / Delete    Cmd-S        — save
//!   Cmd-V       — paste clipboard

use std::path::PathBuf;

use gpui::{
    App, ClipboardItem, Context, Entity, FocusHandle, Focusable, KeyDownEvent, MouseButton,
    MouseDownEvent, Render, Window, div, prelude::*, px,
};

use crate::actions::SaveFile;
use crate::compiler::{CompileRequest, CompilerHandle};
use crate::editor::buffer::Buffer as _;
use crate::editor::{
    apply::{apply, SideEffect},
    buffer::InMemoryBuffer,
    command::EditorCommand,
    state::{EditorState, Mode, Pos, Selection, VisualKind},
};
use crate::ui::preview::PreviewPane;
use crate::ui::theme;
use crate::vault::VaultFile;

// ── View ──────────────────────────────────────────────────────────────────────

pub struct EditorPane {
    pub focus_handle: FocusHandle,
    state: EditorState,
    buffer: InMemoryBuffer,
    /// Undo stack: each entry is (full buffer text, cursor position before mutation).
    /// Capped at 200 entries.
    undo_history: Vec<(String, Pos)>,
    compiler: Option<CompilerHandle>,
    preview: Option<Entity<PreviewPane>>,
    vault_root: Option<PathBuf>,
}

impl EditorPane {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            state: EditorState::new(),
            buffer: InMemoryBuffer::empty(),
            undo_history: Vec::new(),
            compiler: None,
            preview: None,
            vault_root: None,
        }
    }

    pub fn set_compiler(&mut self, handle: CompilerHandle, preview: Entity<PreviewPane>) {
        self.compiler = Some(handle);
        self.preview = Some(preview);
    }

    pub fn open_file(
        &mut self,
        file: &VaultFile,
        vault_root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_file_no_focus(file, vault_root, cx);
        self.focus_handle.focus(window);
    }

    pub fn open_file_no_focus(
        &mut self,
        file: &VaultFile,
        vault_root: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let text = std::fs::read_to_string(&file.abs_path).unwrap_or_default();
        self.buffer = InMemoryBuffer::from_text(&text);
        self.state = EditorState::new();
        self.state.path = Some(file.abs_path.clone());
        self.state.is_dirty = false;
        self.vault_root = Some(vault_root);
        self.undo_history.clear();
        self.trigger_compile();
        cx.notify();
    }

    fn trigger_compile(&self) {
        if let Some(compiler) = &self.compiler {
            compiler.send(CompileRequest {
                source: self.buffer.text(),
                vault_root: self.vault_root.clone(),
            });
        }
    }

    fn save(&mut self) {
        if let Some(path) = &self.state.path {
            let _ = std::fs::write(path, self.buffer.text());
            self.state.is_dirty = false;
        }
    }

    /// Push a snapshot onto the undo stack before a mutating operation.
    fn push_undo(&mut self) {
        let snapshot = (self.buffer.text(), self.state.cursor());
        // Deduplicate: don't push if identical to the most recent snapshot.
        if self.undo_history.last().map(|(t, _)| t == &snapshot.0).unwrap_or(false) {
            return;
        }
        self.undo_history.push(snapshot);
        if self.undo_history.len() > 200 {
            self.undo_history.remove(0);
        }
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let k = &event.keystroke;

        // ── Global shortcuts handled before the state machine ─────────────
        // Cmd-S: save (also bound as GPUI action but catching here for robustness).
        if k.modifiers.platform && k.key == "s" {
            self.save();
            cx.notify();
            return;
        }

        // Cmd-V: paste from OS clipboard.
        if k.modifiers.platform && k.key == "v" {
            if let Some(item) = cx.read_from_clipboard() {
                if let Some(text) = item.text() {
                    self.push_undo();
                    let prev = std::mem::take(&mut self.state);
                    let (new_state, _) = apply(EditorCommand::PasteFromClipboard(text), prev, &mut self.buffer);
                    self.state = new_state;
                    self.trigger_compile();
                    cx.notify();
                }
            }
            return;
        }

        // Cmd-C / Cmd-X: copy / cut selection or current line to OS clipboard.
        if k.modifiers.platform && (k.key == "c" || k.key == "x") {
            let text = match self.state.mode {
                Mode::Visual(VisualKind::Line) => {
                    let start = self.state.selection.start().line;
                    let end = self.state.selection.end().line;
                    (start..=end).map(|l| self.buffer.line(l)).collect::<Vec<_>>().join("\n")
                }
                _ => self.buffer.line(self.state.cursor().line).to_string(),
            };
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            if k.key == "x" {
                // Cut: delete line.
                self.push_undo();
                let prev = std::mem::take(&mut self.state);
                let (new_state, _) = apply(EditorCommand::DeleteLine, prev, &mut self.buffer);
                self.state = new_state;
                self.trigger_compile();
            }
            cx.notify();
            return;
        }

        // Skip held repeats in Normal/Visual modes.
        if event.is_held && self.state.mode != Mode::Insert {
            return;
        }

        // ── Undo (u in Normal/Visual) ─────────────────────────────────────
        if k.key == "u" && !k.modifiers.platform && !k.modifiers.control
            && self.state.mode != Mode::Insert
        {
            if let Some((text, pos)) = self.undo_history.pop() {
                self.buffer = InMemoryBuffer::from_text(&text);
                self.state.mode = Mode::Normal;
                self.state.move_cursor_to(pos);
                self.state.is_dirty = true;
                self.trigger_compile();
                cx.notify();
            }
            return;
        }

        let cmd = keystroke_to_command(event, &self.state);
        if cmd == EditorCommand::Noop {
            return;
        }

        // Snapshot before any mutating command.
        if is_buffer_mutating(&cmd) {
            self.push_undo();
        }

        let prev_state = std::mem::take(&mut self.state);
        let (new_state, effect) = apply(cmd, prev_state, &mut self.buffer);
        self.state = new_state;

        match effect {
            SideEffect::BufferChanged => {
                self.trigger_compile();
                cx.notify();
            }
            SideEffect::SaveFile => {
                self.save();
                cx.notify();
            }
            SideEffect::None => {
                cx.notify();
            }
        }
    }

    /// Best-effort click-to-cursor: positions the cursor to the approximate
    /// line and column given a window-relative mouse position.
    ///
    /// Uses fixed metrics: 16 px padding (`p_4`), 20 px line height, 8.4 px
    /// per character (Menlo at `text_sm` ≈ 14 px). Exact hit-testing requires
    /// GPUI's text layout pipeline and will be refined in a future story.
    fn handle_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let padding = 16.0_f32;
        let line_height = 20.0_f32;
        let char_width = 8.4_f32;

        let raw_y = f32::from(event.position.y) - padding;
        let line = if raw_y < 0.0 {
            0
        } else {
            (raw_y / line_height) as usize
        };
        let line = line.min(self.buffer.line_count().saturating_sub(1));

        let raw_x = (f32::from(event.position.x) - padding).max(0.0);
        let col_approx = (raw_x / char_width) as usize;
        // Clamp col to a valid UTF-8 boundary.
        let line_str = self.buffer.line(line);
        let col = byte_offset_for_char(line_str, col_approx);

        self.state.move_cursor_to(Pos::new(line, col));
        self.state.mode = Mode::Normal;
        cx.notify();
    }
}

impl Focusable for EditorPane {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for EditorPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let line_count = self.buffer.line_count();
        let cursor = self.state.cursor();
        let mode = self.state.mode;
        let selection = self.state.selection;

        let mut line_elements = Vec::with_capacity(line_count);
        for i in 0..line_count {
            let text = self.buffer.line(i).to_string();
            let in_selection = is_line_in_visual_selection(i, mode, &selection);
            line_elements.push(render_line(i, text, cursor, mode, in_selection));
        }

        let mode_label = match mode {
            Mode::Insert => "INSERT",
            Mode::Normal => "NORMAL",
            Mode::Visual(_) => "VISUAL",
        };
        let mode_color = match mode {
            Mode::Insert => gpui::rgb(theme::MODE_INSERT),
            Mode::Normal => gpui::rgb(theme::MODE_NORMAL),
            Mode::Visual(_) => gpui::rgb(theme::MODE_VISUAL),
        };

        // o| logo mark — dark variant matching the app icon.
        let logo = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_px()
            .mr_2()
            .child(
                div()
                    .font_family("Menlo")
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_xs()
                    .text_color(gpui::rgb(theme::TEXT))
                    .child("o"),
            )
            .child(
                div()
                    .w(gpui::px(2.0))
                    .h(gpui::px(11.0))
                    .bg(gpui::rgb(theme::OCHRE)),
            );

        let status_bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .px_3()
            .py_1()
            .bg(gpui::rgb(theme::BG_BASE))
            .border_t_1()
            .border_color(gpui::rgb(theme::BORDER_SUBTLE))
            .text_xs()
            .child(logo)
            .child(
                div()
                    .text_color(mode_color)
                    .font_family("Menlo")
                    .child(mode_label),
            )
            .child(
                div()
                    .text_color(gpui::rgb(theme::TEXT_FAINT))
                    .font_family("Menlo")
                    .child(format!("{}:{}", cursor.line + 1, cursor.col + 1)),
            )
            .child(if self.state.is_dirty {
                div()
                    .text_color(gpui::rgb(theme::OCHRE))
                    .child("●")
                    .into_any_element()
            } else {
                div().into_any_element()
            });

        div()
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(gpui::rgb(theme::BG_PANEL))
            .on_action(cx.listener(|this, _: &SaveFile, _window, cx| {
                this.save();
                cx.notify();
            }))
            .on_key_down(cx.listener(Self::handle_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::handle_mouse_down))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .p_4()
                    .flex()
                    .flex_col()
                    .children(line_elements),
            )
            .child(status_bar)
    }
}

// ── Line rendering ────────────────────────────────────────────────────────────

fn is_line_in_visual_selection(line: usize, mode: Mode, sel: &Selection) -> bool {
    match mode {
        Mode::Visual(VisualKind::Line) => {
            let start = sel.start().line;
            let end = sel.end().line;
            line >= start && line <= end
        }
        Mode::Visual(VisualKind::Char) => {
            let start = sel.start().line;
            let end = sel.end().line;
            line >= start && line <= end
        }
        _ => false,
    }
}

fn render_line(
    line_idx: usize,
    text: String,
    cursor: Pos,
    mode: Mode,
    in_selection: bool,
) -> impl gpui::IntoElement {
    let line_height = px(20.0);

    // Selection highlight: entire-line background for Visual(Line).
    let sel_bg = gpui::rgb(theme::OCHRE_DIM);
    let line_bg = if in_selection && line_idx != cursor.line {
        Some(sel_bg)
    } else {
        None
    };

    if line_idx != cursor.line {
        let mut row = div()
            .min_h(line_height)
            .text_color(gpui::rgb(theme::TEXT_MUTED))
            .text_sm()
            .font_family("Menlo")
            .child(if text.is_empty() { " ".to_string() } else { text });
        if let Some(bg) = line_bg {
            row = row.bg(bg);
        }
        return row.into_any_element();
    }

    // Cursor line: split at the cursor byte offset.
    let col = cursor.col.min(text.len());
    let before = text[..col].to_string();

    let char_end = text[col..]
        .char_indices()
        .nth(1)
        .map(|(b, _)| col + b)
        .unwrap_or(text.len());

    let cursor_char = if col < text.len() {
        text[col..char_end].to_string()
    } else {
        " ".to_string()
    };

    let after = if char_end < text.len() {
        text[char_end..].to_string()
    } else {
        String::new()
    };

    let cursor_color = match mode {
        Mode::Insert => gpui::rgb(theme::MODE_INSERT),
        Mode::Normal => gpui::rgb(theme::MODE_NORMAL),
        Mode::Visual(_) => gpui::rgb(theme::MODE_VISUAL),
    };

    // Insert mode → bar cursor (left border); Normal/Visual → block (filled bg).
    let cursor_cell = if mode == Mode::Insert {
        div()
            .text_color(gpui::rgb(theme::TEXT_MUTED))
            .border_l_2()
            .border_color(cursor_color)
            .child(cursor_char)
    } else {
        div()
            .text_color(gpui::rgb(theme::CURSOR_FG))
            .bg(cursor_color)
            .child(cursor_char)
    };

    let mut row = div()
        .min_h(line_height)
        .flex()
        .flex_row()
        .text_sm()
        .font_family("Menlo")
        .child(div().text_color(gpui::rgb(theme::TEXT_MUTED)).child(before))
        .child(cursor_cell)
        .child(div().text_color(gpui::rgb(theme::TEXT_MUTED)).child(after));

    if in_selection {
        row = row.bg(gpui::rgb(theme::OCHRE_DIM));
    }

    row.into_any_element()
}

// ── Key translation ───────────────────────────────────────────────────────────

fn keystroke_to_command(event: &KeyDownEvent, state: &EditorState) -> EditorCommand {
    match state.mode {
        Mode::Normal => key_normal(event),
        Mode::Visual(_) => key_visual(event),
        Mode::Insert => key_insert(event),
    }
}

fn key_normal(event: &KeyDownEvent) -> EditorCommand {
    use EditorCommand::*;
    let k = &event.keystroke;
    // Guard against Cmd/Ctrl combos (those are handled in handle_key_down).
    if k.modifiers.platform || k.modifiers.control {
        return Noop;
    }
    match k.key.as_str() {
        "h" => MoveLeft,
        "l" => MoveRight,
        "k" => MoveUp,
        "j" => MoveDown,
        "w" => MoveWordForward,
        "b" => MoveWordBackward,
        "0" => MoveStartOfLine,
        "$" => MoveEndOfLine,
        "g" if !k.modifiers.shift => MoveStartOfDocument,
        "G" => MoveEndOfDocument,
        // Insert-mode entry
        "i" => EnterInsert,
        "a" => AppendAfterCursor,
        "I" => InsertLineStart,
        "A" => InsertLineEnd,
        "o" => OpenLineBelow,
        "O" => OpenLineAbove,
        // Delete / change
        "d" => DeleteLine,           // `dd` equivalent: delete current line
        "D" => DeleteToLineEnd,
        "c" => ChangeLine,           // `cc` equivalent
        "C" => ChangeToLineEnd,
        // Yank / paste
        "y" => YankLine,
        "p" => PasteAfter,
        "P" => PasteBefore,
        // Helix-style: `x` selects the line, then `d` (mapped to DeleteSelection in visual) deletes it.
        "x" => SelectCurrentLine,
        _ => Noop,
    }
}

fn key_visual(event: &KeyDownEvent) -> EditorCommand {
    use EditorCommand::*;
    let k = &event.keystroke;
    if k.modifiers.platform || k.modifiers.control {
        return Noop;
    }
    match k.key.as_str() {
        "escape" => EnterNormal,
        "d" | "x" => DeleteSelection,
        "y" => YankSelection,
        "c" => ChangeLine,
        // Motions extend the selection (simplified: just move cursor).
        "h" => MoveLeft,
        "l" => MoveRight,
        "j" => MoveDown,
        "k" => MoveUp,
        _ => Noop,
    }
}

fn key_insert(event: &KeyDownEvent) -> EditorCommand {
    use EditorCommand::*;
    let k = &event.keystroke;
    // Guard against Cmd combos (handled before reaching here).
    if k.modifiers.platform {
        return Noop;
    }
    match k.key.as_str() {
        "escape" => EnterNormal,
        "backspace" => DeleteCharBefore,
        "delete" => DeleteCharAt,
        "enter" => InsertNewline,
        "left" => MoveLeft,
        "right" => MoveRight,
        "up" => MoveUp,
        "down" => MoveDown,
        "home" => MoveStartOfLine,
        "end" => MoveEndOfLine,
        _ => {
            if let Some(c) = &k.key_char {
                if !k.modifiers.control {
                    return Insert(c.clone());
                }
            }
            Noop
        }
    }
}

/// Returns true if the command will modify the buffer (used to decide whether
/// to push an undo snapshot before applying).
fn is_buffer_mutating(cmd: &EditorCommand) -> bool {
    use EditorCommand::*;
    matches!(
        cmd,
        Insert(_)
            | InsertNewline
            | DeleteCharBefore
            | DeleteCharAt
            | DeleteLine
            | DeleteToLineEnd
            | PasteAfter
            | PasteBefore
            | PasteFromClipboard(_)
            | OpenLineBelow
            | OpenLineAbove
            | ChangeLine
            | ChangeToLineEnd
            | DeleteSelection
    )
}

/// Convert a character index (as typed) to a byte offset, clamped to valid
/// UTF-8 boundaries within `s`.
fn byte_offset_for_char(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{buffer::InMemoryBuffer, command::EditorCommand};

    fn make_key(key: &str) -> KeyDownEvent {
        use gpui::{Keystroke, Modifiers};
        KeyDownEvent {
            keystroke: Keystroke {
                modifiers: Modifiers::default(),
                key: key.to_string(),
                key_char: if key.len() == 1 { Some(key.to_string()) } else { None },
            },
            is_held: false,
        }
    }

    #[test]
    fn insert_mode_printable_becomes_insert_command() {
        let state = EditorState::new();
        let event = make_key("a");
        assert_eq!(key_insert(&event), EditorCommand::Insert("a".to_string()));
    }

    #[test]
    fn escape_enters_normal_mode_command() {
        let event = make_key("escape");
        assert_eq!(key_insert(&event), EditorCommand::EnterNormal);
    }

    #[test]
    fn normal_mode_h_is_move_left() {
        let event = make_key("h");
        assert_eq!(key_normal(&event), EditorCommand::MoveLeft);
    }

    #[test]
    fn normal_mode_x_selects_line() {
        let event = make_key("x");
        assert_eq!(key_normal(&event), EditorCommand::SelectCurrentLine);
    }

    #[test]
    fn visual_mode_d_deletes_selection() {
        let event = make_key("d");
        assert_eq!(key_visual(&event), EditorCommand::DeleteSelection);
    }
}
