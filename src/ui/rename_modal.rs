//! Rename modal — a single-line text input for renaming a vault file.
//!
//! Opened from the sidebar context menu (`Rename`).  Pre-filled with the
//! file's current stem (extension preserved).  Enter submits, Escape cancels.
//!
//! The modal only edits the file *stem*; the parent directory and `.typ`
//! extension are kept.  `MainWindow` performs the actual `fs::rename` and
//! refreshes the vault.

use std::path::PathBuf;

use gpui::{
    Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton, Render, Window, div,
    prelude::*, px,
};

use crate::ui::theme::ThemePalette;

// ── Events ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum RenameModalEvent {
    Close,
    /// Commit the rename: (original absolute path, new file stem).
    Submit { original: PathBuf, new_stem: String },
}

impl EventEmitter<RenameModalEvent> for RenameModal {}

// ── View ──────────────────────────────────────────────────────────────────────

pub struct RenameModal {
    pub focus_handle: FocusHandle,
    original: PathBuf,
    /// The editable file stem (no extension).
    value: String,
    /// Extension preserved across the rename (e.g. "typ"), shown as a suffix.
    ext: String,
}

impl RenameModal {
    pub fn new(path: PathBuf, cx: &mut Context<Self>) -> Self {
        let value = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let ext = path
            .extension()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self {
            focus_handle: cx.focus_handle(),
            original: path,
            value,
            ext,
        }
    }

    fn handle_key(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;

        if k.key == "escape" {
            cx.emit(RenameModalEvent::Close);
            return;
        }

        if k.key == "enter" {
            let trimmed = self.value.trim();
            if trimmed.is_empty() {
                cx.emit(RenameModalEvent::Close);
            } else {
                cx.emit(RenameModalEvent::Submit {
                    original: self.original.clone(),
                    new_stem: trimmed.to_string(),
                });
            }
            return;
        }

        if k.key == "backspace" {
            self.value.pop();
            cx.notify();
            return;
        }

        if let Some(ch) = &k.key_char {
            if !k.modifiers.control && !k.modifiers.platform {
                // Disallow path separators in a filename.
                if !ch.contains('/') && !ch.contains('\\') {
                    self.value.push_str(ch);
                    cx.notify();
                }
            }
        }
    }
}

impl Focusable for RenameModal {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

impl Render for RenameModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.global::<ThemePalette>().clone();
        let suffix = if self.ext.is_empty() {
            String::new()
        } else {
            format!(".{}", self.ext)
        };

        div()
            .absolute()
            .inset_0()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::handle_key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(RenameModalEvent::Close)),
            )
            .flex()
            .flex_col()
            .items_center()
            .pt(px(120.0))
            .child(
                div()
                    .w(px(420.0))
                    .bg(gpui::rgb(t.bg_surface))
                    .border_1()
                    .border_color(gpui::rgb(t.ochre_border))
                    .rounded(px(8.0))
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(10.0))
                            .border_b_1()
                            .border_color(gpui::rgb(t.border_subtle))
                            .text_xs()
                            .font_family("Menlo")
                            .text_color(gpui::rgb(t.text_faint))
                            .child("Rename file"),
                    )
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(12.0))
                            .flex()
                            .flex_row()
                            .items_center()
                            .text_sm()
                            .font_family("Menlo")
                            .child(
                                div()
                                    .text_color(gpui::rgb(t.text))
                                    .child(format!("{}|", self.value)),
                            )
                            .child(
                                div()
                                    .text_color(gpui::rgb(t.text_faint))
                                    .child(suffix),
                            ),
                    )
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(6.0))
                            .border_t_1()
                            .border_color(gpui::rgb(t.border_subtle))
                            .text_xs()
                            .font_family("Menlo")
                            .text_color(gpui::rgb(t.text_faint))
                            .child("Enter to rename · Esc to cancel"),
                    ),
            )
    }
}
