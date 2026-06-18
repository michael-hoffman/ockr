//! Helix keyboard mode — Story 23.
//!
//! Implements `KeymapHandler` for the Helix-style modal editing workflow.
//! Extracted from the monolithic `handle_key_down` in `editor_pane.rs`.

use gpui::KeyDownEvent;

use super::command::{EditorCommand, TextObjectKind};
use super::keymap::{CursorStyle, KeymapHandler, KeymapResult, OperatorKind, ViewportAlign};
use super::state::{EditorState, Mode};

// ── Pending-key state machine ────────────────────────────────────────────────

/// Tracks a multi-key Normal-mode sequence in progress.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum PendingKey {
    #[default]
    None,
    /// `g` pressed; awaiting second key.
    G,
    /// `r` pressed; awaiting replacement character.
    Replace,
    /// `m` pressed; awaiting `i` or `a`.
    M,
    /// `mi` pressed; awaiting text-object character.
    MatchInner,
    /// `ma` pressed; awaiting text-object character.
    MatchAround,
    /// `f` pressed; awaiting target character.
    FindChar,
    /// `F` pressed.
    FindCharBack,
    /// `t` pressed.
    TillChar,
    /// `T` pressed.
    TillCharBack,
    /// Operator (`d`/`c`/`y`) pressed; awaiting motion or object.
    Operator(OperatorKind),
    /// Operator + `i`; awaiting text-object character.
    OpInner(OperatorKind),
    /// Operator + `a`; awaiting text-object character.
    OpAround(OperatorKind),
    /// Operator + `t`; awaiting target character.
    OpTill(OperatorKind),
    /// Operator + `f`; awaiting target character.
    OpFind(OperatorKind),
    /// `[` pressed; awaiting second key for bracket-navigation.
    OpenBracket,
    /// `]` pressed; awaiting second key for bracket-navigation.
    CloseBracket,
    /// `q` pressed (not recording); awaiting register character.
    MacroRecord,
    /// `@` pressed; awaiting register character to play back.
    MacroPlay,
    /// `z` pressed; awaiting scroll-alignment key.
    Z,
}

// ── HelixKeymap ──────────────────────────────────────────────────────────────

pub struct HelixKeymap {
    pending: PendingKey,
    /// `true` while a macro is actively being recorded.
    recording: bool,
    /// Digits accumulated for a count prefix (e.g. `42` before `G`).
    /// Cleared after every dispatched command.
    count_buf: String,
    /// Count saved when entering a `PendingKey::G` state, so `<N>gg` works.
    pending_count: usize,
}

impl HelixKeymap {
    pub fn new() -> Self {
        Self {
            pending: PendingKey::None,
            recording: false,
            count_buf: String::new(),
            pending_count: 0,
        }
    }
}

