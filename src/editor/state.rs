//! Editor state — the complete snapshot of the editor at any point in time.
//!
//! `EditorState` is the input and output type of the editor's pure state
//! machine (`apply`). It carries no I/O handles and no rendering concerns.
//! Only things that *logically* belong to the editor's current status live
//! here: cursor position, selection, mode, dirty flag.

/// A `(line, column)` position in the buffer. Both are 0-indexed.
/// `col` is a *byte* offset within the line's UTF-8 string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

impl Pos {
    pub fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}

/// Whether the cursor is inside a selection and which end is the anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// The fixed end of the selection (where it was started).
    pub anchor: Pos,
    /// The moving end of the selection (where the cursor currently is).
    pub cursor: Pos,
}

impl Selection {
    pub fn collapsed(pos: Pos) -> Self {
        Self {
            anchor: pos,
            cursor: pos,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.cursor
    }

    /// The earlier position of the two endpoints.
    pub fn start(&self) -> Pos {
        if self.anchor.line < self.cursor.line
            || (self.anchor.line == self.cursor.line && self.anchor.col <= self.cursor.col)
        {
            self.anchor
        } else {
            self.cursor
        }
    }

    /// The later position of the two endpoints.
    pub fn end(&self) -> Pos {
        if self.start() == self.anchor {
            self.cursor
        } else {
            self.anchor
        }
    }
}

/// The editor's modal state.
///
/// Insert is the default starting mode. Normal and Visual arrive in Story 07.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Insert,
    Normal,
    Visual(VisualKind),
}

/// Discriminates the three Visual sub-modes (Story 13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualKind {
    /// Character-level selection (`v`).
    Char,
    /// Whole-line selection (`V`).
    Line,
    /// Column/block selection (`Ctrl-V`).
    Block,
}

/// Complete snapshot of the editor. Cheap to clone.
///
/// Story 03: position, selection, mode, dirty flag.
/// Story 07: yank register added here; undo history lives in `EditorPane`
///           (it requires buffer snapshots which are not part of pure state).
#[derive(Debug, Clone)]
pub struct EditorState {
    pub selection: Selection,
    pub mode: Mode,
    /// Whether the buffer contains unsaved changes.
    pub is_dirty: bool,
    /// Path of the open file, if any.
    pub path: Option<std::path::PathBuf>,
    /// Unnamed yank register ("clipboard" internal to ockr).
    /// Set by `yy`/`dd`/`x`; read by `p`/`P`.
    pub yank_register: String,
}

impl EditorState {
    pub fn new() -> Self {
        Self {
            selection: Selection::collapsed(Pos::default()),
            mode: Mode::Insert,
            is_dirty: false,
            path: None,
            yank_register: String::new(),
        }
    }

    /// The current cursor position (the moving end of the selection).
    pub fn cursor(&self) -> Pos {
        self.selection.cursor
    }

    /// Move the cursor to `pos`, collapsing the selection.
    pub fn move_cursor_to(&mut self, pos: Pos) {
        self.selection = Selection::collapsed(pos);
    }
}

impl Default for EditorState {
    fn default() -> Self {
        Self::new()
    }
}
