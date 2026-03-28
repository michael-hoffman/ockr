//! Command Palette — fuzzy-searchable list of all registered commands.
//!
//! Triggered by `Cmd-P`. Renders as a floating modal overlay with a search
//! input field and filtered command list. Navigation with arrows or Ctrl-J/K;
//! `Enter` executes, `Escape` dismisses.

use gpui::{
    Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent,
    Render, Window, div, prelude::*, px,
};

use crate::command::CommandRegistry;
use crate::ui::theme::ThemePalette;

// ── Events ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum PaletteEvent {
    /// User dismissed the palette without executing a command.
    Close,
    /// User selected a command; carries the command's `id`.
    Execute(String),
}

impl EventEmitter<PaletteEvent> for CommandPalette {}

// ── View ──────────────────────────────────────────────────────────────────────

pub struct CommandPalette {
    pub focus_handle: FocusHandle,
    query: String,
    /// Indices into `CommandRegistry::entries()` matching the current query.
    matches: Vec<usize>,
    selected: usize,
}

impl CommandPalette {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut palette = Self {
            focus_handle: cx.focus_handle(),
            query: String::new(),
            matches: Vec::new(),
            selected: 0,
        };
        palette.refresh_matches(cx);
        palette
    }

    fn refresh_matches(&mut self, cx: &mut Context<Self>) {
        let registry = cx.global::<CommandRegistry>();
        self.matches = registry.search(&self.query);
        if self.selected >= self.matches.len() {
            self.selected = self.matches.len().saturating_sub(1);
        }
    }

    fn handle_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let k = &event.keystroke;

        // Close on Escape.
        if k.key == "escape" {
            cx.emit(PaletteEvent::Close);
            return;
        }

        // Execute on Enter.
        if k.key == "enter" {
            if let Some(&idx) = self.matches.get(self.selected) {
                let id = cx.global::<CommandRegistry>().entries()[idx].id.clone();
                cx.emit(PaletteEvent::Execute(id));
            } else {
                cx.emit(PaletteEvent::Close);
            }
            return;
        }

        // Navigate up.
        if k.key == "up" || (k.modifiers.control && k.key == "k") {
            self.selected = self.selected.saturating_sub(1);
            cx.notify();
            return;
        }

        // Navigate down.
        if k.key == "down" || (k.modifiers.control && k.key == "j") {
            if !self.matches.is_empty() && self.selected + 1 < self.matches.len() {
                self.selected += 1;
            }
            cx.notify();
            return;
        }

        // Backspace: erase last query character.
        if k.key == "backspace" {
            self.query.pop();
            self.refresh_matches(cx);
            cx.notify();
            return;
        }

        // Printable character: append to query.
        if let Some(ch) = &k.key_char {
            if !k.modifiers.control && !k.modifiers.platform {
                self.query.push_str(ch);
                self.refresh_matches(cx);
                cx.notify();
            }
        }
    }

    fn handle_row_click(
        &mut self,
        idx: usize,
        _event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = cx.global::<CommandRegistry>().entries()[idx].id.clone();
        cx.emit(PaletteEvent::Execute(id));
    }
}

impl Focusable for CommandPalette {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

impl Render for CommandPalette {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.global::<ThemePalette>().clone();
        let registry = cx.global::<CommandRegistry>();
        let entries = registry.entries();

        // Build result rows.
        let mut rows = Vec::with_capacity(self.matches.len());
        for (row_idx, &entry_idx) in self.matches.iter().enumerate() {
            let entry = &entries[entry_idx];
            let name = entry.name.clone();
            let hint = entry.keybinding_hint.as_deref().unwrap_or("").to_string();
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
            let text_color = if is_selected {
                gpui::rgb(t.text)
            } else {
                gpui::rgb(t.text_muted)
            };
            let hint_color = gpui::rgb(t.text_faint);

            let row = div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .px(px(16.0))
                .py(px(8.0))
                .bg(bg)
                .border_l(px(2.0))
                .border_color(accent)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event, window, cx| {
                        this.handle_row_click(entry_idx, event, window, cx);
                    }),
                )
                .child(
                    div()
                        .text_sm()
                        .font_family("Menlo")
                        .text_color(text_color)
                        .child(name),
                )
                .child(
                    div()
                        .text_xs()
                        .font_family("Menlo")
                        .text_color(hint_color)
                        .child(hint),
                );
            rows.push(row);
        }

        // Query display — shows typed text with a blinking-style cursor stub.
        let query_display = if self.query.is_empty() {
            div()
                .text_sm()
                .font_family("Menlo")
                .text_color(gpui::rgb(t.text_faint))
                .child("Search commands…")
        } else {
            div()
                .text_sm()
                .font_family("Menlo")
                .text_color(gpui::rgb(t.text))
                .child(self.query.clone())
        };

        // Backdrop: full-screen transparent hit-area — clicks outside modal close it.
        div()
            .absolute()
            .inset_0()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::handle_key))
            // Clicking the backdrop dismisses the palette.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.emit(PaletteEvent::Close);
                    cx.notify();
                    // Suppress unused warning
                    let _ = this;
                }),
            )
            .flex()
            .flex_col()
            .items_center()  // center modal horizontally
            .pt(px(48.0))    // offset from top
            .child(
                // Modal container — stop click propagation to backdrop.
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
                    // Stop click propagation so the backdrop's dismiss handler
                    // doesn't fire when clicking inside the modal.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    // Search input row
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
                                // "⌘P" badge
                                div()
                                    .text_xs()
                                    .font_family("Menlo")
                                    .text_color(gpui::rgb(t.ochre))
                                    .child("⌘"),
                            )
                            .child(query_display),
                    )
                    // Results list (capped at 12 visible rows via max_h + hidden overflow)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .max_h(px(320.0))
                            .overflow_hidden()
                            .children(rows),
                    ),
            )
    }
}