impl KeymapHandler for HelixKeymap {
    fn handle_key(&mut self, event: &KeyDownEvent, state: &EditorState) -> KeymapResult {
        let k = &event.keystroke;
        let in_modal = matches!(state.mode, Mode::Normal | Mode::Visual(_));

        // Skip held repeats in Normal/Visual modes.
        if event.is_held && state.mode != Mode::Insert {
            return KeymapResult::Passthrough;
        }

        // ── Undo / Redo (vim keys) ──────────────────────────────────────
        if k.key == "u"
            && !k.modifiers.platform
            && !k.modifiers.control
            && state.mode != Mode::Insert
        {
            return KeymapResult::Undo;
        }
        if k.modifiers.control
            && k.key == "r"
            && !k.modifiers.platform
            && state.mode != Mode::Insert
        {
            return KeymapResult::Redo;
        }

        // ── Jump list (Ctrl-o back / Ctrl-i forward) ────────────────────
        if k.modifiers.control && !k.modifiers.platform && state.mode != Mode::Insert {
            if k.key == "o" {
                return KeymapResult::JumpBack;
            }
            if k.key == "i" {
                return KeymapResult::JumpForward;
            }
        }

        // ── LSP completion (Ctrl-Space, any mode) ───────────────────────
        if k.modifiers.control
            && !k.modifiers.platform
            && (k.key == " " || k.key == "space")
        {
            return KeymapResult::RequestCompletion;
        }

        // ── `r<c>` replace ──────────────────────────────────────────────
        if state.mode == Mode::Normal
            && k.key == "r"
            && !k.modifiers.platform
            && !k.modifiers.control
            && !k.modifiers.shift
        {
            self.pending = PendingKey::Replace;
            return KeymapResult::Pending;
        }
        if self.pending == PendingKey::Replace {
            self.pending = PendingKey::None;
            if state.mode == Mode::Normal {
                if let Some(ch) = &k.key_char {
                    if !k.modifiers.control && !k.modifiers.platform {
                        return KeymapResult::Command(EditorCommand::ReplaceChar(ch.clone()));
                    }
                }
            }
            return KeymapResult::Passthrough;
        }

        // ── Count prefix accumulation (Normal / Visual only) ────────────
        // Digits build a repeat count consumed by G, gg, j, k.
        // `0` alone is MoveStartOfLine; `0` after other digits extends the count.
        if in_modal
            && self.pending == PendingKey::None
            && !k.modifiers.platform
            && !k.modifiers.control
            && !k.modifiers.alt
        {
            let d = k.key.as_str();
            let starts_count = d.len() == 1
                && d.chars().next().is_some_and(|c| c.is_ascii_digit() && c != '0');
            let extends_count = d == "0" && !self.count_buf.is_empty();
            if starts_count || extends_count {
                self.count_buf.push_str(d);
                return KeymapResult::Pending;
            }
        }
        // Commit count — valid for the remainder of this keypress only.
        let count = self.count_buf.parse::<usize>().unwrap_or(0);
        self.count_buf.clear();

        // ── `g` prefix sequences ────────────────────────────────────────
        if in_modal
            && k.key == "g"
            && !k.modifiers.platform
            && !k.modifiers.control
            && !k.modifiers.shift
        {
            if self.pending == PendingKey::G {
                self.pending = PendingKey::None;
                let saved = self.pending_count;
                self.pending_count = 0;
                if saved > 0 {
                    return KeymapResult::Command(EditorCommand::GotoLine(saved));
                }
                return KeymapResult::Command(EditorCommand::MoveStartOfDocument);
            } else {
                self.pending_count = count; // save for potential `gg`
                self.pending = PendingKey::G;
                return KeymapResult::Pending;
            }
        }
        if self.pending == PendingKey::G {
            self.pending = PendingKey::None;
            self.pending_count = 0;
            if !k.modifiers.platform && !k.modifiers.control {
                let cmd: Option<KeymapResult> = match k.key.as_str() {
                    "v" if state.mode == Mode::Normal => {
                        Some(KeymapResult::Command(EditorCommand::ReselectLastVisual))
                    }
                    "h" => Some(KeymapResult::Command(EditorCommand::MoveStartOfLine)),
                    "l" => Some(KeymapResult::Command(EditorCommand::MoveEndOfLine)),
                    "s" => Some(KeymapResult::Command(EditorCommand::MoveFirstNonWhitespace)),
                    "e" => Some(KeymapResult::Command(EditorCommand::MoveWordEnd)),
                    "E" => Some(KeymapResult::Command(EditorCommand::MoveWORDEnd)),
                    "c" => Some(KeymapResult::Command(EditorCommand::ToggleComment)),
                    // Visual-line movement (same as j/k for non-wrapped display).
                    "j" => Some(KeymapResult::Command(EditorCommand::MoveDown)),
                    "k" => Some(KeymapResult::Command(EditorCommand::MoveUp)),
                    // Middle of line.
                    "m" => Some(KeymapResult::Command(EditorCommand::GotoMiddleOfLine)),
                    // Last insert position.
                    "i" => Some(KeymapResult::Command(EditorCommand::GotoLastInsert)),
                    // Last modification position.
                    "." => Some(KeymapResult::Command(EditorCommand::GotoLastModified)),
                    // Follow link / open file under cursor.
                    "f" | "x" => Some(KeymapResult::FollowLink),
                    // Buffer navigation.
                    "n" => Some(KeymapResult::BufferNav { forward: true }),
                    "p" => Some(KeymapResult::BufferNav { forward: false }),
                    // LSP go-to-definition.
                    "d" => Some(KeymapResult::GotoDefinition),
                    _ => None,
                };
                if let Some(result) = cmd {
                    return result;
                }
            }
            // Unknown g-sequence — fall through.
        }

        // ── `m` text-object sequences (mi<obj> / ma<obj>) ──────────────
        if in_modal && k.key == "m" && !k.modifiers.platform && !k.modifiers.control {
            self.pending = PendingKey::M;
            return KeymapResult::Pending;
        }
        if self.pending == PendingKey::M {
            self.pending = PendingKey::None;
            match k.key.as_str() {
                "i" => {
                    self.pending = PendingKey::MatchInner;
                    return KeymapResult::Pending;
                }
                "a" => {
                    self.pending = PendingKey::MatchAround;
                    return KeymapResult::Pending;
                }
                _ => {} // fall through
            }
        }
        if matches!(
            self.pending,
            PendingKey::MatchInner | PendingKey::MatchAround
        ) {
            let inner = self.pending == PendingKey::MatchInner;
            self.pending = PendingKey::None;
            if let Some(kind) = parse_text_object_key(k) {
                return KeymapResult::Command(EditorCommand::SelectObject { inner, kind });
            }
            return KeymapResult::Passthrough;
        }

        // ── Operator + text-object (OpInner / OpAround) ─────────────────
        if matches!(
            self.pending,
            PendingKey::OpInner(_) | PendingKey::OpAround(_)
        ) {
            let (op, inner) = match self.pending {
                PendingKey::OpInner(o) => (o, true),
                PendingKey::OpAround(o) => (o, false),
                _ => unreachable!(),
            };
            self.pending = PendingKey::None;
            if let Some(kind) = parse_text_object_key_for_operator(k) {
                return KeymapResult::OperatorObject { op, inner, kind };
            }
            return KeymapResult::Passthrough;
        }

        // ── Operator + till/find char (OpTill / OpFind) ─────────────────
        if matches!(self.pending, PendingKey::OpTill(_) | PendingKey::OpFind(_)) {
            let (op, is_till) = match self.pending {
                PendingKey::OpTill(o) => (o, true),
                PendingKey::OpFind(o) => (o, false),
                _ => unreachable!(),
            };
            self.pending = PendingKey::None;
            if let Some(ch) = k.key_char.as_ref().and_then(|s| s.chars().next()) {
                if !k.modifiers.control && !k.modifiers.platform {
                    let motion = if is_till {
                        EditorCommand::TillChar(ch)
                    } else {
                        EditorCommand::FindChar(ch)
                    };
                    return KeymapResult::OperatorMotion { op, motion };
                }
            }
            return KeymapResult::Passthrough;
        }

        // ── Complete Operator + motion ──────────────────────────────────
        if let PendingKey::Operator(op) = self.pending {
            self.pending = PendingKey::None;
            if !k.modifiers.platform && !k.modifiers.control {
                // Doubled key → linewise.
                let same_key = matches!(
                    (op, k.key.as_str()),
                    (OperatorKind::Delete, "d")
                        | (OperatorKind::Change, "c")
                        | (OperatorKind::Yank, "y")
                );
                if same_key && !k.modifiers.shift {
                    return KeymapResult::OperatorLinewise(op);
                }
                // Sub-sequences.
                match k.key.as_str() {
                    "i" if !k.modifiers.shift => {
                        self.pending = PendingKey::OpInner(op);
                        return KeymapResult::Pending;
                    }
                    "a" if !k.modifiers.shift => {
                        self.pending = PendingKey::OpAround(op);
                        return KeymapResult::Pending;
                    }
                    "t" if !k.modifiers.shift => {
                        self.pending = PendingKey::OpTill(op);
                        return KeymapResult::Pending;
                    }
                    "f" if !k.modifiers.shift => {
                        self.pending = PendingKey::OpFind(op);
                        return KeymapResult::Pending;
                    }
                    _ => {}
                }
                // Single-key motion.
                let key_str = k.key_char.as_deref().unwrap_or(&k.key);
                if let Some(motion) = operator_motion_from_key(key_str) {
                    return KeymapResult::OperatorMotion { op, motion };
                }
            }
            return KeymapResult::Passthrough;
        }

        // ── `[` / `]` bracket-navigation prefix ────────────────────────
        if in_modal
            && self.pending == PendingKey::None
            && !k.modifiers.platform
            && !k.modifiers.control
            && !k.modifiers.shift
        {
            if k.key == "[" {
                self.pending = PendingKey::OpenBracket;
                return KeymapResult::Pending;
            }
            if k.key == "]" {
                self.pending = PendingKey::CloseBracket;
                return KeymapResult::Pending;
            }
        }
        if matches!(self.pending, PendingKey::OpenBracket | PendingKey::CloseBracket) {
            let forward = self.pending == PendingKey::CloseBracket;
            self.pending = PendingKey::None;
            if !k.modifiers.platform && !k.modifiers.control {
                match k.key.as_str() {
                    "d" => return KeymapResult::JumpDiagnostic { forward },
                    "p" => return KeymapResult::Command(if forward {
                        EditorCommand::MoveParagraphForward
                    } else {
                        EditorCommand::MoveParagraphBack
                    }),
                    _ => {}
                }
            }
            // Unknown bracket-sequence — fall through.
        }

        // ── `s` select-within-selection (Visual mode) ───────────────────
        if matches!(state.mode, Mode::Visual(_))
            && k.key == "s"
            && !k.modifiers.platform
            && !k.modifiers.control
            && !k.modifiers.shift
            && self.pending == PendingKey::None
        {
            return KeymapResult::SelectInSelection;
        }

        // ── Macro record / play (`q` / `@`) ────────────────────────────
        if state.mode == Mode::Normal
            && !k.modifiers.platform
            && !k.modifiers.control
            && !k.modifiers.shift
            && self.pending == PendingKey::None
        {
            if k.key == "q" {
                if self.recording {
                    // Stop recording.
                    return KeymapResult::StopMacro;
                } else {
                    // Start recording: wait for register key.
                    self.pending = PendingKey::MacroRecord;
                    return KeymapResult::Pending;
                }
            }
            if k.key == "@" || k.key_char.as_deref() == Some("@") {
                self.pending = PendingKey::MacroPlay;
                return KeymapResult::Pending;
            }
        }
        if self.pending == PendingKey::MacroRecord {
            self.pending = PendingKey::None;
            if !k.modifiers.platform && !k.modifiers.control {
                if let Some(ch) = k.key_char.as_ref().and_then(|s| s.chars().next()) {
                    return KeymapResult::StartMacro(ch);
                }
            }
            return KeymapResult::Passthrough;
        }
        if self.pending == PendingKey::MacroPlay {
            self.pending = PendingKey::None;
            if !k.modifiers.platform && !k.modifiers.control {
                if let Some(ch) = k.key_char.as_ref().and_then(|s| s.chars().next()) {
                    return KeymapResult::PlayMacro(ch);
                }
            }
            return KeymapResult::Passthrough;
        }

        // ── `z` scroll-alignment sequences ─────────────────────────────
        if in_modal
            && k.key == "z"
            && !k.modifiers.platform
            && !k.modifiers.control
            && !k.modifiers.shift
            && self.pending == PendingKey::None
        {
            self.pending = PendingKey::Z;
            return KeymapResult::Pending;
        }
        if self.pending == PendingKey::Z {
            self.pending = PendingKey::None;
            if !k.modifiers.platform && !k.modifiers.control {
                let align = match k.key.as_str() {
                    // Top
                    "t" | "enter" => Some(ViewportAlign::Top),
                    // Center
                    "z" | "." => Some(ViewportAlign::Center),
                    // Bottom
                    "b" | "-" => Some(ViewportAlign::Bottom),
                    // One-line scroll (cursor follows if it leaves view)
                    "j" => Some(ViewportAlign::LineDown),
                    "k" => Some(ViewportAlign::LineUp),
                    _ => None,
                };
                if let Some(a) = align {
                    return KeymapResult::ScrollViewport(a);
                }
            }
            // Unknown z-sequence — fall through.
        }

        // ── Multi-cursor (`C` / `Alt-C` / `,` / `Alt-,`) ───────────────
        if in_modal
            && self.pending == PendingKey::None
            && !k.modifiers.platform
            && !k.modifiers.control
        {
            // C (shift) — add cursor below (in Helix, C without shift too; we use
            // the capital letter since lowercase `c` is already change).
            if k.key == "C" && !k.modifiers.alt {
                return KeymapResult::Command(EditorCommand::AddCursorBelow);
            }
            // Alt-C — add cursor above.
            if k.key == "C" && k.modifiers.alt {
                return KeymapResult::Command(EditorCommand::AddCursorAbove);
            }
            // , — keep only primary cursor.
            if (k.key == "," || k.key_char.as_deref() == Some(",")) && !k.modifiers.alt && !k.modifiers.shift {
                return KeymapResult::Command(EditorCommand::KeepPrimaryCursor);
            }
            // Alt-, — remove primary cursor.
            if (k.key == "," || k.key_char.as_deref() == Some(",")) && k.modifiers.alt {
                return KeymapResult::Command(EditorCommand::RemovePrimaryCursor);
            }
        }

        // ── `/`, `?` search and `n`/`N`/`*`/`#` ────────────────────────
        if in_modal && !k.modifiers.platform && !k.modifiers.control {
            let is_slash = k.key == "/" || k.key_char.as_deref() == Some("/");
            let is_question = k.key == "?" || k.key_char.as_deref() == Some("?");
            if is_slash || is_question {
                return KeymapResult::OpenSearch {
                    backward: is_question,
                };
            }
            if k.key == "n" {
                return KeymapResult::SearchNext;
            }
            if k.key == "N" {
                return KeymapResult::SearchPrev;
            }
            let key_ch = k.key_char.as_deref().unwrap_or(&k.key);
            if key_ch == "*" {
                return KeymapResult::SearchWordForward;
            }
            if key_ch == "#" {
                return KeymapResult::SearchWordBackward;
            }
            // LSP hover popup.
            if k.key == "K" || k.key_char.as_deref() == Some("K") {
                return KeymapResult::ShowHover;
            }
        }

        // ── `d`/`c`/`y` operator pending in Normal mode ────────────────
        if state.mode == Mode::Normal
            && self.pending == PendingKey::None
            && !k.modifiers.platform
            && !k.modifiers.control
            && !k.modifiers.shift
        {
            let op = match k.key.as_str() {
                "d" => Some(OperatorKind::Delete),
                "c" => Some(OperatorKind::Change),
                "y" => Some(OperatorKind::Yank),
                _ => None,
            };
            if let Some(op) = op {
                self.pending = PendingKey::Operator(op);
                return KeymapResult::Pending;
            }
        }

        // ── `f`/`F`/`t`/`T` find-char sequences ────────────────────────
        if in_modal
            && self.pending == PendingKey::None
            && !k.modifiers.platform
            && !k.modifiers.control
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
                return KeymapResult::Pending;
            }
        }
        if matches!(
            self.pending,
            PendingKey::FindChar | PendingKey::FindCharBack | PendingKey::TillChar | PendingKey::TillCharBack
        ) {
            let pending_kind = self.pending;
            self.pending = PendingKey::None;
            if !k.modifiers.platform && !k.modifiers.control {
                if let Some(ch) = k.key_char.as_ref().and_then(|s| s.chars().next()) {
                    let cmd = match pending_kind {
                        PendingKey::FindChar => EditorCommand::FindChar(ch),
                        PendingKey::FindCharBack => EditorCommand::FindCharBack(ch),
                        PendingKey::TillChar => EditorCommand::TillChar(ch),
                        PendingKey::TillCharBack => EditorCommand::TillCharBack(ch),
                        _ => unreachable!(),
                    };
                    return KeymapResult::Command(cmd);
                }
            }
            return KeymapResult::Passthrough;
        }

        // ── Count-qualified motions ──────────────────────────────────────
        // `<N>G` → jump to line N; `<N>j`/`<N>k` → move N lines.
        if in_modal && count > 0 && !k.modifiers.platform && !k.modifiers.control {
            match k.key.as_str() {
                "G" => {
                    return KeymapResult::Command(EditorCommand::GotoLine(count));
                }
                "j" if count > 1 => {
                    let cmds = vec![EditorCommand::MoveDown; count.min(500)];
                    return KeymapResult::Commands(cmds);
                }
                "k" if count > 1 => {
                    let cmds = vec![EditorCommand::MoveUp; count.min(500)];
                    return KeymapResult::Commands(cmds);
                }
                _ => {} // fall through
            }
        }

        // ── Map keystroke to EditorCommand ───────────────────────────────
        let cmd = keystroke_to_command(event, state);
        if cmd == EditorCommand::Noop {
            // Visual mode surround.
            if matches!(state.mode, Mode::Visual(_))
                && !k.modifiers.platform
                && !k.modifiers.control
                && !k.modifiers.alt
            {
                if let Some(typed) = k.key_char.as_deref() {
                    if let Some(close) = visual_surround_close(typed) {
                        return KeymapResult::Surround {
                            open: typed.to_string(),
                            close,
                        };
                    }
                }
            }
            return KeymapResult::Passthrough;
        }

        if cmd == EditorCommand::OpenPalette {
            return KeymapResult::OpenPalette;
        }

        KeymapResult::Command(cmd)
    }

    fn mode_label(&self, state: &EditorState) -> &str {
        match state.mode {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual(_) => "VISUAL",
        }
    }

    fn cursor_style(&self, state: &EditorState) -> CursorStyle {
        match state.mode {
            Mode::Insert => CursorStyle::Line,
            _ => CursorStyle::Block,
        }
    }

    fn set_macro_recording(&mut self, active: bool) {
        self.recording = active;
    }
}

