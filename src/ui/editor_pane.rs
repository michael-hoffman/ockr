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
//!   Ctrl-d/u    — scroll half page down / up  Ctrl-f/b  — page down / up
//!   f/F/t/T<c>  — find char forward/back / till forward/back
//!   {/}         — paragraph back / forward     %         — select whole file
//!   ~           — switch case                  .         — repeat last change
//!   mi/ma<obj>  — select inner/around object (Story 20)
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
    MouseButton, MouseDownEvent, Render, Window, div, prelude::*, px, rgba,
};

use crate::actions::{FollowLink, LineNumbersAbsolute, LineNumbersOff, LineNumbersRelative, OpenReplace, OpenSearch, SaveFile};
use crate::compiler::{preprocess::{normalise, preprocess_wikilinks}, CompileRequest, CompilerHandle, Diagnostic, DiagnosticSeverity, PluginPackages, PreviewMode};
use crate::editor::buffer::Buffer as _;
use crate::editor::{
    apply::{apply, SideEffect},
    buffer::InMemoryBuffer,
    command::{EditorCommand, TextObjectKind},
    keymap::{KeymapHandler, KeymapResult, OperatorKind},
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

/// How line numbers are displayed in the gutter.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum LineNumberMode {
    /// No gutter — maximum horizontal space for content.
    Off,
    /// Show the 1-based absolute line number on every line.
    Absolute,
    /// Show the cursor line's absolute number; other lines show distance from cursor.
    /// Matches Helix / Neovim `relativenumber` behaviour.
    #[default]
    Relative,
}

/// State for the `[[` wikilink autocomplete popup.
struct WikilinkState {
    /// Byte offset of the opening `[` on the current line.
    open_col: usize,
    /// Vault file titles that match `fragment` (prefix-insensitive).
    candidates: Vec<String>,
    /// Currently highlighted candidate (0-based index into `candidates`).
    selected: usize,
}

/// State for the in-buffer `/` or `?` search bar (and optional replace row).
struct SearchState {
    /// Text typed so far.
    query: String,
    /// All match start positions across the whole buffer (updated on every keystroke).
    matches: Vec<Pos>,
    /// Index into `matches` for the "current" focused match.
    current_idx: usize,
    /// Cursor position when search was opened — restored on Escape.
    saved_cursor: Pos,
    /// `true` = `?` backward search; `false` = `/` forward search.
    backward: bool,
    /// Replacement string. `None` = search-only mode; `Some` = find-and-replace mode.
    replace: Option<String>,
    /// Which row has keyboard focus: `false` = query, `true` = replace.
    replace_focused: bool,
    /// `true` when the last match jump had to wrap around the document boundary.
    wrapped: bool,
}

/// Typst preamble prepended for HTML-mode compilation only.
///
/// Two fixes are bundled here:
///
/// 1. **Inline-math paragraph split** — Typst's HTML export treats `$x$`
///    as a block element, breaking it out of the surrounding paragraph.
///    The show rule wraps non-block equations in `box` so they stay inline.
///
/// 2. **SVG math color** — `html.frame` renders math via the typst SVG
///    pipeline, which uses the document's `text.fill` for glyph outlines.
///    The default is black, which is invisible on ockr's dark background.
///    `#set text(fill: luma(95%))` makes glyph paths near-white.
///    This set rule only affects SVG frames (text.fill is not yet applied to
///    HTML elements in typst-html), so normal paragraph text is unaffected.
const HTML_PREAMBLE: &str = concat!(
    // Make math glyphs near-white so they show on the dark background.
    // (text.fill is ignored for HTML elements; only SVG frames honour it.)
    "#set text(fill: luma(95%))\n",
    // Fix inline-math paragraph splitting.
    "#show math.equation: it => context {\n",
    "  if target() == \"html\" {\n",
    "    show: if it.block { it => it } else { box }\n",
    "    html.frame(it)\n",
    "  } else {\n",
    "    it\n",
    "  }\n",
    "}\n",
);

// ── Events ────────────────────────────────────────────────────────────────────

/// Events emitted by `EditorPane` to its subscribers (e.g. `MainWindow`).
pub enum EditorPaneEvent {
    /// Request that MainWindow open the given file.
    OpenFile(PathBuf),
    /// Request that MainWindow open the command palette.
    OpenPalette,
}

impl EventEmitter<EditorPaneEvent> for EditorPane {}

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
    /// Pluggable keyboard mode (Helix, Standard, etc.).
    keymap: Box<dyn KeymapHandler>,
    /// Active `[[` autocomplete popup state; `Some` while in Insert mode inside `[[…`.
    wikilink_complete: Option<WikilinkState>,
    /// Active search bar state; `Some` while `/` search is open.
    search: Option<SearchState>,
    /// Query from the last completed search; used by `n`/`N` repeat navigation.
    last_search: Option<String>,
    /// Direction of the last completed search (`true` = backward / `?`).
    last_search_backward: bool,
    /// Match position from the last `n`/`N` navigation: (current 1-indexed, total, wrapped).
    /// Shown in the status bar while the search bar is closed.
    search_nav_status: Option<(usize, usize, bool)>,
    /// How line numbers are rendered in the gutter.
    line_number_mode: LineNumberMode,
    /// Plugin-provided typst packages forwarded to each CompileRequest.
    plugin_packages: Option<PluginPackages>,
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
    /// Diagnostics from the most recent typst compilation.
    ///
    /// Cleared on a successful compile; updated with `set_diagnostics` when
    /// the compiler returns errors or warnings.  Used to render per-line
    /// indicator gutter marks.
    diagnostics: Vec<Diagnostic>,
    /// Cached (word_count, char_count) — `None` means stale.
    /// Invalidated whenever the buffer mutates; recomputed lazily in `render()`.
    cached_doc_stats: Option<(usize, usize)>,
    /// Microseconds elapsed for the last `handle_key_down` call (perf overlay).
    last_key_micros: Option<u128>,
    /// Whether the `OCKR_PERF=1` timing overlay is enabled (set once at init).
    perf_overlay: bool,
}

