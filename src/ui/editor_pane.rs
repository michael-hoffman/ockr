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
//! ## Key map (Stories 07 + helix parity)
//!
//! Normal mode:
//!   h/j/k/l     — movement          w/b/e        — word motion (start/back/end)
//!   0/$         — line start/end    ^            — first non-whitespace
//!   gg/G        — doc start/end
//!   g h/l/s     — line start / line end / first non-ws (g-prefix)
//!   x           — select line (Helix `x`)
//!   d           — delete selection / delete line (dd analogue)
//!   y           — yank line (yy)    p/P          — paste after/before
//!   i/a/I/A     — enter insert      o/O          — open line below/above
//!   c/C         — change line / to EOL
//!   r<c>        — replace char at cursor
//!   v/V         — enter Visual Char / Line       gv — reselect last visual
//!   Ctrl-V      — enter Visual Block
//!   u           — undo              Ctrl-r       — redo
//!   Ctrl-d/u    — scroll half page down / up
//!   ;           — collapse selection (no-op in Normal)
//!   Escape      — enter Normal
//!
//! Insert mode:
//!   printable   — insert char       Backspace    — delete before
//!   Enter       — newline           Escape       — Normal
//!   Ctrl-w      — delete word before cursor
//!   arrows / Home / End / Delete    Cmd-S        — save
//!   Cmd-V       — paste clipboard

use std::path::PathBuf;

use gpui::{
    App, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent,
    MouseButton, MouseDownEvent, Render, Window, div, prelude::*, px,
};

use crate::actions::{FollowLink, OpenCommandPalette, SaveFile};
use crate::compiler::{preprocess::{normalise, preprocess_wikilinks}, CompileRequest, CompilerHandle};
use crate::editor::buffer::Buffer as _;
use crate::editor::{
    apply::{apply, SideEffect},
    buffer::InMemoryBuffer,
    command::EditorCommand,
    state::{EditorState, Mode, Pos, Selection, VisualKind},
};
use crate::ui::preview::PreviewPane;
use crate::ui::theme::ThemePalette;
use crate::vault::{VaultFile, VaultState};

/// Number of lines rendered in one viewport.
///
/// Only lines in `[viewport_top, viewport_top + VIEWPORT_LINES)` are emitted
/// as GPUI elements; the rest are skipped.  80 lines @ 20 px = 1600 px,
/// comfortably covering any realistic window height.
const VIEWPORT_LINES: usize = 80;

// ── Events ────────────────────────────────────────────────────────────────────

/// Events emitted by `EditorPane` to its subscribers (e.g. `MainWindow`).
pub enum EditorPaneEvent {
    /// Request that MainWindow open the given file.
    OpenFile(PathBuf),
}

impl EventEmitter<EditorPaneEvent> for EditorPane {}

// ── Multi-key sequence state ───────────────────────────────────────────────────

/// Tracks the current multi-key Normal-mode sequence, if any.
///
/// Each variant means "we received the first key of this sequence and are
/// waiting for the second key to complete it."  Only one sequence can be
/// pending at a time; starting a new sequence always cancels any prior one.
///
/// Extending for text objects (Story 20) means adding variants here rather
/// than bolting on more boolean flags.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum PendingKey {
    /// No multi-key sequence in progress.
    #[default]
    None,
    /// `g` was pressed; awaiting the second key:
    /// `g` → start of doc, `v` → reselect visual, `h/l/s/e` → g-prefix motions.
    G,
    /// `r` was pressed; awaiting the replacement character (`r<c>`).
    Replace,
}

// ── View ──────────────────────────────────────────────────────────────────────

