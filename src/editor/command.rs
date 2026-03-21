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
    /// Enter Visual Char mode at the current cursor (`v`).
    EnterVisualChar,
    /// Enter Visual Line mode, selecting the whole current line (`V`).
    EnterVisualLine,
    /// Enter Visual Block mode at the current cursor (`Ctrl-V`).
    EnterVisualBlock,
    /// Re-enter the last visual selection (`gv`).
    ReselectLastVisual,
    /// Delete the active Visual selection into the yank register (`d` in Visual).
    DeleteSelection,
    /// Yank the active Visual selection into the register (`y` in Visual).
    YankSelection,
    /// Delete the visual selection and enter Insert mode (`c` in Visual).
    ChangeSelection,
    /// Indent the selected (or current) lines by one level (`>`).
    IndentLines,
    /// Dedent the selected (or current) lines by one level (`<`).
    DedentLines,

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
    /// Move to the end of the current/next word (`e`).
    MoveWordEnd,
    /// Move forward one WORD (whitespace-delimited; `W`).
    MoveWORDForward,
    /// Move backward one WORD (`B`).
    MoveWORDBackward,
    /// Move to the end of the current/next WORD (`E`).
    MoveWORDEnd,
    /// Move to the first non-whitespace character on the current line (`^`).
    MoveFirstNonWhitespace,
    /// Scroll (move cursor) half a page down — ~20 lines (`Ctrl-d`).
    ScrollHalfDown,
    /// Scroll (move cursor) half a page up — ~20 lines (`Ctrl-u`).
    ScrollHalfUp,

    /// Replace the character under the cursor with the given char and stay in Normal (`r<c>`).
    ReplaceChar(String),
    /// Delete the word before the cursor in Insert mode (`Ctrl-w`).
    DeleteWordBefore,
    /// Collapse the current Visual selection to its cursor endpoint and return to Normal (`;`).
    CollapseSelection,
    /// Trim leading/trailing whitespace from the selection bounds (`_`).
    /// In Visual mode the selection is shrunk to exclude whitespace at both ends;
    /// the mode switches to Visual Char if it was Visual Line.
    /// In Normal mode the cursor moves to the first non-whitespace char (identical to `^`).
    TrimSelection,

    // ── Text object selection (Helix `mi` / `ma`) ─────────────────────────
    /// Select a text object, entering Visual Char mode.
    ///
    /// Helix grammar: user selects with `mi<char>` / `ma<char>`, then acts
    /// (`d`, `y`, `c`).  `inner = true` ↔ `i` (no delimiters/trailing space).
    SelectObject { inner: bool, kind: TextObjectKind },

    // ── Mode transitions ───────────────────────────────────────────────────
    /// Enter Normal mode (Escape / Ctrl-[).
    EnterNormal,

    // ── UI commands (dispatched to the window, not the buffer) ─────────────
    /// Open the command palette (`:` in Normal mode, Cmd-P globally).
    OpenPalette,

    // ── No-op ──────────────────────────────────────────────────────────────
    /// Discard the key with no effect (unknown binding in Normal mode, etc.).
    Noop,
}

// ── Text object kinds ─────────────────────────────────────────────────────────

/// Which flavour of text object `SelectObject` targets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextObjectKind {
    /// Word (`w`): alphanumeric / `_` run.
    Word,
    /// WORD (`W`): any non-whitespace run.
    WORD,
    /// Paragraph (`p`): blank-line-delimited block.
    Paragraph,
    /// Parentheses `(` / `)`.
    Paren,
    /// Braces `{` / `}`.
    Brace,
    /// Brackets `[` / `]`.
    Bracket,
    /// Angle brackets `<` / `>`.
    Angle,
    /// Double-quoted string `"`.
    DoubleQuote,
    /// Single-quoted string `'`.
    SingleQuote,
    /// Backtick string `` ` ``.
    Backtick,
    /// Typst inline-math zone `$…$`.
    InlineMath,
    /// Typst content block `[…]` (alias for Bracket but distinct intent).
    TypstContent,
}
