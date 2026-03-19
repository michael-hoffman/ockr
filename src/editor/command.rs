//! Editor commands — the typed enum of every operation the editor can perform.
//!
//! Commands are the sole input to the editor state machine (`apply`). Nothing
//! else mutates `EditorState`. This makes the editor fully testable without a
//! UI, fully recordable for macros, and safely extensible.
//!
//! Story 07: Vi/Helix Normal mode operators, insert variants, yank/paste,
//!           Visual(Line) selection, and the change operator.

/// A single editor operation.
#[derive(Debug, Clone, PartialEq)]
pub enum EditorCommand {
    // ── Text mutation ──────────────────────────────────────────────────────
    /// Insert a string at the current cursor position.
    Insert(String),
    /// Delete the character before the cursor (Backspace).
    DeleteCharBefore,
    /// Delete the character at the cursor (Delete key in Insert mode).
    DeleteCharAt,

    // ── Line operations ────────────────────────────────────────────────────
    /// Insert a newline at the cursor (Enter key in Insert mode).
    InsertNewline,
    /// Delete the current line into the yank register (`dd`).
    DeleteLine,
    /// Delete from the cursor to end-of-line into the yank register (`D`).
    DeleteToLineEnd,
    /// Yank (copy) the current line into the register (`yy`).
    YankLine,
    /// Yank from the cursor to end-of-line (`Y`).
    YankToLineEnd,
    /// Paste the yank register as a new line below the cursor (`p`).
    /// For non-line register content: paste after cursor character.
    PasteAfter,
    /// Paste the yank register as a new line above the cursor (`P`).
    /// For non-line register content: paste before cursor character.
    PasteBefore,
    /// Insert text from the OS clipboard at the cursor position (Cmd-V).
    PasteFromClipboard(String),

    // ── Insert-mode entry variants ─────────────────────────────────────────
    /// Enter Insert mode at the current cursor (basic `i`).
    EnterInsert,
    /// Append: move one character right, then enter Insert mode (`a`).
    AppendAfterCursor,
    /// Insert at line start (col 0) (`I`).
    InsertLineStart,
    /// Insert at line end (`A`).
    InsertLineEnd,
    /// Open a new line below the current line and enter Insert mode (`o`).
    OpenLineBelow,
    /// Open a new line above the current line and enter Insert mode (`O`).
    OpenLineAbove,

    // ── Change (delete + insert) ───────────────────────────────────────────
    /// Delete the current line content and enter Insert mode (`cc`).
    ChangeLine,
    /// Delete from cursor to EOL and enter Insert mode (`C`).
    ChangeToLineEnd,

    // ── Selection (Helix-style) ────────────────────────────────────────────
    /// Select the current line and enter Visual(Line) mode (`x`).
    /// Helix semantics: `x` selects; operators (`d`, `y`, `c`) act on the selection.
    SelectCurrentLine,
    /// Delete the active Visual selection into the yank register (`d` in Visual).
    DeleteSelection,
    /// Yank the active Visual selection into the register (`y` in Visual).
    YankSelection,

    // ── Cursor movement ────────────────────────────────────────────────────
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    MoveStartOfLine,
    MoveEndOfLine,
    MoveStartOfDocument,
    MoveEndOfDocument,
    /// Move forward one word (to the start of the next word).
    MoveWordForward,
    /// Move backward one word (to the start of the previous word).
    MoveWordBackward,

    // ── Mode transitions ───────────────────────────────────────────────────
    /// Enter Normal mode (Escape / Ctrl-[).
    EnterNormal,

    // ── No-op ──────────────────────────────────────────────────────────────
    /// Discard the key with no effect (unknown binding in Normal mode, etc.).
    Noop,
}