pub struct EditorPane {
    pub focus_handle: FocusHandle,
    state: EditorState,
    buffer: InMemoryBuffer,
    /// Undo stack: each entry is (full buffer text, cursor position before mutation).
    /// Capped at 200 entries.
    undo_history: Vec<(String, Pos)>,
    /// Redo stack: populated when undo is applied; cleared on any normal edit.
    redo_history: Vec<(String, Pos)>,
    compiler: Option<CompilerHandle>,
    preview: Option<Entity<PreviewPane>>,
    /// Live vault reference for wikilink resolution during compilation.
    vault: Option<Entity<VaultState>>,
    vault_root: Option<PathBuf>,
    /// Vault-relative path of the open file (e.g. `"notes/foo.typ"`).
    /// Sent with every CompileRequest so the world resolves imports correctly.
    file_rel_path: Option<String>,
    /// Pending multi-key sequence state (e.g. `g…`, `r<c>`).
    pending: PendingKey,
    /// Monotonically-increasing compile sequence number.
    ///
    /// Incremented on every `trigger_compile` call.  The async debounce task
    /// captures the value at spawn time and checks it before sending; if a
    /// newer keystroke fired in the meantime the task silently drops its
    /// request.  This gives a trailing 50 ms debounce for free.
    compile_sequence: u64,
    /// Index of the first line visible in the editor viewport.
    ///
    /// Updated whenever the cursor moves.  Render only emits elements for
    /// `viewport_top .. viewport_top + VIEWPORT_LINES`, keeping the element
    /// tree small for large files.
    viewport_top: usize,
}