// ── Key mapping functions ────────────────────────────────────────────────────

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

    if k.modifiers.control && !k.modifiers.platform {
        return match k.key.as_str() {
            "v" => EnterVisualBlock,
            "d" => ScrollHalfDown,
            "u" => ScrollHalfUp,
            "f" => ScrollPageDown,
            "b" => ScrollPageUp,
            "a" => IncrementNumber,
            "x" => DecrementNumber,
            _ => Noop,
        };
    }
    if k.modifiers.platform {
        return Noop;
    }
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
        "G" => MoveEndOfDocument,
        ";" => CollapseSelection,
        "_" => TrimSelection,
        "i" => EnterInsert,
        "a" => AppendAfterCursor,
        "I" => InsertLineStart,
        "A" => InsertLineEnd,
        "o" => OpenLineBelow,
        "O" => OpenLineAbove,
        "d" => DeleteLine,
        "D" => DeleteToLineEnd,
        "c" => ChangeLine,
        "C" => ChangeToLineEnd,
        "y" => YankLine,
        "p" => PasteAfter,
        "P" => PasteBefore,
        "x" => SelectCurrentLine,
        "X" => ExtendLineBelow,
        "R" => ReplaceWithYanked,
        "=" => AutoIndent,
        "v" => EnterVisualChar,
        "V" => EnterVisualLine,
        ">" => IndentLines,
        "<" => DedentLines,
        "{" => MoveParagraphBack,
        "}" => MoveParagraphForward,
        "%" => SelectWholeFile,
        "~" => SwitchCase,
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
    if k.modifiers.control {
        return match k.key.as_str() {
            "v" => EnterVisualBlock,
            "d" => ScrollHalfDown,
            "u" => ScrollHalfUp,
            "f" => ScrollPageDown,
            "b" => ScrollPageUp,
            "a" => IncrementNumber,
            "x" => DecrementNumber,
            _ => Noop,
        };
    }
    if k.modifiers.alt {
        return match k.key.as_str() {
            ";" => FlipSelection,
            _ => Noop,
        };
    }
    match k.key.as_str() {
        "escape" => EnterNormal,
        "d" | "x" => DeleteSelection,
        "y" => YankSelection,
        "c" => ChangeSelection,
        "R" => ReplaceWithYanked,
        "X" => ExtendLineBelow,
        "=" => AutoIndent,
        ">" => IndentLines,
        "<" => DedentLines,
        ";" => CollapseSelection,
        "_" => TrimSelection,
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
        "v" => EnterVisualChar,
        "V" => EnterVisualLine,
        "{" => MoveParagraphBack,
        "}" => MoveParagraphForward,
        "%" => SelectWholeFile,
        "~" => SwitchCase,
        _ => Noop,
    }
}