impl EditorPane {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let keymap: Box<dyn KeymapHandler> = if cx.try_global::<crate::settings::Settings>()
            .is_some_and(|s| s.keyboard_mode == "standard")
        {
            Box::new(crate::editor::keymap_standard::StandardKeymap::new())
        } else {
            Box::new(crate::editor::keymap_helix::HelixKeymap::new())
        };
        // Standard mode starts in Insert (the default); Helix starts in Normal.
        let state = EditorState::new();
        if keymap.mode_label(&state) != "STANDARD" {
            // EditorState defaults to Insert; switch to Normal for Helix mode.
            // (We don't have a Noop path through apply for this, so set directly.)
        }
        Self {
            focus_handle: cx.focus_handle(),
            state,
            buffer: InMemoryBuffer::empty(),
            undo_history: Vec::new(),
            redo_history: Vec::new(),
            compiler: None,
            preview: None,
            vault: None,
            vault_root: None,
            file_rel_path: None,
            keymap,
            wikilink_complete: None,
            search: None,
            last_search: None,
            last_search_backward: false,
            search_nav_status: None,
            line_number_mode: LineNumberMode::Relative,
            plugin_packages: None,
            compile_sequence: 0,
            viewport_top: 0,
            diagnostics: Vec::new(),
            cached_doc_stats: None,
            last_key_micros: None,
            perf_overlay: std::env::var_os("OCKR_PERF").is_some(),
        }
    }

    /// Update the per-line diagnostic underlines after a compile error.
    pub fn set_diagnostics(&mut self, diags: Vec<Diagnostic>) {
        self.diagnostics = diags;
    }

    /// Clear diagnostics (called after a successful compile).
    pub fn clear_diagnostics(&mut self) {
        self.diagnostics.clear();
    }

    pub fn set_vault(&mut self, vault: Entity<VaultState>) {
        self.vault = Some(vault);
    }

    /// Switch between Helix and Standard keyboard modes.
    pub fn switch_keyboard_mode(&mut self) {
        let current = self.keymap.mode_label(&self.state);
        if current == "STANDARD" {
            self.keymap = Box::new(crate::editor::keymap_helix::HelixKeymap::new());
            self.state.mode = Mode::Normal;
        } else {
            self.keymap = Box::new(crate::editor::keymap_standard::StandardKeymap::new());
            self.state.mode = Mode::Insert;
        }
    }

    /// Share the plugin packages map so every CompileRequest carries it.
    pub fn set_plugin_packages(&mut self, packages: PluginPackages) {
        self.plugin_packages = Some(packages);
    }

    /// Vault-relative path of the currently open file, if any.
    pub fn current_rel_path(&self) -> Option<&str> {
        self.file_rel_path.as_deref()
    }


    /// Current cursor position.
    pub fn cursor_pos(&self) -> Pos {
        self.state.cursor()
    }

    /// Index of the topmost visible line in the viewport.
    pub fn viewport_top(&self) -> usize {
        self.viewport_top
    }


    /// Restore cursor position and viewport after switching back to a tab.
    pub fn restore_cursor_and_viewport(&mut self, pos: Pos, viewport: usize) {
        self.state.move_cursor_to(pos);
        self.viewport_top = viewport;
    }

    /// Whether the buffer has unsaved changes.
    pub fn is_dirty(&self) -> bool {
        self.state.is_dirty
    }

    pub fn set_compiler(&mut self, handle: CompilerHandle, preview: Entity<PreviewPane>) {
        self.compiler = Some(handle);
        self.preview = Some(preview);
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
        // Restore persisted undo/redo history for this file (empty if never saved).
        let (undo, redo) = crate::undo_store::load_undo_history(&file.abs_path);
        self.undo_history = undo;
        self.redo_history = redo;
        self.cached_doc_stats = None;
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
    pub(crate) fn trigger_compile(&mut self, cx: &mut Context<Self>) {
        let Some(compiler) = self.compiler.clone() else { return };

        // Bump sequence — the old task will see a mismatch and drop.
        let seq = self.compile_sequence.wrapping_add(1);
        self.compile_sequence = seq;

        // Build the compile request from current buffer state.
        let files = self.vault.as_ref()
            .map(|v| v.read(cx).files.clone())
            .unwrap_or_default();
        // Read the active preview mode from the GPUI global (set by MainWindow toggle).
        let mode = cx.try_global::<PreviewMode>().copied().unwrap_or_default();

        // Preprocess source: resolve wikilinks, then optionally prepend the
        // HTML preamble that fixes inline-math paragraph splitting (a known
        // Typst HTML-export limitation — `$x$` is treated as a block element
        // unless wrapped with `box` via a show rule).
        let preprocessed = preprocess_wikilinks(&self.buffer.text(), &files);
        let source = if mode == PreviewMode::Html {
            format!("{HTML_PREAMBLE}{preprocessed}")
        } else {
            preprocessed
        };

        let request = CompileRequest {
            source,
            vault_root: self.vault_root.clone(),
            file_path: self.file_rel_path.clone(),
            mode,
            plugin_packages: self.plugin_packages.clone(),
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

    /// Returns the full text of the current buffer (lines joined by `\n`).
    ///
    /// Used by the Document Outline panel to parse headings.
    pub fn buffer_text(&self) -> String {
        (0..self.buffer.line_count())
            .map(|l| self.buffer.line(l))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Compute (word_count, char_count) by iterating lines directly.
    /// Avoids allocating a full buffer join string — O(N) chars but no heap join.
    fn compute_doc_stats(&self) -> (usize, usize) {
        let mut words = 0usize;
        let mut chars = 0usize;
        let lc = self.buffer.line_count();
        for i in 0..lc {
            let line = self.buffer.line(i);
            words += line.split_whitespace().count();
            chars += line.chars().count();
            if i + 1 < lc {
                chars += 1; // newline between lines
            }
        }
        (words, chars)
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

    /// Follow the wikilink under the cursor — callable from the command palette
    /// where the `FollowLink` action cannot be dispatched via the focus chain.
    pub fn follow_link_at_cursor(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.resolve_wikilink_at_cursor(cx) {
            cx.emit(EditorPaneEvent::OpenFile(path));
        }
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.state.path.clone() else { return };
        let content = self.buffer.text();
        let _ = std::fs::write(&path, &content);
        self.state.is_dirty = false;

        // Tell the compiler to drop this file's source cache entry so any
        // other file that imports it will pick up the updated content on the
        // next compilation rather than serving the stale cached version.
        if let Some(ref compiler) = self.compiler {
            compiler.invalidate_import(path.clone());
        }

        // Persist the undo/redo stacks so they survive a close-and-reopen.
        crate::undo_store::save_undo_history(&path, &self.undo_history, &self.redo_history);

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

    // ── Search ────────────────────────────────────────────────────────────────

    /// Open the search bar (`/` forward, `?` backward), saving cursor for Escape-cancel.
    pub fn open_search(&mut self, backward: bool) {
        let saved = self.state.cursor();
        self.search_nav_status = None;
        self.search = Some(SearchState {
            query: String::new(),
            matches: Vec::new(),
            current_idx: 0,
            saved_cursor: saved,
            backward,
            replace: None,
            replace_focused: false,
            wrapped: false,
        });
    }

    /// Open the search+replace bar (Cmd-H), focused on the query row.
    pub fn open_replace(&mut self) {
        let saved = self.state.cursor();
        self.search_nav_status = None;
        self.search = Some(SearchState {
            query: String::new(),
            matches: Vec::new(),
            current_idx: 0,
            saved_cursor: saved,
            backward: false,
            replace: Some(String::new()),
            replace_focused: false,
            wrapped: false,
        });
    }

    pub fn set_line_number_mode(&mut self, mode: LineNumberMode) {
        self.line_number_mode = mode;
    }

    /// Route a keystroke to the search bar (or its replace row) while it is open.
    fn handle_search_key(&mut self, k: &gpui::Keystroke, cx: &mut Context<Self>) {
        let in_replace = self.search.as_ref()
            .map(|s| s.replace.is_some() && s.replace_focused)
            .unwrap_or(false);

        match k.key.as_str() {
            "escape" => {
                // Cancel: restore cursor to pre-search position.
                let saved = self.search.as_ref().map(|s| s.saved_cursor);
                self.search = None;
                if let Some(pos) = saved {
                    self.state.move_cursor_to(pos);
                    self.update_viewport();
                }
            }
            "tab" => {
                // Toggle focus between query and replace rows.
                if let Some(ref mut s) = self.search {
                    if s.replace.is_some() {
                        s.replace_focused = !s.replace_focused;
                    }
                }
            }
            "enter" => {
                if in_replace {
                    // Enter in replace row: replace current match and advance.
                    self.replace_current(cx);
                } else {
                    // Enter in query row: close bar, persist for n/N.
                    if let Some(ref s) = self.search {
                        if !s.query.is_empty() {
                            self.last_search = Some(s.query.clone());
                            self.last_search_backward = s.backward;
                        }
                    }
                    self.search = None;
                }
            }
            "backspace" => {
                if let Some(ref mut s) = self.search {
                    if in_replace {
                        if let Some(ref mut r) = s.replace { r.pop(); }
                    } else {
                        s.query.pop();
                    }
                }
                if !in_replace { self.update_search_matches(cx); }
            }
            "a" if k.modifiers.control && in_replace => {
                // Ctrl-A in replace row: replace all matches.
                self.replace_all(cx);
            }
            _ => {
                if !k.modifiers.platform && !k.modifiers.control {
                    // Use key_char when present; fall back to explicit "space" key name.
                    let ch_opt = k.key_char.as_deref()
                        .or_else(|| if k.key == "space" { Some(" ") } else { None });
                    if let Some(ch) = ch_opt {
                        if let Some(ref mut s) = self.search {
                            if in_replace {
                                if let Some(ref mut r) = s.replace { r.push_str(ch); }
                            } else {
                                s.query.push_str(ch);
                            }
                        }
                        if !in_replace { self.update_search_matches(cx); }
                    }
                }
            }
        }
        cx.notify();
    }

    /// Recompute match positions from the current query and jump to the best one.
    fn update_search_matches(&mut self, _cx: &mut Context<Self>) {
        let (query, saved, backward) = match &self.search {
            Some(s) => (s.query.clone(), s.saved_cursor, s.backward),
            None => return,
        };

        let matches = find_all_matches(&self.buffer, &query);

        // For forward search: pick first match at or after saved cursor.
        // For backward search: pick last match at or before saved cursor.
        let (idx, wrapped) = if matches.is_empty() {
            (0, false)
        } else if backward {
            let found = matches
                .iter()
                .rposition(|p| {
                    p.line < saved.line
                        || (p.line == saved.line && p.col <= saved.col)
                });
            (found.unwrap_or(matches.len() - 1), found.is_none())
        } else {
            let found = matches
                .iter()
                .position(|p| {
                    p.line > saved.line
                        || (p.line == saved.line && p.col >= saved.col)
                });
            (found.unwrap_or(0), found.is_none())
        };

        let dest = matches.get(idx).copied();

        if let Some(s) = &mut self.search {
            s.matches = matches;
            s.current_idx = idx;
            s.wrapped = wrapped && dest.is_some();
        }

        if let Some(pos) = dest {
            self.state.move_cursor_to(pos);
        } else if query.is_empty() {
            self.state.move_cursor_to(saved);
        }
        self.update_viewport();
    }

    /// Move to the next match in the search direction, wrapping around.
    ///
    /// "Next" means forward for `/` searches and backward for `?` searches,
    /// matching Vim / Helix semantics: `n` repeats the original direction.
    fn search_next(&mut self, cx: &mut Context<Self>) {
        let query = match &self.last_search {
            Some(q) => q.clone(),
            None => return,
        };
        let backward = self.last_search_backward;
        let matches = find_all_matches(&self.buffer, &query);
        if matches.is_empty() {
            return;
        }
        let cursor = self.state.cursor();
        let (idx, wrapped) = if backward {
            let found = matches
                .iter()
                .rposition(|p| p.line < cursor.line || (p.line == cursor.line && p.col < cursor.col));
            (found.unwrap_or(matches.len() - 1), found.is_none())
        } else {
            let found = matches
                .iter()
                .position(|p| p.line > cursor.line || (p.line == cursor.line && p.col > cursor.col));
            (found.unwrap_or(0), found.is_none())
        };
        self.state.move_cursor_to(matches[idx]);
        self.search_nav_status = Some((idx + 1, matches.len(), wrapped));
        self.update_viewport();
        cx.notify();
    }

    /// Move to the previous match (opposite of `search_next`), wrapping around.
    fn search_prev(&mut self, cx: &mut Context<Self>) {
        let query = match &self.last_search {
            Some(q) => q.clone(),
            None => return,
        };
        let backward = self.last_search_backward;
        let matches = find_all_matches(&self.buffer, &query);
        if matches.is_empty() {
            return;
        }
        let cursor = self.state.cursor();
        // `N` is always the reverse of `n`.
        let (idx, wrapped) = if backward {
            let found = matches
                .iter()
                .position(|p| p.line > cursor.line || (p.line == cursor.line && p.col > cursor.col));
            (found.unwrap_or(0), found.is_none())
        } else {
            let found = matches
                .iter()
                .rposition(|p| p.line < cursor.line || (p.line == cursor.line && p.col < cursor.col));
            (found.unwrap_or(matches.len() - 1), found.is_none())
        };
        self.state.move_cursor_to(matches[idx]);
        self.search_nav_status = Some((idx + 1, matches.len(), wrapped));
        self.update_viewport();
        cx.notify();
    }

    // ── Replace ───────────────────────────────────────────────────────────────

    /// Replace the current search match with the replacement string, then advance.
    fn replace_current(&mut self, cx: &mut Context<Self>) {
        let (query, replace_str, match_pos) = {
            let Some(ref s) = self.search else { return };
            let Some(ref r) = s.replace else { return };
            if s.matches.is_empty() || s.query.is_empty() { return; }
            (s.query.clone(), r.clone(), s.matches[s.current_idx])
        };

        self.push_undo();
        let qlen = query.len();
        self.buffer.delete_range(match_pos.line, match_pos.col, match_pos.col + qlen);
        self.buffer.insert(match_pos.line, match_pos.col, &replace_str);
        self.state.is_dirty = true;

        // Recompute matches after the edit; advance to next.
        self.update_search_matches(cx);
        self.search_next(cx);
        self.trigger_compile(cx);
    }

    /// Replace every occurrence of the search query with the replacement string.
    fn replace_all(&mut self, cx: &mut Context<Self>) {
        let (query, replace_str, matches) = {
            let Some(ref s) = self.search else { return };
            let Some(ref r) = s.replace else { return };
            if s.query.is_empty() || s.matches.is_empty() { return; }
            (s.query.clone(), r.clone(), s.matches.clone())
        };

        self.push_undo();
        let qlen = query.len();
        // Iterate in reverse so earlier positions stay valid after each deletion.
        for m in matches.iter().rev() {
            self.buffer.delete_range(m.line, m.col, m.col + qlen);
            self.buffer.insert(m.line, m.col, &replace_str);
        }
        self.state.is_dirty = true;

        // Clear matches (they're all gone) and close the bar.
        if let Some(ref mut s) = self.search {
            s.matches.clear();
            s.current_idx = 0;
        }
        self.trigger_compile(cx);
        cx.notify();
    }

    // ── Operator-motion helpers ───────────────────────────────────────────────

    /// Extract the word under the cursor for `*` / `#` search.
    ///
    /// Expands from the cursor position left and right along the same character
    /// class (alphanumeric+underscore = word; anything else = symbolic).
    /// Returns `None` when the cursor is on whitespace.
    fn word_under_cursor(&self) -> Option<String> {
        let cursor = self.state.cursor();
        let line = self.buffer.line(cursor.line);
        if line.is_empty() {
            return None;
        }
        let col = cursor.col.min(line.len().saturating_sub(1));
        let chars: Vec<char> = line.chars().collect();
        // Convert byte offset to char index.
        let char_idx = line[..col].chars().count();
        let char_idx = char_idx.min(chars.len().saturating_sub(1));
        let ch = chars[char_idx];
        if ch.is_whitespace() {
            return None;
        }
        let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
        let same_class = |c: char| is_word_char(c) == is_word_char(ch);
        let mut start = char_idx;
        while start > 0 && same_class(chars[start - 1]) {
            start -= 1;
        }
        let mut end = char_idx;
        while end + 1 < chars.len() && same_class(chars[end + 1]) {
            end += 1;
        }
        Some(chars[start..=end].iter().collect())
    }

    /// Apply an operator (`d`/`c`/`y`) after extending a selection via `motion`.
    fn apply_operator_motion(
        &mut self,
        op: OperatorKind,
        motion: EditorCommand,
        cx: &mut Context<Self>,
    ) {
        self.push_undo();
        let cursor = self.state.cursor();
        self.state.mode = Mode::Visual(VisualKind::Char);
        self.state.selection = Selection { anchor: cursor, cursor };
        let prev = std::mem::take(&mut self.state);
        let (sel_state, _) = apply(motion, prev, &mut self.buffer);
        self.state = sel_state;
        let op_cmd = operator_kind_to_command(op);
        let prev2 = std::mem::take(&mut self.state);
        let (new_state, effect) = apply(op_cmd, prev2, &mut self.buffer);
        self.state = new_state;
        if effect == SideEffect::BufferChanged {
            self.cached_doc_stats = None;
            self.trigger_compile(cx);
        }
        self.update_viewport();
        cx.notify();
    }

    /// Apply an operator (`d`/`c`/`y`) on a text object (inner or around).
    fn apply_operator_object(
        &mut self,
        op: OperatorKind,
        inner: bool,
        kind: TextObjectKind,
        cx: &mut Context<Self>,
    ) {
        self.push_undo();
        let prev = std::mem::take(&mut self.state);
        let (sel_state, _) =
            apply(EditorCommand::SelectObject { inner, kind }, prev, &mut self.buffer);
        self.state = sel_state;
        if matches!(self.state.mode, Mode::Visual(_)) {
            let op_cmd = operator_kind_to_command(op);
            let prev2 = std::mem::take(&mut self.state);
            let (new_state, effect) = apply(op_cmd, prev2, &mut self.buffer);
            self.state = new_state;
            if effect == SideEffect::BufferChanged {
                self.cached_doc_stats = None;
                self.trigger_compile(cx);
            }
        }
        self.update_viewport();
        cx.notify();
    }

    // ── Wikilink autocomplete ─────────────────────────────────────────────────

    /// Check if the cursor is inside an open `[[…` span and update the popup.
    ///
    /// Called after every Insert-mode keystroke that may have changed the buffer.
    fn check_wikilink_trigger(&mut self, cx: &mut Context<Self>) {
        // Only active in Insert mode.
        if self.state.mode != Mode::Insert {
            self.wikilink_complete = None;
            return;
        }

        let pos = self.state.cursor();
        let line = self.buffer.line(pos.line).to_string();
        let col = pos.col.min(line.len());

        // Scan backward from cursor for `[[` without an intervening `]]`.
        let prefix = &line[..col];
        let open_col = if let Some(idx) = prefix.rfind("[[") {
            // Ensure no `]]` between `[[` and cursor.
            if prefix[idx + 2..].contains("]]") {
                None
            } else {
                Some(idx)
            }
        } else {
            None
        };

        if let Some(open) = open_col {
            let fragment = &line[open + 2..col];
            // Gather matching vault titles.
            let fragment_lower = fragment.to_ascii_lowercase();
            let candidates: Vec<String> = if let Some(ref vault_entity) = self.vault {
                vault_entity
                    .read(cx)
                    .files
                    .iter()
                    .filter(|f| f.title.to_ascii_lowercase().contains(&fragment_lower))
                    .map(|f| f.title.clone())
                    .take(8)
                    .collect()
            } else {
                Vec::new()
            };

            // Keep selected index in range (reset if candidates changed significantly).
            let selected = self
                .wikilink_complete
                .as_ref()
                .map(|s| s.selected.min(candidates.len().saturating_sub(1)))
                .unwrap_or(0);

            if !candidates.is_empty() {
                self.wikilink_complete = Some(WikilinkState {
                    open_col: open,
                    candidates,
                    selected,
                });
            } else {
                self.wikilink_complete = None;
            }
        } else {
            self.wikilink_complete = None;
        }

        cx.notify();
    }

    /// Apply the currently selected wikilink completion, replacing `[[fragment` with `[[title]]`.
    fn apply_wikilink_completion(&mut self, cx: &mut Context<Self>) {
        let (open_col, title) = match &self.wikilink_complete {
            Some(s) if !s.candidates.is_empty() => {
                (s.open_col, s.candidates[s.selected].clone())
            }
            _ => return,
        };

        let pos = self.state.cursor();
        let line = self.buffer.line(pos.line).to_string();
        let cursor_col = pos.col.min(line.len());

        // Build the replacement: `[[Title]]`.
        let replacement = format!("[[{}]]", title);

        // Strategy: move cursor to open_col, delete to cursor_col (via Backspace
        // from cursor_col), then insert the replacement string.
        self.push_undo();

        // Delete the `[[fragment` span (chars from open_col to cursor_col).
        let chars_to_delete = line[open_col..cursor_col].chars().count();
        // Position cursor at cursor_col and backspace chars_to_delete times.
        self.state.move_cursor_to(Pos::new(pos.line, cursor_col));
        for _ in 0..chars_to_delete {
            let (ns, _) = apply(
                EditorCommand::DeleteCharBefore,
                std::mem::take(&mut self.state),
                &mut self.buffer,
            );
            self.state = ns;
        }

        // Insert the replacement string as a single `Insert` command.
        let (ns, _) = apply(
            EditorCommand::Insert(replacement),
            std::mem::take(&mut self.state),
            &mut self.buffer,
        );
        self.state = ns;

        self.wikilink_complete = None;
        self.update_viewport();
        self.trigger_compile(cx);
        cx.notify();
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let _perf_start: Option<std::time::Instant> = if self.perf_overlay {
            Some(std::time::Instant::now())
        } else {
            None
        };
        let k = &event.keystroke;

        // ── Global shortcuts handled before the keymap ────────────────────
        // Cmd-Z: undo (macOS convention — works in all modes).
        if k.modifiers.platform && k.key == "z" && !k.modifiers.shift {
            self.do_undo(cx);
            return;
        }

        // Cmd-Shift-Z: redo (macOS convention).
        if k.modifiers.platform && k.key == "z" && k.modifiers.shift {
            self.do_redo(cx);
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

        // Search bar routing: while `/` search is open, send non-Cmd keys there.
        if self.search.is_some() && !k.modifiers.platform {
            self.handle_search_key(k, cx);
            cx.stop_propagation();
            return;
        }

        // Wikilink autocomplete navigation.
        if self.wikilink_complete.is_some() && !k.modifiers.platform {
            let is_ctrl_k = k.modifiers.control && k.key == "k";
            let is_ctrl_j = k.modifiers.control && k.key == "j";
            if is_ctrl_k || k.key == "up" {
                if let Some(ref mut s) = self.wikilink_complete {
                    if s.selected > 0 {
                        s.selected -= 1;
                    } else {
                        s.selected = s.candidates.len().saturating_sub(1);
                    }
                }
                cx.stop_propagation();
                cx.notify();
                return;
            }
            if is_ctrl_j || k.key == "down" {
                if let Some(ref mut s) = self.wikilink_complete {
                    s.selected = (s.selected + 1) % s.candidates.len().max(1);
                }
                cx.stop_propagation();
                cx.notify();
                return;
            }
            if !k.modifiers.control {
                match k.key.as_str() {
                    "tab" | "enter" => {
                        self.apply_wikilink_completion(cx);
                        cx.stop_propagation();
                        return;
                    }
                    "escape" => {
                        self.wikilink_complete = None;
                        cx.notify();
                        // fall through to keymap
                    }
                    _ => {}
                }
            }
        }

        // Cmd-C / Cmd-X: copy / cut to OS clipboard.
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
                self.push_undo();
                let prev = std::mem::take(&mut self.state);
                let (new_state, _) = apply(EditorCommand::DeleteLine, prev, &mut self.buffer);
                self.state = new_state;
                self.trigger_compile(cx);
            }
            cx.notify();
            return;
        }

        // ── Delegate to the keymap handler ────────────────────────────────
        let result = self.keymap.handle_key(event, &self.state);
        self.execute_keymap_result(result, cx);

        if let Some(start) = _perf_start {
            self.last_key_micros = Some(start.elapsed().as_micros());
        }
    }

    /// Execute a `KeymapResult` produced by the keymap handler.
    fn execute_keymap_result(
        &mut self,
        result: KeymapResult,
        cx: &mut Context<Self>,
    ) {
        match result {
            KeymapResult::Passthrough => {}
            KeymapResult::Pending => {
                cx.stop_propagation();
            }
            KeymapResult::Undo => {
                self.do_undo(cx);
            }
            KeymapResult::Redo => {
                self.do_redo(cx);
            }
            KeymapResult::OpenSearch { backward } => {
                self.open_search(backward);
                cx.stop_propagation();
                cx.notify();
            }
            KeymapResult::SearchNext => {
                if self.last_search.is_some() {
                    self.search_next(cx);
                    cx.stop_propagation();
                }
            }
            KeymapResult::SearchPrev => {
                if self.last_search.is_some() {
                    self.search_prev(cx);
                    cx.stop_propagation();
                }
            }
            KeymapResult::SearchWordForward => {
                if let Some(word) = self.word_under_cursor() {
                    self.last_search = Some(word);
                    self.last_search_backward = false;
                    self.search_next(cx);
                    cx.stop_propagation();
                }
            }
            KeymapResult::SearchWordBackward => {
                if let Some(word) = self.word_under_cursor() {
                    self.last_search = Some(word);
                    self.last_search_backward = true;
                    self.search_prev(cx);
                    cx.stop_propagation();
                }
            }
            KeymapResult::OpenPalette => {
                cx.stop_propagation();
                cx.emit(EditorPaneEvent::OpenPalette);
            }
            KeymapResult::OperatorMotion { op, motion } => {
                cx.stop_propagation();
                self.apply_operator_motion(op, motion, cx);
            }
            KeymapResult::OperatorObject { op, inner, kind } => {
                cx.stop_propagation();
                self.apply_operator_object(op, inner, kind, cx);
            }
            KeymapResult::OperatorLinewise(op) => {
                cx.stop_propagation();
                self.push_undo();
                let cmd = match op {
                    OperatorKind::Delete => EditorCommand::DeleteLine,
                    OperatorKind::Change => EditorCommand::ChangeLine,
                    OperatorKind::Yank => EditorCommand::YankLine,
                };
                let prev = std::mem::take(&mut self.state);
                let (new_state, effect) = apply(cmd, prev, &mut self.buffer);
                self.state = new_state;
                if effect == SideEffect::BufferChanged {
                    self.cached_doc_stats = None;
                    self.trigger_compile(cx);
                }
                self.update_viewport();
                cx.notify();
            }
            KeymapResult::Surround { open, close } => {
                self.apply_surround(&open, close, cx);
            }
            KeymapResult::Commands(cmds) => {
                for cmd in cmds {
                    self.execute_command(cmd, cx);
                }
            }
            KeymapResult::Command(cmd) => {
                self.execute_command(cmd, cx);
            }
        }
    }

    /// Execute a single `EditorCommand`: undo snapshot, apply, side effects.
    fn execute_command(
        &mut self,
        cmd: EditorCommand,
        cx: &mut Context<Self>,
    ) {
        // We are handling this key — tell GPUI so macOS doesn't double-insert.
        cx.stop_propagation();

        // Auto-close / skip-over in Insert mode.
        if let EditorCommand::Insert(ref typed) = cmd {
            if self.state.mode == Mode::Insert && self.handle_auto_close(typed.clone()) {
                self.update_viewport();
                self.trigger_compile(cx);
                cx.notify();
                return;
            }
        }

        // Smart backspace.
        if cmd == EditorCommand::DeleteCharBefore && self.state.mode == Mode::Insert {
            if self.handle_pair_backspace() {
                self.update_viewport();
                self.trigger_compile(cx);
                cx.notify();
                return;
            }
        }

        let mutating = is_buffer_mutating(&cmd);

        if mutating {
            self.push_undo();
        }

        let record = if mutating && cmd != EditorCommand::RepeatLastChange {
            Some(cmd.clone())
        } else {
            None
        };

        let prev_state = std::mem::take(&mut self.state);
        let (new_state, effect) = apply(cmd, prev_state, &mut self.buffer);
        self.state = new_state;

        if let Some(c) = record {
            self.state.last_change = Some(vec![c]);
        }

        self.update_viewport();

        if self.state.mode == Mode::Insert && mutating {
            self.check_wikilink_trigger(cx);
        } else if self.state.mode != Mode::Insert {
            self.wikilink_complete = None;
        }

        match effect {
            SideEffect::BufferChanged => {
                self.cached_doc_stats = None;
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

    /// Perform undo (shared between Cmd-Z and keymap Undo).
    fn do_undo(&mut self, cx: &mut Context<Self>) {
        if let Some((text, pos)) = self.undo_history.pop() {
            let redo_snap = (self.buffer.text(), self.state.cursor());
            self.redo_history.push(redo_snap);
            self.buffer = InMemoryBuffer::from_text(&text);
            self.state.mode = Mode::Normal;
            self.state.move_cursor_to(pos);
            self.state.is_dirty = true;
            self.cached_doc_stats = None;
            self.update_viewport();
            self.trigger_compile(cx);
            cx.notify();
        }
    }

    /// Perform redo (shared between Cmd-Shift-Z and keymap Redo).
    fn do_redo(&mut self, cx: &mut Context<Self>) {
        if let Some((text, pos)) = self.redo_history.pop() {
            self.push_undo_keeping_redo();
            self.buffer = InMemoryBuffer::from_text(&text);
            self.state.mode = Mode::Normal;
            self.state.move_cursor_to(pos);
            self.state.is_dirty = true;
            self.cached_doc_stats = None;
            self.update_viewport();
            self.trigger_compile(cx);
            cx.notify();
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

        // Compute matching bracket position (if cursor is on a bracket).
        let bracket_match = find_bracket_match(&self.buffer, cursor);

        // Virtual rendering: only emit the lines visible in the current viewport.
        // This bounds element creation to O(VIEWPORT_LINES) regardless of document
        // length, keeping layout work constant as the file grows.
        let first = self.viewport_top;
        let last = (first + VIEWPORT_LINES).min(line_count);

        // Per-line search highlight data (only computed when search bar is open).
        let (search_query_len, search_matches_ref) = if let Some(ref s) = self.search {
            (s.query.len(), Some((&s.matches, s.current_idx)))
        } else {
            (0, None)
        };

        let lnm = self.line_number_mode;

        let mut line_elements = Vec::with_capacity(last - first);
        for i in first..last {
            let text = self.buffer.line(i).to_string();
            let in_selection = is_line_in_visual_selection(i, mode, &selection);
            let is_front_matter = front_matter_end.map(|end| i < end).unwrap_or(false);

            // Collect search match byte-ranges that fall on this line.
            let (focused_match, other_matches): (Option<(usize, usize)>, Vec<(usize, usize)>) =
                if let Some((matches, cur_idx)) = search_matches_ref {
                    if search_query_len > 0 {
                        let focused = matches
                            .get(cur_idx)
                            .filter(|p| p.line == i)
                            .map(|p| (p.col, p.col + search_query_len));
                        let others: Vec<_> = matches
                            .iter()
                            .enumerate()
                            .filter(|&(idx, p)| p.line == i && idx != cur_idx)
                            .map(|(_, p)| (p.col, p.col + search_query_len))
                            .collect();
                        (focused, others)
                    } else {
                        (None, vec![])
                    }
                } else {
                    (None, vec![])
                };

            // Highest-severity diagnostic on this line (error beats warning).
            let diag_sev: Option<DiagnosticSeverity> = self.diagnostics.iter()
                .filter(|d| d.line == Some(i))
                .fold(None, |acc, d| Some(match acc {
                    None => d.severity.clone(),
                    Some(DiagnosticSeverity::Error) => DiagnosticSeverity::Error,
                    Some(DiagnosticSeverity::Warning) => d.severity.clone(),
                }));

            // Bracket match highlight: byte range on this line (if any).
            let bracket_range: Option<(usize, usize)> = bracket_match
                .filter(|m| m.line == i)
                .map(|m| {
                    let row = self.buffer.line(i);
                    let end = m.col + row[m.col..].chars().next()
                        .map(|c| c.len_utf8()).unwrap_or(1);
                    (m.col, end)
                });

            line_elements.push(render_line(
                i, text, cursor, mode, in_selection, is_front_matter,
                lnm, line_count, &other_matches, focused_match, diag_sev,
                bracket_range, &t,
            ));
        }

        // ── Document stats ─────────────────────────────────────────────────
        let line_count_total: usize = self.buffer.line_count();
        if self.cached_doc_stats.is_none() {
            let stats = self.compute_doc_stats();
            self.cached_doc_stats = Some(stats);
        }
        let (word_count, char_count) = self.cached_doc_stats.unwrap();
        let doc_stats_label = format!("{} w · {} ch · {} L", word_count, char_count, line_count_total);

        // When a Visual selection is active, also show per-selection stats.
        let sel_stats_label: Option<String> = if matches!(mode, Mode::Visual(_)) {
            let sel   = &self.state.selection;
            let start = sel.start();
            let end   = sel.end();
            if start != end {
                let selected: String = if start.line == end.line {
                    self.buffer.line(start.line)
                        .get(start.col..end.col)
                        .unwrap_or("")
                        .to_string()
                } else {
                    let mut parts: Vec<String> = Vec::new();
                    parts.push(
                        self.buffer.line(start.line)
                            .get(start.col..)
                            .unwrap_or("")
                            .to_string(),
                    );
                    for l in (start.line + 1)..end.line {
                        parts.push(self.buffer.line(l).to_string());
                    }
                    parts.push(
                        self.buffer.line(end.line)
                            .get(..end.col)
                            .unwrap_or("")
                            .to_string(),
                    );
                    parts.join("\n")
                };
                let sel_words = selected.split_whitespace().count();
                let sel_chars = selected.chars().count();
                Some(format!("{} w · {} ch sel", sel_words, sel_chars))
            } else {
                None
            }
        } else {
            None
        };

        let mode_label = self.keymap.mode_label(&self.state).to_string();
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
            .child(if let Some(ref lbl) = sel_stats_label {
                div()
                    .text_color(gpui::rgb(t.mode_visual))
                    .font_family("Menlo")
                    .child(lbl.clone())
                    .into_any_element()
            } else {
                div().into_any_element()
            })
            .child(
                div()
                    .text_color(gpui::rgb(t.text_faint))
                    .font_family("Menlo")
                    .child(doc_stats_label),
            )
            .child(if let Some((cur, total, wrapped)) = self.search_nav_status {
                // Show match position from the last n/N navigation when the
                // search bar is closed.
                if self.search.is_none() {
                    let label = if wrapped {
                        format!("{}/{} ↩", cur, total)
                    } else {
                        format!("{}/{}", cur, total)
                    };
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .text_color(gpui::rgb(t.text_faint))
                                .font_family("Menlo")
                                .child(label),
                        )
                        .into_any_element()
                } else {
                    div().into_any_element()
                }
            } else {
                div().into_any_element()
            })
            .child(if self.state.is_dirty {
                div()
                    .text_color(gpui::rgb(t.ochre))
                    .child("●")
                    .into_any_element()
            } else {
                div().into_any_element()
            });

        // ── Search bar (shown at top when `/`, `?`, or Cmd-H is active) ────────
        let search_bar = self.search.as_ref().map(|s| {
            let match_info: gpui::AnyElement = if s.query.is_empty() {
                div().into_any_element()
            } else if s.matches.is_empty() {
                div()
                    .text_color(gpui::rgb(0xF87171))
                    .child("  no matches")
                    .into_any_element()
            } else {
                let count_str = format!("  [{}/{}]", s.current_idx + 1, s.matches.len());
                let wrap_str = if s.wrapped { "  ↩ wrap" } else { "" };
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(
                        div()
                            .text_color(gpui::rgb(t.text_faint))
                            .child(count_str),
                    )
                    .child(
                        div()
                            .text_color(gpui::rgb(t.ochre))
                            .child(wrap_str),
                    )
                    .into_any_element()
            };

            let prompt = if s.backward { "?" } else { "/" };
            // Cursor blink indicator appears on whichever row is focused.
            let cursor_el = div().text_color(gpui::rgb(t.mode_insert)).child("▌");

            // Query row — always shown.
            let query_row = div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .px_4()
                .py_1()
                .bg(gpui::rgb(t.bg_base))
                .text_sm()
                .font_family("Menlo")
                .child(div().text_color(gpui::rgb(t.ochre)).child(prompt))
                .child(div().text_color(gpui::rgb(t.text)).child(s.query.clone()))
                .when(!s.replace_focused, |r| r.child(cursor_el))
                .child(match_info);

            let mut bar = div()
                .w_full()
                .flex()
                .flex_col()
                .bg(gpui::rgb(t.bg_base))
                .border_b_1()
                .border_color(gpui::rgb(t.border_subtle))
                .child(query_row);

            // Replace row — only shown in find-and-replace mode.
            if let Some(ref replace_text) = s.replace {
                let hint = div()
                    .text_color(gpui::rgb(t.text_faint))
                    .text_xs()
                    .child("  ↵ replace  ^A all  Tab switch");
                let replace_row = div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .px_4()
                    .py_1()
                    .border_t_1()
                    .border_color(gpui::rgb(t.border_subtle))
                    .text_sm()
                    .font_family("Menlo")
                    .child(div().text_color(gpui::rgb(t.text_subtle)).child("→"))
                    .child(div().text_color(gpui::rgb(t.text)).child(replace_text.clone()))
                    .when(s.replace_focused, |r| {
                        r.child(div().text_color(gpui::rgb(t.mode_insert)).child("▌"))
                    })
                    .child(hint);
                bar = bar.child(replace_row);
            }

            bar
        });

        // ── Wikilink autocomplete popup ───────────────────────────────────────
        // Positioned immediately below and to the right of the `[[` opening on
        // the cursor line, using approximate fixed character metrics.
        let wikilink_popup = self.wikilink_complete.as_ref().map(|wl| {
            const CHAR_W: f32 = 8.4;
            const LINE_H: f32 = 20.0;
            const PADDING: f32 = 16.0;
            // Gutter contributes roughly (digits × CHAR_W) + 12 px right-padding.
            let gutter_w = if lnm != LineNumberMode::Off {
                gutter_digits(line_count) as f32 * CHAR_W + 12.0
            } else {
                0.0
            };
            // Leave vertical room for the search bar when it is open.
            let search_h = if search_bar.is_some() { 28.0_f32 } else { 0.0 };
            let popup_left = px(PADDING + gutter_w + wl.open_col as f32 * CHAR_W);
            let popup_top = px(
                search_h
                    + PADDING
                    + (cursor.line.saturating_sub(self.viewport_top) + 1) as f32 * LINE_H,
            );

            let items: Vec<gpui::AnyElement> = wl
                .candidates
                .iter()
                .enumerate()
                .map(|(i, title)| {
                    let is_sel = i == wl.selected;
                    div()
                        .px_2()
                        .py(px(3.0))
                        .text_sm()
                        .font_family("Menlo")
                        .text_color(gpui::rgb(if is_sel { t.cursor_fg } else { t.text }))
                        .when(is_sel, |d| d.bg(gpui::rgb(t.ochre)))
                        .child(format!("[[{}]]", title))
                        .into_any_element()
                })
                .collect();

            div()
                .absolute()
                .top(popup_top)
                .left(popup_left)
                .min_w(px(220.0))
                .bg(gpui::rgb(t.bg_surface))
                .border_1()
                .border_color(gpui::rgb(t.ochre_border))
                .rounded(px(6.0))
                .shadow_lg()
                .overflow_hidden()
                .children(items)
        });

        div()
            .relative()
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(gpui::rgb(t.bg_panel))
            .on_action(cx.listener(|this, _: &SaveFile, _window, cx| {
                this.save(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &OpenSearch, _window, cx| {
                this.open_search(false);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &OpenReplace, _window, cx| {
                this.open_replace();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &LineNumbersRelative, _, cx| {
                this.line_number_mode = LineNumberMode::Relative;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &LineNumbersAbsolute, _, cx| {
                this.line_number_mode = LineNumberMode::Absolute;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &LineNumbersOff, _, cx| {
                this.line_number_mode = LineNumberMode::Off;
                cx.notify();
            }))
            .on_action(cx.listener(Self::follow_link))
            .on_key_down(cx.listener(Self::handle_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::handle_mouse_down))
            .when_some(search_bar, |root, bar| root.child(bar))
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
            .when_some(wikilink_popup, |root, popup| root.child(popup))
            .when_some(
                if self.perf_overlay { self.last_key_micros } else { None },
                |root, micros| {
                    let label = if micros >= 1_000 {
                        format!("{:.1} ms", micros as f64 / 1_000.0)
                    } else {
                        format!("{} µs", micros)
                    };
                    root.child(
                        div()
                            .absolute()
                            .bottom(px(28.0))
                            .right(px(8.0))
                            .px_2()
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .bg(gpui::rgba(0x000000cc))
                            .text_xs()
                            .font_family("Menlo")
                            .text_color(gpui::rgb(0x00ff88))
                            .child(label),
                    )
                },
            )
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

/// Render one editor line, including optional gutter, syntax highlighting,
/// search-match backgrounds, selection background, and the block/bar cursor.
///
/// All styling concerns are composed via a single segment-building pass:
/// boundaries are collected from syntax spans, search ranges, and the cursor
/// position, then each interval gets the highest-priority style applied.
#[allow(clippy::too_many_arguments)]
/// Number of digits needed to display line numbers up to `count`.
/// Minimum of 2 so the gutter always has a reasonable width.
fn gutter_digits(count: usize) -> usize {
    if count < 10 { 2 }
    else if count < 100 { 3 }
    else if count < 1000 { 4 }
    else { 5 }
}

/// A single syntax-highlight span on a line.
#[derive(Clone, Copy)]
struct SyntaxSpan {
    start: usize,
    end: usize,
    color: u32,
    bold: bool,
    italic: bool,
}

impl SyntaxSpan {
    fn new(start: usize, end: usize, color: u32) -> Self {
        Self { start, end, color, bold: false, italic: false }
    }
    fn bold(mut self) -> Self { self.bold = true; self }
    fn italic(mut self) -> Self { self.italic = true; self }
}

/// Compute syntax-highlight spans for a single line.
///
/// Returns a sorted, non-overlapping list of `SyntaxSpan` values.
/// Text not covered by any span uses the caller-supplied default foreground.
/// Front-matter lines are left unhighlighted (returned as empty vec).
fn highlight_spans(line: &str, t: &ThemePalette, is_front_matter: bool) -> Vec<SyntaxSpan> {
    if is_front_matter || line.is_empty() {
        return Vec::new();
    }

    let b = line.as_bytes();
    let n = b.len();
    let mut spans: Vec<SyntaxSpan> = Vec::new();

    // Full-line: Typst headings (`=`, `==`, `===`, …)
    {
        let eq_count = b.iter().take_while(|&&c| c == b'=').count();
        if eq_count > 0 && (eq_count == n || b[eq_count] == b' ') {
            spans.push(SyntaxSpan::new(0, n, t.syntax_heading).bold());
            return spans;
        }
    }

    let mut i = 0usize;
    while i < n {
        // `//` comment — captures rest of line.
        if b[i] == b'/' && i + 1 < n && b[i + 1] == b'/' {
            spans.push(SyntaxSpan::new(i, n, t.syntax_comment).italic());
            break;
        }

        // `[[wikilink]]`
        if b[i] == b'[' && i + 1 < n && b[i + 1] == b'[' {
            if let Some(rel) = line[i + 2..].find("]]") {
                let end = i + 2 + rel + 2;
                spans.push(SyntaxSpan::new(i, end, t.syntax_link));
                i = end;
                continue;
            }
        }

        // `inline code`
        if b[i] == b'`' {
            if let Some(rel) = line[i + 1..].find('`') {
                let end = i + 1 + rel + 1;
                spans.push(SyntaxSpan::new(i, end, t.syntax_code));
                i = end;
                continue;
            }
        }

        // `$math$`
        if b[i] == b'$' {
            if let Some(rel) = line[i + 1..].find('$') {
                let end = i + 1 + rel + 1;
                spans.push(SyntaxSpan::new(i, end, t.syntax_math));
                i = end;
                continue;
            }
        }

        // `*bold*` — must not be whitespace immediately inside delimiters.
        if b[i] == b'*' {
            let inner_start = i + 1;
            if inner_start < n && b[inner_start] != b' ' {
                if let Some(rel) = line[inner_start..].find('*') {
                    let end = inner_start + rel + 1;
                    // Avoid matching `**` (empty bold).
                    if rel > 0 {
                        spans.push(SyntaxSpan::new(i, end, t.text).bold());
                        i = end;
                        continue;
                    }
                }
            }
        }

        // `_italic_` — must not be whitespace immediately inside delimiters.
        if b[i] == b'_' {
            let inner_start = i + 1;
            if inner_start < n && b[inner_start] != b' ' && b[inner_start] != b'_' {
                if let Some(rel) = line[inner_start..].find('_') {
                    let end = inner_start + rel + 1;
                    if rel > 0 {
                        spans.push(SyntaxSpan::new(i, end, t.text).italic());
                        i = end;
                        continue;
                    }
                }
            }
        }

        // `#keyword` — Typst directive
        if b[i] == b'#'
            && i + 1 < n
            && (b[i + 1].is_ascii_alphabetic() || b[i + 1] == b'_')
        {
            let start = i;
            i += 1;
            while i < n && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            spans.push(SyntaxSpan::new(start, i, t.syntax_keyword));
            continue;
        }

        i += 1;
    }

    spans
}

/// Map a `PendingOp` to the corresponding selection-operator `EditorCommand`.
fn operator_kind_to_command(op: OperatorKind) -> EditorCommand {
    match op {
        OperatorKind::Delete => EditorCommand::DeleteSelection,
        OperatorKind::Change => EditorCommand::ChangeSelection,
        OperatorKind::Yank   => EditorCommand::YankSelection,
    }
}

/// Find all occurrences of `query` in `buf`, returning the start `Pos` of each.
/// Returns an empty vec if `query` is empty.
fn find_all_matches(buf: &InMemoryBuffer, query: &str) -> Vec<Pos> {
    let mut out = Vec::new();
    if query.is_empty() {
        return out;
    }
    for line_idx in 0..buf.line_count() {
        let line = buf.line(line_idx);
        let mut offset = 0usize;
        while let Some(rel) = line[offset..].find(query) {
            out.push(Pos::new(line_idx, offset + rel));
            // Advance by at least 1 to avoid infinite loops on zero-width matches.
            offset += rel + query.len().max(1);
        }
    }
    out
}

fn render_line(
    line_idx: usize,
    text: String,
    cursor: Pos,
    mode: Mode,
    in_selection: bool,
    is_front_matter: bool,
    line_number_mode: LineNumberMode,
    total_lines: usize,
    other_matches: &[(usize, usize)],
    focused_match: Option<(usize, usize)>,
    diag_sev: Option<DiagnosticSeverity>,
    bracket_range: Option<(usize, usize)>,
    t: &ThemePalette,
) -> gpui::AnyElement {
    let line_height = px(20.0);
    let is_cursor_line = line_idx == cursor.line;
    let text_len = text.len();
    let default_fg = if is_front_matter { t.text_faint } else { t.text_muted };

    // ── Gutter ────────────────────────────────────────────────────────────────
    let gutter_cell = if line_number_mode != LineNumberMode::Off {
        let w = gutter_digits(total_lines);
        let (label, color) = if is_cursor_line {
            (format!("{:>width$}", line_idx + 1, width = w), t.text_subtle)
        } else {
            let n: isize = match line_number_mode {
                LineNumberMode::Absolute => (line_idx + 1) as isize,
                LineNumberMode::Relative => {
                    (line_idx as isize - cursor.line as isize).abs()
                }
                LineNumberMode::Off => unreachable!(),
            };
            (format!("{:>width$}", n, width = w), t.text_faint)
        };
        Some(
            div()
                .flex_shrink_0()
                .pr_3()
                .text_sm()
                .font_family("Menlo")
                .text_color(gpui::rgb(color))
                .child(label),
        )
    } else {
        None
    };

    // ── Syntax spans ──────────────────────────────────────────────────────────
    let spans = highlight_spans(&text, t, is_front_matter);

    // ── Cursor character bounds ───────────────────────────────────────────────
    let cursor_col = if is_cursor_line { cursor.col.min(text_len) } else { usize::MAX };
    let cursor_end = if is_cursor_line && cursor_col < text_len {
        text[cursor_col..]
            .char_indices()
            .nth(1)
            .map(|(b, _)| cursor_col + b)
            .unwrap_or(text_len)
    } else {
        text_len
    };

    // ── Collect boundary points ────────────────────────────────────────────────
    let mut boundaries: Vec<usize> = Vec::with_capacity(16);
    boundaries.push(0);
    boundaries.push(text_len);
    if is_cursor_line {
        boundaries.push(cursor_col);
        boundaries.push(cursor_end);
    }
    for sp in &spans {
        boundaries.push(sp.start);
        boundaries.push(sp.end);
    }
    for &(s, e) in other_matches {
        boundaries.push(s);
        boundaries.push(e);
    }
    if let Some((s, e)) = focused_match {
        boundaries.push(s);
        boundaries.push(e);
    }
    if let Some((s, e)) = bracket_range {
        boundaries.push(s);
        boundaries.push(e);
    }
    // Keep only valid UTF-8 char boundaries inside the string.
    boundaries.retain(|&b| b <= text_len && text.is_char_boundary(b));
    boundaries.sort_unstable();
    boundaries.dedup();

    // ── Mode-derived colours ──────────────────────────────────────────────────
    let cursor_bg: gpui::Hsla = match mode {
        Mode::Insert   => gpui::rgb(t.mode_insert).into(),
        Mode::Normal   => gpui::rgb(t.mode_normal).into(),
        Mode::Visual(_)=> gpui::rgb(t.mode_visual).into(),
    };
    let cursor_fg: gpui::Hsla = gpui::rgb(t.cursor_fg).into();
    let sel_bg: gpui::Hsla    = gpui::rgb(t.ochre_dim).into();
    // Non-current match: translucent ochre tint.
    let match_bg: gpui::Hsla  = rgba(((t.ochre as u64) << 8 | 0x55) as u32).into();
    // Current (focused) match: solid ochre, inverted text.
    let focused_bg: gpui::Hsla = gpui::rgb(t.ochre).into();
    // Matching bracket highlight.
    let bracket_bg: gpui::Hsla = gpui::rgb(t.bracket_match_bg).into();

    // ── Build TextRun list ────────────────────────────────────────────────────
    // Each run covers a byte range of the line text.  We use `StyledText` with
    // explicit `TextRun`s so the whole line is a single text layout that wraps
    // naturally at word boundaries when wider than the editor column.
    let base_font = gpui::font("Menlo");
    let mut runs: Vec<gpui::TextRun> = Vec::with_capacity(boundaries.len().saturating_sub(1));

    // Append a trailing space when the cursor sits at end-of-line so the EOL
    // cursor position is visible (the space gets the cursor highlight).
    let eol_cursor = is_cursor_line && cursor_col >= text_len;
    let text_content: String = if eol_cursor {
        format!("{} ", text)
    } else {
        text.clone()
    };

    for w in boundaries.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a >= b { continue; }
        if text.get(a..b).is_none() { continue; }

        let is_cursor_block =
            is_cursor_line && a == cursor_col && cursor_col < text_len && mode != Mode::Insert;
        let is_cursor_bar =
            is_cursor_line && a == cursor_col && mode == Mode::Insert;

        // Foreground and font attributes from syntax spans (or default).
        let active_span = spans.iter().find(|sp| sp.start <= a && a < sp.end);
        let syn_fg: gpui::Hsla = gpui::rgb(
            active_span.map(|sp| sp.color).unwrap_or(default_fg)
        ).into();
        let syn_bold   = active_span.map(|sp| sp.bold).unwrap_or(false);
        let syn_italic = active_span.map(|sp| sp.italic).unwrap_or(false);

        let font = gpui::Font {
            weight: if syn_bold { gpui::FontWeight::BOLD } else { gpui::FontWeight::NORMAL },
            style:  if syn_italic { gpui::FontStyle::Italic } else { gpui::FontStyle::Normal },
            ..base_font.clone()
        };

        let (fg, bg, underline) = if is_cursor_block {
            (cursor_fg, Some(cursor_bg), None)
        } else if is_cursor_bar {
            // Insert-mode cursor: render the character normally; the caret bar
            // is painted as an absolute-positioned overlay below.
            (syn_fg, None, None)
        } else if focused_match.map(|(s, e)| s <= a && a < e).unwrap_or(false) {
            (cursor_fg, Some(focused_bg), None)
        } else if other_matches.iter().any(|&(s, e)| s <= a && a < e) {
            (syn_fg, Some(match_bg), None)
        } else if bracket_range.map(|(s, e)| s <= a && a < e).unwrap_or(false) {
            (syn_fg, Some(bracket_bg), None)
        } else if in_selection {
            (syn_fg, Some(sel_bg), None)
        } else {
            (syn_fg, None, None)
        };

        runs.push(gpui::TextRun {
            len: b - a,
            font,
            color: fg,
            background_color: bg,
            underline,
            strikethrough: None,
        });
    }

    // End-of-line cursor run (over the appended trailing space).
    if eol_cursor {
        let (fg, bg) = if mode != Mode::Insert {
            (cursor_fg, Some(cursor_bg))
        } else {
            // Insert cursor at EOL: show a plain space; the bar overlay below
            // handles the visual caret.
            (gpui::rgb(t.text_muted).into(), None)
        };
        runs.push(gpui::TextRun {
            len: 1, // the trailing space appended above
            font: base_font.clone(),
            color: fg,
            background_color: bg,
            underline: None,
            strikethrough: None,
        });
    }

    // ── Insert-mode cursor bar ────────────────────────────────────────────────
    // Overlay a 2 px vertical bar at the cursor's character position.
    // Menlo at text_sm (14 px / 0.875 rem) has a fixed advance of ~8.4 px.
    // The bar is absolutely positioned relative to the content div so it sits
    // exactly at the left edge of the character under the caret.
    const CHAR_W: f32 = 8.4;
    let insert_bar: Option<gpui::AnyElement> = if is_cursor_line && mode == Mode::Insert {
        // Convert byte cursor to character column for pixel positioning.
        let char_col = text[..cursor_col.min(text_len)].chars().count();
        Some(
            div()
                .absolute()
                .left(px(char_col as f32 * CHAR_W))
                .top(px(0.0))
                .w(px(2.0))
                .h(px(20.0)) // line_height
                .bg(gpui::rgb(t.mode_insert))
                .into_any_element(),
        )
    } else {
        None
    };

    // ── Assemble content ──────────────────────────────────────────────────────
    // `StyledText` renders the whole line as a single text layout that wraps at
    // word boundaries when the available width is narrower than the text.  This
    // gives proper soft-wrap behaviour — the same logical line number covers
    // multiple visual rows.  `flex_1` ensures the content div fills the width
    // left over after the gutter so the wrap constraint is the editor column.
    // `min_w_0()` zeroes the flex item's default `min-width: auto` so taffy
    // can shrink the content div below its text's natural (unwrapped) width.
    // Without it the flex algorithm honours the minimum content size, the div
    // overflows the row, and text escapes past the editor boundary.
    // With `min_w_0()` taffy assigns a definite known_dimensions.width equal
    // to the flex-allocated width, which StyledText receives as its wrap
    // constraint, producing proper soft word-wrap.
    let content = if runs.is_empty() {
        // Empty line — render a zero-width placeholder so the row has min_h.
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .text_sm()
            .font_family("Menlo")
            .child("")
            .when_some(insert_bar, |d, bar| d.child(bar))
            .into_any_element()
    } else {
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .text_sm()
            .font_family("Menlo")
            .child(gpui::StyledText::new(text_content).with_runs(runs))
            .when_some(insert_bar, |d, bar| d.child(bar))
            .into_any_element()
    };

    // ── Assemble row ──────────────────────────────────────────────────────────
    let mut row = div()
        .min_h(line_height)
        .flex()
        .flex_row();

    // Diagnostic left-border indicator: red stripe for errors, amber for warnings.
    if let Some(sev) = diag_sev {
        let stripe_color = match sev {
            DiagnosticSeverity::Error   => gpui::rgb(0xff5555u32),
            DiagnosticSeverity::Warning => gpui::rgb(0xffb86cu32),
        };
        row = row.border_l_2().border_color(stripe_color);
    }

    if let Some(g) = gutter_cell {
        row = row.child(g);
    }
    row.child(content).into_any_element()
}

// ── Key translation ───────────────────────────────────────────────────────────


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
            | ChangeSelection
            | ReplaceChar(_)
            | DeleteWordBefore
            | IndentLines
            | DedentLines
            | SwitchCase
            | RepeatLastChange
            | ReplaceWithYanked
            | AutoIndent
            | DeleteToLineStart
            | DeleteRestOfLine
            | ToggleComment
    )
}

/// Find the position of the bracket matching the one at `cursor`.
///
/// Returns `None` if the character at the cursor is not a bracket or no
/// matching bracket is found within the buffer.
fn find_bracket_match(buf: &InMemoryBuffer, cursor: Pos) -> Option<Pos> {
    use crate::editor::buffer::Buffer;
    let line = buf.line(cursor.line);
    if cursor.col >= line.len() {
        return None;
    }
    let ch = line[cursor.col..].chars().next()?;
    let (open, close, forward) = match ch {
        '(' => ('(', ')', true),
        ')' => ('(', ')', false),
        '[' => ('[', ']', true),
        ']' => ('[', ']', false),
        '{' => ('{', '}', true),
        '}' => ('{', '}', false),
        _ => return None,
    };

    let line_count = buf.line_count();
    let mut depth: i32 = 0;

    if forward {
        // Scan from cursor position forward.
        let mut start_col = cursor.col;
        for l in cursor.line..line_count {
            let row = buf.line(l);
            let scan_from = if l == cursor.line { start_col } else { 0 };
            for (byte_idx, c) in row[scan_from..].char_indices() {
                let col = scan_from + byte_idx;
                if c == open  { depth += 1; }
                else if c == close {
                    depth -= 1;
                    if depth == 0 {
                        return Some(Pos::new(l, col));
                    }
                }
            }
            start_col = 0;
        }
    } else {
        // Scan from cursor position backward.
        let mut end_col = cursor.col + ch.len_utf8();
        for l in (0..=cursor.line).rev() {
            let row = buf.line(l);
            let scan_to = if l == cursor.line { end_col } else { row.len() };
            let slice = &row[..scan_to];
            // Collect char positions in reverse order.
            let chars: Vec<(usize, char)> = slice.char_indices().collect();
            for &(byte_idx, c) in chars.iter().rev() {
                if c == close { depth += 1; }
                else if c == open {
                    depth -= 1;
                    if depth == 0 {
                        return Some(Pos::new(l, byte_idx));
                    }
                }
            }
            end_col = row.len();
        }
    }
    None
}

/// Convert a character index (as typed) to a byte offset, clamped to valid
/// UTF-8 boundaries within `s`.
fn byte_offset_for_char(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

// ── Delimiter helpers ─────────────────────────────────────────────────────────

/// In Insert mode: return the closing delimiter to auto-insert after `open`.
/// Returns `None` if no auto-close should apply for this character.
fn insert_auto_close_pair(open: &str) -> Option<&'static str> {
    match open {
        "(" => Some(")"),
        "[" => Some("]"),
        "{" => Some("}"),
        "\"" => Some("\""),
        "$" => Some("$"),
        _ => None,
    }
}

impl EditorPane {
    /// Smart backspace: when the cursor is directly between an auto-inserted
    /// pair with nothing in between (e.g. `(|)`), delete both delimiters.
    ///
    /// Returns `true` if the pair was deleted (caller should skip normal apply).
    /// Returns `false` if the cursor is not between a matching pair — the
    /// caller then falls through to the regular `DeleteCharBefore` path.
    fn handle_pair_backspace(&mut self) -> bool {
        let cursor = self.state.cursor();
        if cursor.col == 0 { return false; }

        let line = self.buffer.line(cursor.line).to_string();

        // Character immediately before the cursor (the potential opener).
        let before = line.get(..cursor.col)
            .and_then(|s| s.chars().last())
            .map(|c| c.to_string());
        // Character at the cursor (the potential closer).
        let at = line.get(cursor.col..)
            .and_then(|s| s.chars().next())
            .map(|c| c.to_string());

        let Some(open) = before else { return false };
        let Some(close_at_cursor) = at else { return false };

        // Only fire when open + close form a recognised pair.
        if insert_auto_close_pair(&open) != Some(close_at_cursor.as_str()) {
            return false;
        }

        self.push_undo();
        // Delete open and close in one range: [cursor.col-open.len(), cursor.col+close.len())
        let del_start = cursor.col - open.len();
        let del_end   = cursor.col + close_at_cursor.len();
        self.buffer.delete_range(cursor.line, del_start, del_end);
        self.state.move_cursor_to(Pos::new(cursor.line, del_start));
        self.state.is_dirty = true;
        true
    }

    /// Auto-close or skip-over a delimiter in Insert mode.
    ///
    /// Returns `true` if the keystroke was handled (caller should skip the
    /// normal `apply` path and call `trigger_compile` + `notify` instead).
    ///
    /// **Skip-over:** if the cursor is sitting on the exact character that was
    /// typed (e.g. the auto-inserted `)`) and that character is a closing
    /// delimiter, move the cursor past it without inserting.
    ///
    /// **Auto-close:** for opening delimiters, insert the pair and place the
    /// cursor between them.
    fn handle_auto_close(&mut self, typed: String) -> bool {
        let cursor = self.state.cursor();
        let line_text = self.buffer.line(cursor.line).to_string();

        // Check the character currently at the cursor position.
        let char_at_cursor = line_text.get(cursor.col..)
            .and_then(|s| s.get(..typed.len()))
            .map(str::to_string);

        // Skip-over: close delimiters (and symmetric delimiters like " and $)
        // when the auto-inserted closing char is already there.
        let is_close = matches!(typed.as_str(), ")" | "]" | "}" | "\"" | "$");
        if is_close && char_at_cursor.as_deref() == Some(typed.as_str()) {
            self.state.move_cursor_to(Pos::new(cursor.line, cursor.col + typed.len()));
            return true; // no buffer change, no undo snapshot needed
        }

        // Auto-close: insert the pair and place cursor between them.
        if let Some(close) = insert_auto_close_pair(&typed) {
            self.push_undo();
            self.buffer.insert(cursor.line, cursor.col, &format!("{}{}", typed, close));
            self.state.move_cursor_to(Pos::new(cursor.line, cursor.col + typed.len()));
            self.state.is_dirty = true;
            return true;
        }

        false
    }

    /// Wrap the current Visual selection in `open` + `close` delimiters.
    ///
    /// Inserts `close` after the selection end first, then `open` before the
    /// start, so the end-position byte offset isn't perturbed by the first
    /// insertion when both are on the same line.  Exits Visual mode and
    /// positions the cursor on the closing delimiter.
    fn apply_surround(&mut self, open: &str, close: &str, cx: &mut Context<Self>) {
        cx.stop_propagation();
        self.push_undo();

        let start = self.state.selection.start();
        let end   = self.state.selection.end();

        // Insert close first (at the character after the selection end).
        self.buffer.insert(end.line, end.col + 1, close);
        // Insert open at the selection start (offsets only shift on the same line).
        self.buffer.insert(start.line, start.col, open);

        // The closing delimiter's final column accounts for the open insertion
        // shifting everything on the same line.
        let close_col = if end.line == start.line {
            end.col + 1 + open.len()
        } else {
            end.col + 1
        };

        self.state.mode = Mode::Normal;
        self.state.move_cursor_to(Pos::new(end.line, close_col));
        self.state.is_dirty = true;

        self.update_viewport();
        self.trigger_compile(cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{
        command::EditorCommand,
        keymap::KeymapHandler,
        keymap_helix::HelixKeymap,
        state::Mode,
    };

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
        let mut km = HelixKeymap::new();
        let mut state = EditorState::new();
        state.mode = Mode::Insert;
        let event = make_key("a");
        assert!(matches!(km.handle_key(&event, &state), KeymapResult::Command(EditorCommand::Insert(ref s)) if s == "a"));
    }

    #[test]
    fn escape_enters_normal_mode_command() {
        let mut km = HelixKeymap::new();
        let mut state = EditorState::new();
        state.mode = Mode::Insert;
        let event = make_key("escape");
        assert!(matches!(km.handle_key(&event, &state), KeymapResult::Command(EditorCommand::EnterNormal)));
    }

    #[test]
    fn normal_mode_h_is_move_left() {
        let mut km = HelixKeymap::new();
        let mut state = EditorState::new();
        state.mode = Mode::Normal;
        let event = make_key("h");
        assert!(matches!(km.handle_key(&event, &state), KeymapResult::Command(EditorCommand::MoveLeft)));
    }

    #[test]
    fn normal_mode_x_selects_line() {
        let mut km = HelixKeymap::new();
        let mut state = EditorState::new();
        state.mode = Mode::Normal;
        let event = make_key("x");
        assert!(matches!(km.handle_key(&event, &state), KeymapResult::Command(EditorCommand::SelectCurrentLine)));
    }

    #[test]
    fn visual_mode_d_deletes_selection() {
        let mut km = HelixKeymap::new();
        let mut state = EditorState::new();
        state.mode = Mode::Visual(VisualKind::Char);
        let event = make_key("d");
        assert!(matches!(km.handle_key(&event, &state), KeymapResult::Command(EditorCommand::DeleteSelection)));
    }
}
