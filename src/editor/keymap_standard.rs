//! Standard (VSCode-like) keyboard mode — Story 24.
//!
//! Non-modal editing: no Normal/Visual modes.  The editor is always in
//! Insert mode.  Arrow keys move, Shift+Arrow extends selection.
//! Cmd+Left/Right = home/end, Opt+Left/Right = word movement.

use gpui::KeyDownEvent;

use super::command::EditorCommand;
use super::keymap::{CursorStyle, KeymapHandler, KeymapResult};
use super::state::{EditorState, Mode};

pub struct StandardKeymap;

impl StandardKeymap {
    pub fn new() -> Self {
        Self
    }
}

impl KeymapHandler for StandardKeymap {
    fn handle_key(&mut self, event: &KeyDownEvent, state: &EditorState) -> KeymapResult {
        let k = &event.keystroke;

        // Platform (Cmd) shortcuts are handled by EditorPane before we get here,
        // except Cmd+A (select all) and Cmd+/ (toggle comment) which are Standard-mode-specific.
        if k.modifiers.platform {
            return match k.key.as_str() {
                "a" => KeymapResult::Command(EditorCommand::SelectWholeFile),
                "/" => KeymapResult::Command(EditorCommand::ToggleComment),
                _ => KeymapResult::Passthrough,
            };
        }

        // Ctrl combos.
        if k.modifiers.control {
            return KeymapResult::Passthrough;
        }

        // Alt+Shift+Arrow — extend selection by one word.
        // Must be checked before plain Shift+Arrow so the more-specific binding wins.
        if k.modifiers.alt && k.modifiers.shift {
            let motion = match k.key.as_str() {
                "left"  => Some(EditorCommand::MoveWordBackward),
                "right" => Some(EditorCommand::MoveWordForward),
                _ => None,
            };
            if let Some(motion) = motion {
                if !matches!(state.mode, Mode::Visual(_)) {
                    // Enter visual char mode and extend by one word atomically.
                    return KeymapResult::Commands(vec![
                        EditorCommand::EnterVisualChar,
                        motion,
                    ]);
                }
                return KeymapResult::Command(motion);
            }
        }

        // Shift+Arrow — extend (or start) a character selection.
        if k.modifiers.shift {
            let motion = match k.key.as_str() {
                "left"  => Some(EditorCommand::MoveLeft),
                "right" => Some(EditorCommand::MoveRight),
                "up"    => Some(EditorCommand::MoveUp),
                "down"  => Some(EditorCommand::MoveDown),
                "home"  => Some(EditorCommand::MoveStartOfLine),
                "end"   => Some(EditorCommand::MoveEndOfLine),
                _ => None,
            };
            if let Some(motion) = motion {
                if !matches!(state.mode, Mode::Visual(_)) {
                    // First Shift+Arrow: enter visual char mode AND perform the
                    // motion in one step so the user sees a 1-character selection
                    // immediately (matches VS Code / standard-editor behaviour).
                    return KeymapResult::Commands(vec![
                        EditorCommand::EnterVisualChar,
                        motion,
                    ]);
                }
                return KeymapResult::Command(motion);
            }
        }

        // Alt (Option) + arrow = word movement (no selection).
        if k.modifiers.alt {
            return match k.key.as_str() {
                "left"      => KeymapResult::Command(EditorCommand::MoveWordBackward),
                "right"     => KeymapResult::Command(EditorCommand::MoveWordForward),
                "backspace" => KeymapResult::Command(EditorCommand::DeleteWordBefore),
                _ => KeymapResult::Passthrough,
            };
        }

        match k.key.as_str() {
            "backspace" => KeymapResult::Command(EditorCommand::DeleteCharBefore),
            "delete"    => KeymapResult::Command(EditorCommand::DeleteCharAt),
            "enter"     => KeymapResult::Command(EditorCommand::InsertNewline),
            // Explicit space binding so GPUI never swallows it before key_char is checked.
            "space"     => KeymapResult::Command(EditorCommand::Insert(" ".to_string())),
            "left" => {
                // Collapse to the LEFT end of the selection (VS Code behaviour).
                if matches!(state.mode, Mode::Visual(_)) {
                    return KeymapResult::Command(EditorCommand::CollapseSelectionLeft);
                }
                KeymapResult::Command(EditorCommand::MoveLeft)
            }
            "right" => {
                // Collapse to the RIGHT end of the selection (VS Code behaviour).
                if matches!(state.mode, Mode::Visual(_)) {
                    return KeymapResult::Command(EditorCommand::CollapseSelectionRight);
                }
                KeymapResult::Command(EditorCommand::MoveRight)
            }
            "up" => {
                if matches!(state.mode, Mode::Visual(_)) {
                    return KeymapResult::Command(EditorCommand::CollapseSelection);
                }
                KeymapResult::Command(EditorCommand::MoveUp)
            }
            "down" => {
                if matches!(state.mode, Mode::Visual(_)) {
                    return KeymapResult::Command(EditorCommand::CollapseSelection);
                }
                KeymapResult::Command(EditorCommand::MoveDown)
            }
            "home" => KeymapResult::Command(EditorCommand::MoveStartOfLine),
            "end"  => KeymapResult::Command(EditorCommand::MoveEndOfLine),
            "escape" => {
                // In Standard mode, Escape collapses any active selection.
                if matches!(state.mode, Mode::Visual(_)) {
                    return KeymapResult::Command(EditorCommand::CollapseSelection);
                }
                KeymapResult::Passthrough
            }
            "tab" => KeymapResult::Command(EditorCommand::Insert("  ".to_string())),
            _ => {
                if let Some(c) = &k.key_char {
                    // Typing while a Visual selection is active replaces it.
                    if matches!(state.mode, Mode::Visual(_)) {
                        // ChangeSelection deletes the selection and enters Insert
                        // mode; the typed character arrives as the next event.
                        return KeymapResult::Command(EditorCommand::ChangeSelection);
                    }
                    return KeymapResult::Command(EditorCommand::Insert(c.clone()));
                }
                KeymapResult::Passthrough
            }
        }
    }