fn key_insert(event: &KeyDownEvent) -> EditorCommand {
    use EditorCommand::*;
    let k = &event.keystroke;
    if k.modifiers.platform {
        return Noop;
    }
    if k.modifiers.control {
        return match k.key.as_str() {
            "w" => DeleteWordBefore,
            "u" => DeleteToLineStart,
            "k" => DeleteRestOfLine,
            "j" => InsertNewline,
            _ => Noop,
        };
    }
    match k.key.as_str() {
        "escape" => EnterNormal,
        "backspace" => DeleteCharBefore,
        "delete" => DeleteCharAt,
        "enter" => InsertNewline,
        // Explicit space binding so GPUI never swallows it before key_char is checked.
        "space" => Insert(" ".to_string()),
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

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Parse a text-object key for the `mi`/`ma` sequences.
fn parse_text_object_key(k: &gpui::Keystroke) -> Option<TextObjectKind> {
    if k.modifiers.platform || k.modifiers.control {
        return None;
    }
    match k.key_char.as_deref().unwrap_or(&k.key) {
        "w" => Some(TextObjectKind::Word),
        "W" => Some(TextObjectKind::WORD),
        "p" => Some(TextObjectKind::Paragraph),
        "(" | ")" => Some(TextObjectKind::Paren),
        "{" | "}" => Some(TextObjectKind::Brace),
        "[" | "]" => Some(TextObjectKind::Bracket),
        "<" | ">" => Some(TextObjectKind::Angle),
        "\"" => Some(TextObjectKind::DoubleQuote),
        "'" => Some(TextObjectKind::SingleQuote),
        "`" => Some(TextObjectKind::Backtick),
        "$" => Some(TextObjectKind::InlineMath),
        "t" => Some(TextObjectKind::TypstContent),
        _ => None,
    }
}

/// Parse a text-object key for operator sequences (includes `b`/`B` aliases).
fn parse_text_object_key_for_operator(k: &gpui::Keystroke) -> Option<TextObjectKind> {
    if k.modifiers.platform || k.modifiers.control {
        return None;
    }
    match k.key_char.as_deref().unwrap_or(&k.key) {
        "w" => Some(TextObjectKind::Word),
        "W" => Some(TextObjectKind::WORD),
        "p" => Some(TextObjectKind::Paragraph),
        "(" | ")" | "b" => Some(TextObjectKind::Paren),
        "{" | "}" | "B" => Some(TextObjectKind::Brace),
        "[" | "]" => Some(TextObjectKind::Bracket),
        "<" | ">" => Some(TextObjectKind::Angle),
        "\"" => Some(TextObjectKind::DoubleQuote),
        "'" => Some(TextObjectKind::SingleQuote),
        "`" => Some(TextObjectKind::Backtick),
        "$" => Some(TextObjectKind::InlineMath),
        "t" => Some(TextObjectKind::TypstContent),
        _ => None,
    }
}

/// Map a key string to a motion EditorCommand for operator+motion sequences.
fn operator_motion_from_key(key: &str) -> Option<EditorCommand> {
    Some(match key {
        "w" => EditorCommand::MoveWordForward,
        "b" => EditorCommand::MoveWordBackward,
        "e" => EditorCommand::MoveWordEnd,
        "W" => EditorCommand::MoveWORDForward,
        "B" => EditorCommand::MoveWORDBackward,
        "E" => EditorCommand::MoveWORDEnd,
        "h" => EditorCommand::MoveLeft,
        "l" => EditorCommand::MoveRight,
        "k" => EditorCommand::MoveUp,
        "j" => EditorCommand::MoveDown,
        "0" => EditorCommand::MoveStartOfLine,
        "$" => EditorCommand::MoveEndOfLine,
        "^" => EditorCommand::MoveFirstNonWhitespace,
        "G" => EditorCommand::MoveEndOfDocument,
        "{" => EditorCommand::MoveParagraphBack,
        "}" => EditorCommand::MoveParagraphForward,
        _ => return None,
    })
}

/// In Visual mode: return the closing delimiter for a surround operation.
fn visual_surround_close(open: &str) -> Option<&'static str> {
    match open {
        "(" => Some(")"),
        "[" => Some("]"),
        "\"" => Some("\""),
        _ => None,
    }
}
