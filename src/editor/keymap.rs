//! Keyboard mode abstraction — Story 23.
//!
//! `KeymapHandler` is the trait that decouples keystroke interpretation from
//! the rest of the editor.  Each keyboard mode (Helix, Standard) implements
//! this trait.  `EditorPane` owns a `Box<dyn KeymapHandler>` and delegates
//! mode-specific key interpretation to it.
//!
//! The handler returns a `KeymapResult` describing what should happen;
//! `EditorPane` then executes it (undo snapshots, `apply()`, compile, etc.).

use gpui::KeyDownEvent;

use super::command::{EditorCommand, TextObjectKind};
use super::state::EditorState;

// ── Result enum ──────────────────────────────────────────────────────────────

/// The operator half of a vim-style operator-motion/object sequence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OperatorKind {
    Delete,
    Change,
    Yank,
}

/// What the cursor should look like.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CursorStyle {
    /// Full-cell block cursor (Helix Normal/Visual).
    Block,
    /// Thin vertical line (Standard mode, Helix Insert).
    Line,
}

/// Result of `KeymapHandler::handle_key`.
///
/// The caller (`EditorPane`) interprets these and performs the actual
/// state mutation, undo management, compilation, etc.
#[derive(Debug)]
pub enum KeymapResult {
    /// Apply a single `EditorCommand` through the state machine.
    Command(EditorCommand),
    /// Apply a sequence of `EditorCommand`s in order.
    ///
    /// Used when a single keypress must trigger multiple operations atomically
    /// (e.g. the first Shift+Arrow in Standard mode: enter Visual + move).
    Commands(Vec<EditorCommand>),
    /// Undo request (Helix `u`).
    Undo,
    /// Redo request (Helix `Ctrl-r`).
    Redo,
    /// Open the in-buffer search bar.
    OpenSearch { backward: bool },
    /// Jump to the next match of the last search.
    SearchNext,
    /// Jump to the previous match of the last search.
    SearchPrev,
    /// Search for the word under the cursor (forward).
    SearchWordForward,
    /// Search for the word under the cursor (backward).
    SearchWordBackward,
    /// Open the command palette (Helix `:` key).
    OpenPalette,
    /// Operator + single-key motion (e.g. `dw`, `cj`).
    OperatorMotion {
        op: OperatorKind,
        motion: EditorCommand,
    },
    /// Operator + text object (e.g. `ci"`, `da(`).
    OperatorObject {
        op: OperatorKind,
        inner: bool,
        kind: TextObjectKind,
    },
    /// Operator + doubled key → linewise (e.g. `dd`, `yy`).
    OperatorLinewise(OperatorKind),
    /// Wrap the Visual selection in delimiters.
    Surround {
        open: String,
        close: &'static str,
    },
    /// Open the in-buffer search scoped to the current visual selection.
    /// Matches outside the selection are hidden; `n`/`N` stay within bounds.
    SelectInSelection,
    /// Jump to the next (forward=true) or previous (forward=false) diagnostic.
    JumpDiagnostic { forward: bool },
    /// Key consumed; multi-key sequence in progress.
    Pending,
    /// Not handled by the keymap.  Caller should check global shortcuts.
    Passthrough,
    /// Begin recording keystrokes into register `reg`.
    StartMacro(char),
    /// Finish recording and save the macro.
    StopMacro,
    /// Replay the macro stored in register `reg`.
    PlayMacro(char),
    /// Follow the link or file path under the cursor (`gf` / `gx`).
    FollowLink,
    /// Navigate to the next (`forward = true`) or previous buffer (`gn` / `gp`).
    BufferNav { forward: bool },
    /// Reposition the viewport relative to the cursor without moving the cursor.
    ScrollViewport(ViewportAlign),
}

/// How to align the cursor line within the visible viewport (`z` commands).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ViewportAlign {
    /// Cursor line at the top of the viewport (`zt`, `z<Enter>`).
    Top,
    /// Cursor line centred in the viewport (`zz`, `z.`).
    Center,
    /// Cursor line at the bottom of the viewport (`zb`, `z-`).
    Bottom,
    /// Scroll the viewport down one line; cursor follows if it leaves view (`zj`).
    LineDown,
    /// Scroll the viewport up one line; cursor follows if it leaves view (`zk`).
    LineUp,
}

// ── Trait ─────────────────────────────────────────────────────────────────────

/// A pluggable keyboard mode.
///
/// Implementations translate raw GPUI key events into `KeymapResult` values
/// that the `EditorPane` can execute generically.
pub trait KeymapHandler: Send {
    /// Translate a keystroke into an action, given the current editor state.
    ///
    /// The handler may maintain internal pending-key state (e.g. multi-key
    /// sequences in Helix mode).  It must NOT modify editor state directly.
    fn handle_key(&mut self, event: &KeyDownEvent, state: &EditorState) -> KeymapResult;

    /// Label shown in the status bar (e.g. "NORMAL", "INSERT", "STANDARD").
    fn mode_label(&self, state: &EditorState) -> &str;

    /// How the cursor should render in the current state.
    fn cursor_style(&self, state: &EditorState) -> CursorStyle;

    /// Notify the keymap that macro recording has started (`true`) or stopped
    /// (`false`).  The keymap may use this to adjust key handling (e.g. `q`
    /// to stop recording when active).  Default implementation is a no-op.
    fn set_macro_recording(&mut self, _active: bool) {}
}
