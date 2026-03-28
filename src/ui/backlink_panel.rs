//! Backlink panel — shows all notes that link to the currently open note.
//!
//! Opened with `Cmd-Shift-K`.  Rendered as a right-aligned overlay (same
//! position as the quick-switch panel) so the user can scan it without
//! leaving the editor.
//!
//! Navigation: arrow keys / Ctrl-J / Ctrl-K; Enter opens; Escape dismisses.

use std::path::PathBuf;

use gpui::{
    Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent,
    Render, Window, div, prelude::*, px,
};

use crate::ui::theme::ThemePalette;
use crate::vault::VaultFile;

// ── Events ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum BacklinkPanelEvent {
    Close,
    Open(PathBuf),
}

impl EventEmitter<BacklinkPanelEvent> for BacklinkPanel {}

// ── View ──────────────────────────────────────────────────────────────────────

pub struct BacklinkPanel {
    pub focus_handle: FocusHandle,
    /// The note whose incoming links are displayed.
    current_title: String,
    /// Incoming links, sorted by title.
    links: Vec<VaultFile>,
    selected: usize,
}

impl BacklinkPanel {
    pub fn new(
        current_title: String,
        mut links: Vec<VaultFile>,
        cx: &mut Context<Self>,
    ) -> Self {
        links.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
        Self {
            focus_handle: cx.focus_handle(),
            current_title,
            links,
            selected: 0,
        }
    }

    fn handle_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let k = &event.keystroke;

        if k.key == "escape" {
            cx.emit(BacklinkPanelEvent::Close);
            return;
        }
        if k.key == "enter" {
            if let Some(f) = self.links.get(self.selected) {
                cx.emit(BacklinkPanelEvent::Open(f.abs_path.clone()));
            } else {
                cx.emit(BacklinkPanelEvent::Close);
            }
            return;
        }
        if k.key == "up" || (k.modifiers.control && k.key == "k") {
            self.selected = self.selected.saturating_sub(1);
            cx.notify();
        }
        if k.key == "down" || (k.modifiers.control && k.key == "j") {
            if !self.links.is_empty() && self.selected + 1 < self.links.len() {
                self.selected += 1;
            }
            cx.notify();
        }
    }

    fn handle_row_click(
        &mut self,
        idx: usize,
        _event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.emit(BacklinkPanelEvent::Open(self.links[idx].abs_path.clone()));
    }
}

impl Focusable for BacklinkPanel {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

impl Render for BacklinkPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.global::<ThemePalette>().clone();
        let mut rows = Vec::new();

        if self.links.is_empty() {
            rows.push(
                div()
                    .px(px(16.0))
                    .py(px(12.0))
                    .text_sm()
                    .font_family("Menlo")
                    .text_color(gpui::rgb(t.text_faint))
                    .child("No backlinks found.")
                    .into_any_element(),
            );
        } else {
            for (row_idx, file) in self.links.iter().enumerate().take(20) {
                let is_selected = row_idx == self.selected;
                let bg = if is_selected {
                    gpui::rgb(t.bg_hover)
                } else {
                    gpui::rgb(t.bg_surface)
                };
                let accent = if is_selected {
                    gpui::rgb(t.ochre)
                } else {
                    gpui::rgb(t.border_subtle)
                };
                let title = file.title.clone();
                let subtitle = file.rel_path.to_string_lossy().into_owned();
                let title_color = if is_selected { t.text } else { t.text_muted };

                rows.push(
                    div()
                        .flex()
                        .flex_col()
                        .px(px(16.0))
                        .py(px(6.0))
                        .bg(bg)
                        .border_l(px(2.0))
                        .border_color(accent)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event, window, cx| {
                                this.handle_row_click(row_idx, event, window, cx);
                            }),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_family("Menlo")
                                .text_color(gpui::rgb(title_color))
                                .child(title),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_family("Menlo")
                                .text_color(gpui::rgb(t.text_faint))
                                .child(subtitle),
                        )
                        .into_any_element(),
                );
            }
        }

        let header_label = format!("← links to \"{}\"", self.current_title);

        div()
            .absolute()
            .inset_0()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::handle_key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(BacklinkPanelEvent::Close)),
            )
            .flex()
            .flex_col()
            .items_center()
            .pt(px(48.0))
            .child(
                div()
                    .w(px(400.0))
                    .bg(gpui::rgb(t.bg_surface))
                    .border_1()
                    .border_color(gpui::rgb(t.border_subtle))
                    .rounded(px(8.0))
                    .overflow_hidden()
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .px(px(16.0))
                            .py(px(12.0))
                            .border_b_1()
                            .border_color(gpui::rgb(t.border_subtle))
                            .gap(px(10.0))
                            .child(
                                div()
                                    .text_xs()
                                    .font_family("Menlo")
                                    .text_color(gpui::rgb(t.ochre))
                                    .child("⌘⇧K"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_family("Menlo")
                                    .text_color(gpui::rgb(t.text_muted))
                                    .child(header_label),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .max_h(px(380.0))
                            .overflow_hidden()
                            .children(rows),
                    ),
            )
    }
}
