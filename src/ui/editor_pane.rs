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

use crate::actions::{FollowLink, LineNumbersAbsolute, LineNumbersOff, LineNumbersRelative, SaveFile};
use crate::compiler::{preprocess::{normalise, preprocess_wikilinks}, CompileRequest, CompilerHandle, PreviewMode};
use crate::editor::buffer::Buffer as _;
use crate::editor::{
    apply::{apply, SideEffect},
    buffer::InMemoryBuffer,
    command::{EditorCommand, TextObjectKind},
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
    /// Text typed after `[[` and before the cursor.
    fragment: String,
    /// Byte offset of the opening `[` on the current line.
    open_col: usize,
    /// Vault file titles that match `fragment` (prefix-insensitive).
    candidates: Vec<String>,
    /// Currently highlighted candidate (0-based index into `candidates`).
    selected: usize,
}

/// State for the in-buffer `/` or `?` search bar.
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
    /// `m` was pressed; awaiting `i` (inner) or `a` (around).
    M,
    /// `mi` pressed; awaiting the text-object character.
    MatchInner,
    /// `ma` pressed; awaiting the text-object character.
    MatchAround,
    /// `f` pressed; awaiting the target character (`f<c>`).
    FindChar,
    /// `F` pressed; awaiting the target character (`F<c>`).
    FindCharBack,
    /// `t` pressed; awaiting the target character (`t<c>`).
    TillChar,
    /// `T` pressed; awaiting the target character (`T<c>`).
    TillCharBack,
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
    /// Active `[[` autocomplete popup state; `Some` while in Insert mode inside `[[…`.
    wikilink_complete: Option<WikilinkState>,
    /// Active search bar state; `Some` while `/` search is open.
    search: Option<SearchState>,
    /// Query from the last completed search; used by `n`/`N` repeat navigation.
    last_search: Option<String>,
    /// Direction of the last completed search (`true` = backward / `?`).
    last_search_backward: bool,
    /// How line numbers are rendered in the gutter.
    line_number_mode: LineNumberMode,
    /// Plugin-provided typst packages forwarded to each CompileRequest.
    plugin_packages: Option<std::sync::Arc<std::sync::RwLock<std::collections::HashMap<String, String>>>>,
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
            wikilink_complete: None,
            search: None,
            last_search: None,
            last_search_backward: false,
            line_number_mode: LineNumberMode::Relative,
            plugin_packages: None,
            compile_sequence: 0,
            viewport_top: 0,
        }
    }

    pub fn set_vault(&mut self, vault: Entity<VaultState>) {
        self.vault = Some(vault);
    }

    /// Share the plugin packages map so every CompileRequest carries it.
    pub fn set_plugin_packages(
        &mut self,
        packages: std::sync::Arc<std::sync::RwLock<std::collections::HashMap<String, String>>>,
    ) {
        self.plugin_packages = Some(packages);
    }

    /// Vault-relative path of the currently open file, if any.
    pub fn current_rel_path(&self) -> Option<&str> {
        self.file_rel_path.as_deref()
    }

    /// Absolute path of the currently open file, if any.
    pub fn current_path(&self) -> Option<&std::path::PathBuf> {
        self.state.path.as_ref()
    }

    /// Current cursor position.
    pub fn cursor_pos(&self) -> Pos {
        self.state.cursor()
    }

    /// Index of the topmost visible line in the viewport.
    pub fn viewport_top(&self) -> usize {
        self.viewport_top
    }

    /// Restore the viewport top (used when switching tabs).
    pub fn set_viewport_top(&mut self, top: usize) {
        self.viewport_top = top;
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

    // ── Search ────────────────────────────────────────────────────────────────

    /// Open the search bar (`/` forward, `?` backward), saving cursor for Escape-cancel.
    fn open_search(&mut self, backward: bool) {
        let saved = self.state.cursor();
        self.search = Some(SearchState {
            query: String::new(),
            matches: Vec::new(),
            current_idx: 0,
            saved_cursor: saved,
            backward,
        });
    }

    /// Route a keystroke to the search bar while it is open.
    fn handle_search_key(&mut self, k: &gpui::Keystroke, cx: &mut Context<Self>) {
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
            "enter" => {
                // Confirm: persist query and direction for n/N, close bar.
                if let Some(ref s) = self.search {
                    if !s.query.is_empty() {
                        self.last_search = Some(s.query.clone());
                        self.last_search_backward = s.backward;
                    }
                }
                self.search = None;
            }
            "backspace" => {
                if let Some(ref mut s) = self.search {
                    s.query.pop();
                }
                self.update_search_matches(cx);
            }
            _ => {
                if !k.modifiers.platform && !k.modifiers.control {
                    if let Some(ch) = &k.key_char {
                        if let Some(ref mut s) = self.search {
                            s.query.push_str(ch);
                        }
                        self.update_search_matches(cx);
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
        let idx = if matches.is_empty() {
            0
        } else if backward {
            matches
                .iter()
                .rposition(|p| {
                    p.line < saved.line
                        || (p.line == saved.line && p.col <= saved.col)
                })
                .unwrap_or(matches.len() - 1)
        } else {
            matches
                .iter()
                .position(|p| {
                    p.line > saved.line
                        || (p.line == saved.line && p.col >= saved.col)
                })
                .unwrap_or(0)
        };

        let dest = matches.get(idx).copied();

        if let Some(s) = &mut self.search {
            s.matches = matches;
            s.current_idx = idx;
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
        let idx = if backward {
            matches
                .iter()
                .rposition(|p| p.line < cursor.line || (p.line == cursor.line && p.col < cursor.col))
                .unwrap_or(matches.len() - 1)
        } else {
            matches
                .iter()
                .position(|p| p.line > cursor.line || (p.line == cursor.line && p.col > cursor.col))
                .unwrap_or(0)
        };
        self.state.move_cursor_to(matches[idx]);
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
        let idx = if backward {
            matches
                .iter()
                .position(|p| p.line > cursor.line || (p.line == cursor.line && p.col > cursor.col))
                .unwrap_or(0)
        } else {
            matches
                .iter()
                .rposition(|p| p.line < cursor.line || (p.line == cursor.line && p.col < cursor.col))
                .unwrap_or(matches.len() - 1)
        };
        self.state.move_cursor_to(matches[idx]);
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
                    fragment: fragment.to_string(),
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

        // Cmd-P / Cmd-Shift-P: open command palette.
        // Stop propagation BEFORE emitting so the `cmd-p` key-binding action
        // (`OpenCommandPalette`) does not also fire — that would call `open_palette`
        // which sees palette.is_some() == true (set by the emit path) and toggles
        // it immediately back closed.
        if k.modifiers.platform && k.key == "p" {
            cx.stop_propagation();
            cx.emit(EditorPaneEvent::OpenPalette);
            return;
        }

        // Cmd-Z: undo (macOS convention — works in all modes).
        if k.modifiers.platform && k.key == "z" && !k.modifiers.shift {
            if let Some((text, pos)) = self.undo_history.pop() {
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

        // Cmd-Shift-Z: redo (macOS convention).
        if k.modifiers.platform && k.key == "z" && k.modifiers.shift {
            if let Some((text, pos)) = self.redo_history.pop() {
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

        // Wikilink autocomplete navigation: intercept Up/Down/Ctrl-K/J/Tab/Enter while popup is open.
        if self.wikilink_complete.is_some() && !k.modifiers.platform {
            // Check ctrl-k / ctrl-j first (modifiers.control + base key).
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
            // Non-ctrl keys below (Tab/Enter/Escape don't fire with ctrl).
            if !k.modifiers.control {
                match k.key.as_str() {
                    "tab" | "enter" => {
                        self.apply_wikilink_completion(cx);
                        cx.stop_propagation();
                        return;
                    }
                    "escape" => {
                        // Dismiss popup but do NOT stop propagation or return —
                        // let Escape continue through key_insert so it also
                        // triggers EnterNormal.  Without this the user is trapped
                        // in Insert mode after closing the popup.
                        self.wikilink_complete = None;
                        cx.notify();
                        // fall through to normal key handling
                    }
                    _ => {}
                }
            }
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

        // ── `m` text-object sequences (Helix: mi<obj> / ma<obj>) ────────────
        // Available in both Normal and Visual modes.
        let in_modal = matches!(self.state.mode, Mode::Normal | Mode::Visual(_));
        if in_modal && k.key == "m" && !k.modifiers.platform && !k.modifiers.control {
            self.pending = PendingKey::M;
            cx.stop_propagation();
            return;
        }
        if self.pending == PendingKey::M {
            self.pending = PendingKey::None;
            match k.key.as_str() {
                "i" => { self.pending = PendingKey::MatchInner; cx.stop_propagation(); return; }
                "a" => { self.pending = PendingKey::MatchAround; cx.stop_propagation(); return; }
                _ => {} // fall through — cancel sequence
            }
        }
        if matches!(self.pending, PendingKey::MatchInner | PendingKey::MatchAround) {
            let inner = self.pending == PendingKey::MatchInner;
            self.pending = PendingKey::None;
            // Map keystroke to a text object kind.
            let kind = if !k.modifiers.platform && !k.modifiers.control {
                match k.key_char.as_deref().unwrap_or(&k.key) {
                    "w"  => Some(TextObjectKind::Word),
                    "W"  => Some(TextObjectKind::WORD),
                    "p"  => Some(TextObjectKind::Paragraph),
                    "("  | ")" => Some(TextObjectKind::Paren),
                    "{"  | "}" => Some(TextObjectKind::Brace),
                    "["  | "]" => Some(TextObjectKind::Bracket),
                    "<"  | ">" => Some(TextObjectKind::Angle),
                    "\"" => Some(TextObjectKind::DoubleQuote),
                    "'"  => Some(TextObjectKind::SingleQuote),
                    "`"  => Some(TextObjectKind::Backtick),
                    "$"  => Some(TextObjectKind::InlineMath),
                    "t"  => Some(TextObjectKind::TypstContent),
                    _    => None,
                }
            } else {
                None
            };
            if let Some(kind) = kind {
                cx.stop_propagation();
                let prev = std::mem::take(&mut self.state);
                let (new_state, _) = apply(
                    EditorCommand::SelectObject { inner, kind },
                    prev,
                    &mut self.buffer,
                );
                self.state = new_state;
                cx.notify();
            }
            return;
        }

        // ── `/` and `?` search, `n`/`N` repeat ──────────────────────────────
        if in_modal && !k.modifiers.platform && !k.modifiers.control {
            let is_slash = k.key == "/" || k.key_char.as_deref() == Some("/");
            let is_question = k.key == "?" || k.key_char.as_deref() == Some("?");
            if is_slash || is_question {
                self.open_search(is_question);
                cx.stop_propagation();
                cx.notify();
                return;
            }
            // n / N: navigate matches from last search (when one exists).
            if k.key == "n" && self.last_search.is_some() {
                self.search_next(cx);
                cx.stop_propagation();
                return;
            }
            if k.key == "N" && self.last_search.is_some() {
                self.search_prev(cx);
                cx.stop_propagation();
                return;
            }
        }

        // ── f/F/t/T find-char sequences ──────────────────────────────────────
        // Available in Normal and Visual modes; only when no other sequence pending.
        if in_modal && self.pending == PendingKey::None
            && !k.modifiers.platform && !k.modifiers.control
        {
            let next = match k.key.as_str() {
                "f" => Some(PendingKey::FindChar),
                "F" => Some(PendingKey::FindCharBack),
                "t" => Some(PendingKey::TillChar),
                "T" => Some(PendingKey::TillCharBack),
                _ => None,
            };
            if let Some(pk) = next {
                self.pending = pk;
                cx.stop_propagation();
                return;
            }
        }
        if matches!(
            self.pending,
            PendingKey::FindChar | PendingKey::FindCharBack
            | PendingKey::TillChar | PendingKey::TillCharBack
        ) {
            let pending_kind = self.pending;
            self.pending = PendingKey::None;
            if !k.modifiers.platform && !k.modifiers.control {
                if let Some(ch) = k.key_char.as_ref().and_then(|s| s.chars().next()) {
                    cx.stop_propagation();
                    let cmd = match pending_kind {
                        PendingKey::FindChar     => EditorCommand::FindChar(ch),
                        PendingKey::FindCharBack => EditorCommand::FindCharBack(ch),
                        PendingKey::TillChar     => EditorCommand::TillChar(ch),
                        PendingKey::TillCharBack => EditorCommand::TillCharBack(ch),
                        _ => unreachable!(),
                    };
                    let prev = std::mem::take(&mut self.state);
                    let (new_state, _) = apply(cmd, prev, &mut self.buffer);
                    self.state = new_state;
                    self.update_viewport();
                    cx.notify();
                }
            }
            return;
        }

        let cmd = keystroke_to_command(event, &self.state);
        if cmd == EditorCommand::Noop {
            return;
        }

        // OpenPalette is a UI command — emit an event so MainWindow opens it.
        // Using cx.emit is more reliable than window.dispatch_action across
        // view boundaries.  Stop propagation so no competing action binding
        // can fire and toggle the palette back closed.
        if cmd == EditorCommand::OpenPalette {
            cx.stop_propagation();
            cx.emit(EditorPaneEvent::OpenPalette);
            return;
        }

        // We are handling this key — tell GPUI so it returns YES to macOS.
        // Without this, macOS falls through to [inputContext handleEvent:] which
        // triggers a second insertion via the IME pipeline and doubles every character.
        cx.stop_propagation();

        let mutating = is_buffer_mutating(&cmd);

        // Snapshot before any mutating command.
        if mutating {
            self.push_undo();
        }

        // Record the command for `.` repeat — but not RepeatLastChange itself.
        let record = if mutating && cmd != EditorCommand::RepeatLastChange {
            Some(cmd.clone())
        } else {
            None
        };

        let prev_state = std::mem::take(&mut self.state);
        let (new_state, effect) = apply(cmd, prev_state, &mut self.buffer);
        self.state = new_state;

        // Persist the last-change record after apply (so state is fresh).
        if let Some(c) = record {
            self.state.last_change = Some(vec![c]);
        }

        self.update_viewport();

        // Update wikilink autocomplete after every Insert-mode change.
        if self.state.mode == Mode::Insert && mutating {
            self.check_wikilink_trigger(cx);
        } else if self.state.mode != Mode::Insert {
            // Clear popup when leaving Insert mode.
            self.wikilink_complete = None;
        }

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

            line_elements.push(render_line(
                i, text, cursor, mode, in_selection, is_front_matter,
                lnm, line_count, &other_matches, focused_match, &t,
            ));
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

        // ── Search bar (shown at top when `/` or `?` is active) ──────────────
        let search_bar = self.search.as_ref().map(|s| {
            let match_info: gpui::AnyElement = if s.query.is_empty() {
                div().into_any_element()
            } else if s.matches.is_empty() {
                div()
                    .text_color(gpui::rgb(0xF87171)) // soft red — no matches
                    .child("  no matches")
                    .into_any_element()
            } else {
                div()
                    .text_color(gpui::rgb(t.text_faint))
                    .child(format!("  [{}/{}]", s.current_idx + 1, s.matches.len()))
                    .into_any_element()
            };

            let prompt = if s.backward { "?" } else { "/" };

            div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .px_4()
                .py_1()
                .bg(gpui::rgb(t.bg_base))
                .border_b_1()
                .border_color(gpui::rgb(t.border_subtle))
                .text_sm()
                .font_family("Menlo")
                .child(div().text_color(gpui::rgb(t.ochre)).child(prompt))
                .child(div().text_color(gpui::rgb(t.text)).child(s.query.clone()))
                .child(div().text_color(gpui::rgb(t.mode_insert)).child("▌"))
                .child(match_info)
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
    // Keep only valid UTF-8 char boundaries inside the string.
    boundaries.retain(|&b| b <= text_len && text.is_char_boundary(b));
    boundaries.sort_unstable();
    boundaries.dedup();

    // ── Mode-derived colours ──────────────────────────────────────────────────
    let cursor_rgb = match mode {
        Mode::Insert   => gpui::rgb(t.mode_insert),
        Mode::Normal   => gpui::rgb(t.mode_normal),
        Mode::Visual(_)=> gpui::rgb(t.mode_visual),
    };
    let sel_bg        = gpui::rgb(t.ochre_dim);
    // Non-current match: translucent ochre tint.
    let match_bg      = rgba(((t.ochre as u64) << 8 | 0x55) as u32);
    // Current (focused) match: solid ochre, inverted text.
    let focused_bg    = gpui::rgb(t.ochre);

    // ── Build segments ────────────────────────────────────────────────────────
    let mut segs: Vec<gpui::AnyElement> = Vec::new();

    for w in boundaries.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a >= b { continue; }
        let Some(seg_text) = text.get(a..b) else { continue };

        let is_cursor_block =
            is_cursor_line && a == cursor_col && cursor_col < text_len && mode != Mode::Insert;
        let is_cursor_bar =
            is_cursor_line && a == cursor_col && mode == Mode::Insert;

        // Foreground and font attributes from syntax spans (or default).
        let active_span = spans.iter().find(|sp| sp.start <= a && a < sp.end);
        let syn_fg = active_span.map(|sp| sp.color).unwrap_or(default_fg);
        let syn_bold = active_span.map(|sp| sp.bold).unwrap_or(false);
        let syn_italic = active_span.map(|sp| sp.italic).unwrap_or(false);

        let cell = div().text_sm().font_family("Menlo");
        // Apply bold / italic from syntax spans.
        let cell = if syn_bold {
            cell.font_weight(gpui::FontWeight::BOLD)
        } else {
            cell
        };
        let cell = if syn_italic { cell.italic() } else { cell };

        let cell = if is_cursor_block {
            cell.text_color(gpui::rgb(t.cursor_fg)).bg(cursor_rgb)
        } else if is_cursor_bar {
            cell.text_color(gpui::rgb(syn_fg))
                .border_l_2()
                .border_color(cursor_rgb)
        } else if focused_match.map(|(s, e)| s <= a && a < e).unwrap_or(false) {
            cell.text_color(gpui::rgb(t.cursor_fg)).bg(focused_bg)
        } else if other_matches.iter().any(|&(s, e)| s <= a && a < e) {
            cell.text_color(gpui::rgb(syn_fg)).bg(match_bg)
        } else if in_selection {
            cell.text_color(gpui::rgb(syn_fg)).bg(sel_bg)
        } else {
            cell.text_color(gpui::rgb(syn_fg))
        };

        segs.push(cell.child(seg_text.to_string()).into_any_element());
    }

    // Cursor at end-of-line (when cursor_col >= text_len).
    if is_cursor_line && cursor_col >= text_len {
        let eol = if mode != Mode::Insert {
            div()
                .text_sm().font_family("Menlo")
                .text_color(gpui::rgb(t.cursor_fg))
                .bg(cursor_rgb)
                .child(" ")
        } else {
            div()
                .text_sm().font_family("Menlo")
                .text_color(gpui::rgb(t.text_muted))
                .border_l_2()
                .border_color(cursor_rgb)
                .child(" ")
        };
        segs.push(eol.into_any_element());
    }

    // ── Assemble row ──────────────────────────────────────────────────────────
    let content_row = div().flex().flex_row().children(segs);

    let mut row = div()
        .min_h(line_height)
        .flex()
        .flex_row()
        .whitespace_nowrap()
        .overflow_x_hidden();

    if let Some(g) = gutter_cell {
        row = row.child(g);
    }
    row.child(content_row).into_any_element()
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
            "f" => ScrollPageDown,
            "b" => ScrollPageUp,
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
        // `X` extends the selection to include the line below (or selects the current line in Normal).
        "X" => ExtendLineBelow,
        // `R` replaces the current line with the yank register (register is not modified).
        "R" => ReplaceWithYanked,
        // `=` re-indents the current line to match the previous non-empty line.
        "=" => AutoIndent,
        // Visual-mode entry
        "v" => EnterVisualChar,
        "V" => EnterVisualLine,
        // Indent / dedent (single-line, same key as visual-mode versions)
        ">" => IndentLines,
        "<" => DedentLines,
        // Paragraph navigation
        "{" => MoveParagraphBack,
        "}" => MoveParagraphForward,
        // Select whole file (Helix `%`)
        "%" => SelectWholeFile,
        // Switch case of char under cursor
        "~" => SwitchCase,
        // Repeat last change
        "." => RepeatLastChange,
        _ => Noop,
    }
}

fn key_visual(event: &KeyDownEvent) -> EditorCommand {
    use EditorCommand::*;
    let k = &event.keystroke;
    if k.modifiers.platform {
        return Noop;
    }
    // Ctrl combos — Ctrl-V cycles back to Visual Block; Ctrl-d/u/f/b scroll.
    if k.modifiers.control {
        return match k.key.as_str() {
            "v" => EnterVisualBlock,
            "d" => ScrollHalfDown,
            "u" => ScrollHalfUp,
            "f" => ScrollPageDown,
            "b" => ScrollPageUp,
            _ => Noop,
        };
    }
    // Alt combos in Visual mode.
    if k.modifiers.alt {
        return match k.key.as_str() {
            // Alt-; flips the selection direction (swaps anchor and cursor).
            ";" => FlipSelection,
            _ => Noop,
        };
    }
    match k.key.as_str() {
        "escape" => EnterNormal,
        // Operators on selection
        "d" | "x" => DeleteSelection,
        "y" => YankSelection,
        "c" => ChangeSelection,
        // Replace selection with yank register (register unchanged).
        "R" => ReplaceWithYanked,
        // Extend selection to include the next line below.
        "X" => ExtendLineBelow,
        // Re-indent selected lines to match the previous non-empty line.
        "=" => AutoIndent,
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
        // Paragraph navigation (extends selection)
        "{" => MoveParagraphBack,
        "}" => MoveParagraphForward,
        // Select whole file
        "%" => SelectWholeFile,
        // Switch case of selection
        "~" => SwitchCase,
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
            "u" => DeleteToLineStart,   // Ctrl-u: delete from line start to cursor
            "k" => DeleteRestOfLine,    // Ctrl-k: delete from cursor to line end
            "j" => InsertNewline,       // Ctrl-j: insert newline (same as Enter)
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