impl EditorPane {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            state: EditorState::new(),
            buffer: InMemoryBuffer::empty(),
            undo_history: Vec::new(),
            redo_history: Vec::new(),
            compiler: None,
            preview: None,
            vault: None,
            vault_root: None,
            file_rel_path: None,
            pending: PendingKey::None,
            compile_sequence: 0,
            viewport_top: 0,
        }
    }

    pub fn set_vault(&mut self, vault: Entity<VaultState>) {
        self.vault = Some(vault);
    }

    /// Vault-relative path of the currently open file, if any.
    pub fn current_rel_path(&self) -> Option<&str> {
        self.file_rel_path.as_deref()
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
        // Compute vault-relative path for correct import resolution.
        self.file_rel_path = file.abs_path
            .strip_prefix(&vault_root)
            .ok()
            .map(|p| p.to_string_lossy().into_owned());
        self.vault_root = Some(vault_root);
        self.undo_history.clear();
        self.trigger_compile(cx);
        cx.notify();
    }

    /// Schedule a compile after a 50 ms debounce.
    ///
    /// Each call increments `compile_sequence`.  The spawned task captures
    /// the current sequence number and drops the request if a newer call
    /// arrived before the timer fires — giving a trailing debounce with no
    /// extra dependencies.  Fast typists therefore produce at most ~20
    /// compile requests per second instead of one per keystroke.
    ///
    /// Also marks the preview pane as stale immediately so the UI shows
    /// visual feedback that a compile is pending.
    fn trigger_compile(&mut self, cx: &mut Context<Self>) {
        let Some(compiler) = self.compiler.clone() else { return };

        // Bump sequence — the old task will see a mismatch and drop.
        let seq = self.compile_sequence.wrapping_add(1);
        self.compile_sequence = seq;

        // Build the compile request from current buffer state.
        let files = self.vault.as_ref()
            .map(|v| v.read(cx).files.clone())
            .unwrap_or_default();
        let request = CompileRequest {
            source: preprocess_wikilinks(&self.buffer.text(), &files),
            vault_root: self.vault_root.clone(),
            file_path: self.file_rel_path.clone(),
        };

        cx.spawn(async move |this, cx| {
            // 50 ms trailing debounce.
            cx.background_executor().timer(std::time::Duration::from_millis(50)).await;
            // Check if this request is still current.
            let still_current = cx.update(|cx| {
                this.update(cx, |pane, _| pane.compile_sequence == seq)
                    .unwrap_or(false)
            }).unwrap_or(false);
            if still_current {
                compiler.send(request);
            }
        }).detach();
    }

    /// Keep the cursor visible within the viewport.
    ///
    /// Called after every state-machine step that might have moved the cursor.
    /// Does not call `cx.notify()` — the caller already will.
    fn update_viewport(&mut self) {
        let cursor_line = self.state.cursor().line;
        if cursor_line < self.viewport_top {
            self.viewport_top = cursor_line;
        } else if cursor_line >= self.viewport_top + VIEWPORT_LINES {
            self.viewport_top = cursor_line + 1 - VIEWPORT_LINES;
        }
    }

    /// Move the cursor to `line` (0-based) and scroll the viewport there.
    ///
    /// Used by features like vault search that know a target line number.
    pub fn jump_to_line(&mut self, line: usize, cx: &mut Context<Self>) {
        let clamped = line.min(self.buffer.line_count().saturating_sub(1));
        self.state.move_cursor_to(Pos::new(clamped, 0));
        self.update_viewport();
        cx.notify();
    }

    /// Find the `[[wikilink]]` under the cursor and resolve it to an absolute path.
    ///
    /// Scans the current line for `[[...]]` spans; returns the target file path
    /// if the cursor column falls inside any such span and the title resolves to
    /// a known vault file.
    fn resolve_wikilink_at_cursor(&self, cx: &App) -> Option<PathBuf> {
        let vault = self.vault.as_ref()?.read(cx);
        let pos = self.state.cursor();
        let line = self.buffer.line(pos.line);
        let col = pos.col;

        let mut offset = 0usize;
        while let Some(rel_open) = line[offset..].find("[[") {
            let open = offset + rel_open;
            let inner_start = open + 2;
            if let Some(rel_close) = line[inner_start..].find("]]") {
                let close = inner_start + rel_close;
                // Is cursor inside [[...]]?
                if col >= open && col < close + 2 {
                    let inner = &line[inner_start..close];
                    // Strip display text after `|`.
                    let target = inner.split('|').next().unwrap_or(inner).trim();
                    let key = normalise(target);
                    for file in &vault.files {
                        if normalise(&file.title) == key {
                            return Some(file.abs_path.clone());
                        }
                    }
                    return None;
                }
                offset = close + 2;
            } else {
                break;
            }
        }
        None
    }

    fn follow_link(&mut self, _: &FollowLink, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.resolve_wikilink_at_cursor(cx) {
            cx.emit(EditorPaneEvent::OpenFile(path));
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.state.path.clone() else { return };
        let content = self.buffer.text();
        let _ = std::fs::write(&path, &content);
        self.state.is_dirty = false;

        // Incrementally update the backlink index for this file.
        if let Some(vault) = &self.vault {
            if let Some(rel) = &self.file_rel_path {
                let rel_path = std::path::PathBuf::from(rel);
                let title = rel_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                let file = crate::vault::VaultFile {
                    rel_path: rel_path.clone(),
                    abs_path: path,
                    title,
                };
                let content_clone = content.clone();
                vault.update(cx, |vs, _cx| {
                    vs.reindex_file(&file, &content_clone);
                });
            }
        }
    }

    /// Push a snapshot onto the undo stack before a mutating operation.
    /// Also clears the redo stack so that new edits don't conflict with redo history.
    fn push_undo(&mut self) {
        self.push_undo_impl();
        self.redo_history.clear();
    }

    /// Push onto undo without clearing redo — used internally during redo.
    fn push_undo_keeping_redo(&mut self) {
        self.push_undo_impl();
    }

    fn push_undo_impl(&mut self) {
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let k = &event.keystroke;

        // ── Global shortcuts handled before the state machine ─────────────
        // Cmd-S: save (also bound as GPUI action but catching here for robustness).
        if k.modifiers.platform && k.key == "s" {
            self.save(cx);
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
                    self.update_viewport();
                    self.trigger_compile(cx);
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
                self.trigger_compile(cx);
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
                // Save current state to redo stack before restoring.
                let redo_snap = (self.buffer.text(), self.state.cursor());
                self.redo_history.push(redo_snap);
                self.buffer = InMemoryBuffer::from_text(&text);
                self.state.mode = Mode::Normal;
                self.state.move_cursor_to(pos);
                self.state.is_dirty = true;
                self.update_viewport();
                self.trigger_compile(cx);
                cx.notify();
            }
            return;
        }

        // ── Redo (Ctrl-r in Normal/Visual) ───────────────────────────────
        if k.modifiers.control && k.key == "r" && !k.modifiers.platform
            && self.state.mode != Mode::Insert
        {
            if let Some((text, pos)) = self.redo_history.pop() {
                // Save current state to undo stack (without clearing redo).
                self.push_undo_keeping_redo();
                self.buffer = InMemoryBuffer::from_text(&text);
                self.state.mode = Mode::Normal;
                self.state.move_cursor_to(pos);
                self.state.is_dirty = true;
                self.update_viewport();
                self.trigger_compile(cx);
                cx.notify();
            }
            return;
        }

        // ── Multi-key sequences ──────────────────────────────────────────
        // ── Multi-key sequences ──────────────────────────────────────────────
        //
        // `r` in Normal mode: set pending to Replace, wait for the char.
        if self.state.mode == Mode::Normal && k.key == "r"
            && !k.modifiers.platform && !k.modifiers.control && !k.modifiers.shift
        {
            self.pending = PendingKey::Replace;
            return;
        }

        // Complete a pending `r<c>` replace.
        if self.pending == PendingKey::Replace {
            self.pending = PendingKey::None;
            if self.state.mode == Mode::Normal {
                if let Some(ch) = &k.key_char {
                    if !k.modifiers.control && !k.modifiers.platform {
                        self.push_undo();
                        let prev = std::mem::take(&mut self.state);
                        let (new_state, effect) = apply(
                            EditorCommand::ReplaceChar(ch.clone()),
                            prev,
                            &mut self.buffer,
                        );
                        self.state = new_state;
                        if effect == SideEffect::BufferChanged {
                            self.trigger_compile(cx);
                        }
                        cx.notify();
                    }
                }
            }
            return;
        }

        // `g` in Normal mode: set pending to G, then complete on second key.
        if self.state.mode == Mode::Normal && k.key == "g"
            && !k.modifiers.platform && !k.modifiers.control && !k.modifiers.shift
        {
            if self.pending == PendingKey::G {
                // Second `g` → go to start of document.
                self.pending = PendingKey::None;
                let prev = std::mem::take(&mut self.state);
                let (new_state, _) = apply(EditorCommand::MoveStartOfDocument, prev, &mut self.buffer);
                self.state = new_state;
                cx.notify();
            } else {
                self.pending = PendingKey::G;
            }
            return;
        }
        if self.pending == PendingKey::G {
            self.pending = PendingKey::None;
            if !k.modifiers.platform && !k.modifiers.control {
                let cmd = match k.key.as_str() {
                    "v" if self.state.mode == Mode::Normal => Some(EditorCommand::ReselectLastVisual),
                    "h" => Some(EditorCommand::MoveStartOfLine),
                    "l" => Some(EditorCommand::MoveEndOfLine),
                    "s" => Some(EditorCommand::MoveFirstNonWhitespace),
                    "e" => Some(EditorCommand::MoveWordEnd),
                    _ => None,
                };
                if let Some(cmd) = cmd {
                    let prev = std::mem::take(&mut self.state);
                    let (new_state, _) = apply(cmd, prev, &mut self.buffer);
                    self.state = new_state;
                    cx.notify();
                    return;
                }
            }
            // Unknown `g…` sequence — fall through to normal handling.
        }

        let cmd = keystroke_to_command(event, &self.state);
        if cmd == EditorCommand::Noop {
            return;
        }

        // OpenPalette is a UI command — bubble it up through the window focus chain
        // so MainWindow's on_action handler receives it.
        if cmd == EditorCommand::OpenPalette {
            window.dispatch_action(Box::new(OpenCommandPalette), cx);
            return;
        }

        // Snapshot before any mutating command.
        if is_buffer_mutating(&cmd) {
            self.push_undo();
        }

        let prev_state = std::mem::take(&mut self.state);
        let (new_state, effect) = apply(cmd, prev_state, &mut self.buffer);
        self.state = new_state;
        self.update_viewport();

        match effect {
            SideEffect::BufferChanged => {
                self.trigger_compile(cx);
                cx.notify();
            }
            SideEffect::SaveFile => {
                self.save(cx);
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
        let visual_line = if raw_y < 0.0 {
            0usize
        } else {
            (raw_y / line_height) as usize
        };
        // Translate the clicked visual row (relative to top of viewport) into
        // an absolute buffer line, then clamp to the document.
        let line = (self.viewport_top + visual_line)
            .min(self.buffer.line_count().saturating_sub(1));

        let raw_x = (f32::from(event.position.x) - padding).max(0.0);
        let col_approx = (raw_x / char_width) as usize;
        // Clamp col to a valid UTF-8 boundary.
        let line_str = self.buffer.line(line);
        let col = byte_offset_for_char(line_str, col_approx);

        self.state.move_cursor_to(Pos::new(line, col));
        self.state.mode = Mode::Normal;
        self.update_viewport();
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
        let t = cx.global::<ThemePalette>().clone();
        let line_count = self.buffer.line_count();
        let cursor = self.state.cursor();
        let mode = self.state.mode;
        let selection = self.state.selection;

        let front_matter_end = front_matter_end(&self.buffer, line_count);

        // Virtual rendering: only emit the lines visible in the current viewport.
        // This bounds element creation to O(VIEWPORT_LINES) regardless of document
        // length, keeping layout work constant as the file grows.
        let first = self.viewport_top;
        let last = (first + VIEWPORT_LINES).min(line_count);

        let mut line_elements = Vec::with_capacity(last - first);
        for i in first..last {
            let text = self.buffer.line(i).to_string();
            let in_selection = is_line_in_visual_selection(i, mode, &selection);
            let is_front_matter = front_matter_end.map(|end| i < end).unwrap_or(false);
            line_elements.push(render_line(i, text, cursor, mode, in_selection, is_front_matter, &t));
        }

        let mode_label = match mode {
            Mode::Insert => "INSERT",
            Mode::Normal => "NORMAL",
            Mode::Visual(_) => "VISUAL",
        };
        let mode_color = match mode {
            Mode::Insert => gpui::rgb(t.mode_insert),
            Mode::Normal => gpui::rgb(t.mode_normal),
            Mode::Visual(_) => gpui::rgb(t.mode_visual),
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
                    .text_color(gpui::rgb(t.text))
                    .child("o"),
            )
            .child(
                div()
                    .w(gpui::px(2.0))
                    .h(gpui::px(11.0))
                    .bg(gpui::rgb(t.ochre)),
            );

        let status_bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .px_3()
            .py_1()
            .bg(gpui::rgb(t.bg_base))
            .border_t_1()
            .border_color(gpui::rgb(t.border_subtle))
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
                    .text_color(gpui::rgb(t.text_faint))
                    .font_family("Menlo")
                    .child(format!("{}:{}", cursor.line + 1, cursor.col + 1)),
            )
            .child(if self.state.is_dirty {
                div()
                    .text_color(gpui::rgb(t.ochre))
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
            .bg(gpui::rgb(t.bg_panel))
            .on_action(cx.listener(|this, _: &SaveFile, _window, cx| {
                this.save(cx);
                cx.notify();
            }))
            .on_action(cx.listener(Self::follow_link))
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

/// Returns the index of the first heading line (`= ...`), which marks the end
/// of the front matter block. Returns `None` if no heading is found (no
/// dimming applied) or if the heading is on line 0 (nothing to dim).
fn front_matter_end(buffer: &InMemoryBuffer, line_count: usize) -> Option<usize> {
    for i in 0..line_count {
        let line = buffer.line(i);
        if line.starts_with("= ") || line.starts_with("== ") || line.starts_with("=== ") {
            return if i > 0 { Some(i) } else { None };
        }
    }
    None
}

fn render_line(
    line_idx: usize,
    text: String,
    cursor: Pos,
    mode: Mode,
    in_selection: bool,
    is_front_matter: bool,
    t: &ThemePalette,
) -> impl gpui::IntoElement {
    let line_height = px(20.0);

    // Selection highlight: entire-line background for Visual(Line).
    let sel_bg = gpui::rgb(t.ochre_dim);
    let line_bg = if in_selection && line_idx != cursor.line {
        Some(sel_bg)
    } else {
        None
    };

    if line_idx != cursor.line {
        let text_color = if is_front_matter {
            gpui::rgb(t.text_faint)
        } else {
            gpui::rgb(t.text_muted)
        };
        let mut row = div()
            .min_h(line_height)
            .whitespace_nowrap()
            .overflow_x_hidden()
            .text_color(text_color)
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
        Mode::Insert => gpui::rgb(t.mode_insert),
        Mode::Normal => gpui::rgb(t.mode_normal),
        Mode::Visual(_) => gpui::rgb(t.mode_visual),
    };

    // Insert mode → bar cursor (left border); Normal/Visual → block (filled bg).
    let cursor_cell = if mode == Mode::Insert {
        div()
            .text_color(gpui::rgb(t.text_muted))
            .border_l_2()
            .border_color(cursor_color)
            .child(cursor_char)
    } else {
        div()
            .text_color(gpui::rgb(t.cursor_fg))
            .bg(cursor_color)
            .child(cursor_char)
    };

    let mut row = div()
        .min_h(line_height)
        .flex()
        .flex_row()
        .whitespace_nowrap()
        .overflow_x_hidden()
        .text_sm()
        .font_family("Menlo")
        .child(div().text_color(gpui::rgb(t.text_muted)).child(before))
        .child(cursor_cell)
        .child(div().text_color(gpui::rgb(t.text_muted)).child(after));

    if in_selection {
        row = row.bg(gpui::rgb(t.ochre_dim));
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

/// Normal-mode key → command.  Called with `pending_g = false` already handled
/// in the caller for multi-key sequences like `gv` / `gg`.
fn key_normal(event: &KeyDownEvent) -> EditorCommand {
    use EditorCommand::*;
    let k = &event.keystroke;

    // Ctrl combos handled before the main guard so Ctrl-V can enter Visual Block.
    if k.modifiers.control && !k.modifiers.platform {
        return match k.key.as_str() {
            "v" => EnterVisualBlock,
            "d" => ScrollHalfDown,
            "u" => ScrollHalfUp,
            _ => Noop,
        };
    }
    // Guard against remaining Cmd combos (handled in handle_key_down).
    if k.modifiers.platform {
        return Noop;
    }
    // `:` opens the command palette (Helix-mode convention, like Zed).
    // Check key_char because GPUI reports the physical key name (";") in k.key,
    // and the shifted character (":") in k.key_char.
    if k.key_char.as_deref() == Some(":") || k.key == ":" {
        return OpenPalette;
    }
    match k.key.as_str() {
        "h" => MoveLeft,
        "l" => MoveRight,
        "k" => MoveUp,
        "j" => MoveDown,
        "w" => MoveWordForward,
        "b" => MoveWordBackward,
        "e" => MoveWordEnd,
        "W" => MoveWORDForward,
        "B" => MoveWORDBackward,
        "E" => MoveWORDEnd,
        "0" => MoveStartOfLine,
        "$" => MoveEndOfLine,
        "^" => MoveFirstNonWhitespace,
        // `g` alone is handled as pending by the caller; single-g falls through to Noop.
        "G" => MoveEndOfDocument,
        // Collapse Visual selection (no-op in Normal, but consistent binding)
        ";" => CollapseSelection,
        "_" => TrimSelection,
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
        // Visual-mode entry
        "v" => EnterVisualChar,
        "V" => EnterVisualLine,
        // Indent / dedent (single-line, same key as visual-mode versions)
        ">" => IndentLines,
        "<" => DedentLines,
        _ => Noop,
    }
}

fn key_visual(event: &KeyDownEvent) -> EditorCommand {
    use EditorCommand::*;
    let k = &event.keystroke;
    if k.modifiers.platform {
        return Noop;
    }
    // Ctrl combos — Ctrl-V cycles back to Visual Block; Ctrl-d/u scroll.
    if k.modifiers.control {
        return match k.key.as_str() {
            "v" => EnterVisualBlock,
            "d" => ScrollHalfDown,
            "u" => ScrollHalfUp,
            _ => Noop,
        };
    }
    match k.key.as_str() {
        "escape" => EnterNormal,
        // Operators on selection
        "d" | "x" => DeleteSelection,
        "y" => YankSelection,
        "c" => ChangeSelection,
        // Indent / dedent and stay in visual
        ">" => IndentLines,
        "<" => DedentLines,
        // Collapse selection to cursor endpoint, return to Normal.
        ";" => CollapseSelection,
        // Trim leading/trailing whitespace from the selection bounds.
        "_" => TrimSelection,
        // All motions extend the selection (anchor fixed, cursor moves).
        "h" => MoveLeft,
        "l" => MoveRight,
        "j" => MoveDown,
        "k" => MoveUp,
        "w" => MoveWordForward,
        "b" => MoveWordBackward,
        "e" => MoveWordEnd,
        "W" => MoveWORDForward,
        "B" => MoveWORDBackward,
        "E" => MoveWORDEnd,
        "0" => MoveStartOfLine,
        "$" => MoveEndOfLine,
        "^" => MoveFirstNonWhitespace,
        "G" => MoveEndOfDocument,
        // Switch between visual modes without leaving visual
        "v" => EnterVisualChar,
        "V" => EnterVisualLine,
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
    // Ctrl combos in Insert mode.
    if k.modifiers.control {
        return match k.key.as_str() {
            "w" => DeleteWordBefore,    // Ctrl-w: delete word before cursor
            _ => Noop,
        };
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
                return Insert(c.clone());
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
            | ReplaceChar(_)
            | DeleteWordBefore
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