    fn mode_label(&self, _state: &EditorState) -> &str {
        "STANDARD"
    }

    fn cursor_style(&self, _state: &EditorState) -> CursorStyle {
        CursorStyle::Line
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::keymap::KeymapHandler;

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

    fn make_key_shift(key: &str) -> KeyDownEvent {
        use gpui::{Keystroke, Modifiers};
        KeyDownEvent {
            keystroke: Keystroke {
                modifiers: Modifiers {
                    shift: true,
                    ..Default::default()
                },
                key: key.to_string(),
                key_char: None,
            },
            is_held: false,
        }
    }

    #[test]
    fn printable_inserts() {
        let mut km = StandardKeymap::new();
        let mut state = EditorState::new();
        state.mode = Mode::Insert;
        let event = make_key("a");
        assert!(matches!(
            km.handle_key(&event, &state),
            KeymapResult::Command(EditorCommand::Insert(ref s)) if s == "a"
        ));
    }

    #[test]
    fn arrow_keys_move() {
        let mut km = StandardKeymap::new();
        let mut state = EditorState::new();
        state.mode = Mode::Insert;
        assert!(matches!(
            km.handle_key(&make_key("left"), &state),
            KeymapResult::Command(EditorCommand::MoveLeft)
        ));
        assert!(matches!(
            km.handle_key(&make_key("right"), &state),
            KeymapResult::Command(EditorCommand::MoveRight)
        ));
    }

    #[test]
    fn shift_arrow_enters_visual_and_moves() {
        let mut km = StandardKeymap::new();
        let mut state = EditorState::new();
        state.mode = Mode::Insert;
        // First Shift+Right should enter visual char mode AND move right
        // in a single step — so the user sees a 1-character selection
        // immediately rather than needing two keypresses.
        let result = km.handle_key(&make_key_shift("right"), &state);
        if let KeymapResult::Commands(cmds) = result {
            assert_eq!(cmds.len(), 2);
            assert_eq!(cmds[0], EditorCommand::EnterVisualChar);
            assert_eq!(cmds[1], EditorCommand::MoveRight);
        } else {
            panic!("expected KeymapResult::Commands, got something else");
        }
    }

    #[test]
    fn shift_arrow_extends_when_already_visual() {
        let mut km = StandardKeymap::new();
        let mut state = EditorState::new();
        state.mode = Mode::Visual(crate::editor::state::VisualKind::Char);
        // Subsequent Shift+Arrow in Visual mode should just emit the motion.
        assert!(matches!(
            km.handle_key(&make_key_shift("right"), &state),
            KeymapResult::Command(EditorCommand::MoveRight)
        ));
    }

    #[test]
    fn mode_label_is_standard() {
        let km = StandardKeymap::new();
        let state = EditorState::new();
        assert_eq!(km.mode_label(&state), "STANDARD");
    }
}
