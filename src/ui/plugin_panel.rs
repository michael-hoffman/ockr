//! Plugin panel overlay — displays a plugin's registered UI panel.
//!
//! Follows the BacklinkPanel pattern: floating overlay, keyboard navigation
//! on Button items, Escape to close.

use gpui::{
    Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent,
    Render, Window, div, prelude::*, px,
};

use crate::plugin::panel::{LayoutItem, PanelPosition, RegisteredPanel};
use crate::ui::theme::ThemePalette;

// ── Events ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum PluginPanelEvent {
    Close,
    ExecuteCommand(String),
}

impl EventEmitter<PluginPanelEvent> for PluginPanel {}

// ── View ──────────────────────────────────────────────────────────────────────

pub struct PluginPanel {
    pub focus_handle: FocusHandle,
    pub panel: RegisteredPanel,
    /// Index into `button_indices` that is currently highlighted.
    selected: usize,
    /// Indices into `panel.layout.items` that are `LayoutItem::Button`.
    button_indices: Vec<usize>,
}

impl PluginPanel {
    pub fn new(panel: RegisteredPanel, cx: &mut Context<Self>) -> Self {
        let button_indices = panel
            .layout
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| matches!(item, LayoutItem::Button { .. }).then_some(i))
            .collect();
        Self {
            focus_handle: cx.focus_handle(),
            panel,
            selected: 0,
            button_indices,
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
            cx.emit(PluginPanelEvent::Close);
            return;
        }

        if k.key == "enter" {
            if let Some(&item_idx) = self.button_indices.get(self.selected) {
                if let Some(LayoutItem::Button { command_id, .. }) =
                    self.panel.layout.items.get(item_idx)
                {
                    cx.emit(PluginPanelEvent::ExecuteCommand(command_id.clone()));
                }
            }
            return;
        }

        if k.key == "up" || (k.modifiers.control && k.key == "k") {
            self.selected = self.selected.saturating_sub(1);
            cx.notify();
            return;
        }

        if k.key == "down" || (k.modifiers.control && k.key == "j") {
            if !self.button_indices.is_empty() && self.selected + 1 < self.button_indices.len() {
                self.selected += 1;
            }
            cx.notify();
        }
    }

    fn handle_button_click(
        &mut self,
        command_id: String,
        _event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.emit(PluginPanelEvent::ExecuteCommand(command_id));
    }
}

impl Focusable for PluginPanel {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

impl Render for PluginPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.global::<ThemePalette>().clone();
        let mut rows: Vec<gpui::AnyElement> = Vec::new();

        let mut button_counter = 0usize;
        for item in &self.panel.layout.items {
            match item {
                LayoutItem::Text { content } => {
                    rows.push(
                        div()
                            .px(px(16.0))
                            .py(px(6.0))
                            .text_sm()
                            .font_family("Menlo")
                            .text_color(gpui::rgb(t.text_muted))
                            .child(content.clone())
                            .into_any_element(),
                    );
                }
                LayoutItem::Button { label, command_id } => {
                    let btn_idx = button_counter;
                    button_counter += 1;
                    let is_selected = btn_idx == self.selected;
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
                    let text_color = if is_selected { t.text } else { t.text_muted };
                    let cmd = command_id.clone();
                    rows.push(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .px(px(16.0))
                            .py(px(8.0))
                            .bg(bg)
                            .border_l(px(2.0))
                            .border_color(accent)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event, window, cx| {
                                    this.handle_button_click(cmd.clone(), event, window, cx);
                                }),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_family("Menlo")
                                    .text_color(gpui::rgb(text_color))
                                    .child(label.clone()),
                            )
                            .into_any_element(),
                    );
                }
                LayoutItem::List { items } => {
                    for item_str in items {
                        rows.push(
                            div()
                                .px(px(20.0))
                                .py(px(4.0))
                                .text_sm()
                                .font_family("Menlo")
                                .text_color(gpui::rgb(t.text_muted))
                                .child(format!("• {item_str}"))
                                .into_any_element(),
                        );
                    }
                }
                LayoutItem::Divider => {
                    rows.push(
                        div()
                            .h(px(1.0))
                            .mx(px(16.0))
                            .my(px(4.0))
                            .bg(gpui::rgb(t.border_subtle))
                            .into_any_element(),
                    );
                }
            }
        }

        if rows.is_empty() {
            rows.push(
                div()
                    .px(px(16.0))
                    .py(px(12.0))
                    .text_sm()
                    .font_family("Menlo")
                    .text_color(gpui::rgb(t.text_faint))
                    .child("No items.")
                    .into_any_element(),
            );
        }

        // Position the panel according to PanelPosition.
        let (backdrop_flex_dir, backdrop_items, backdrop_pt, backdrop_pr, backdrop_pb, backdrop_pl) =
            match &self.panel.position {
                PanelPosition::Sidebar => (
                    true, // flex_col + items_end = right side
                    "end", px(8.0), px(8.0), px(8.0), px(0.0),
                ),
                PanelPosition::Bottom => (
                    false, // flex_row + items_end = bottom
                    "bottom", px(0.0), px(8.0), px(8.0), px(8.0),
                ),
                PanelPosition::Float => (
                    true,
                    "center", px(100.0), px(0.0), px(0.0), px(0.0),
                ),
            };

        let _ = (backdrop_flex_dir, backdrop_items, backdrop_pt, backdrop_pr, backdrop_pb, backdrop_pl);

        let modal_width = match &self.panel.position {
            PanelPosition::Bottom => px(800.0),
            _ => px(400.0),
        };

        let title = self.panel.title.clone();

        div()
            .absolute()
            .inset_0()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::handle_key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(PluginPanelEvent::Close)),
            )
            .flex()
            .flex_col()
            .items_center()
            .pt(px(48.0))
            .child(
                div()
                    .w(modal_width)
                    .bg(gpui::rgb(t.bg_surface))
                    .border_1()
                    .border_color(gpui::rgb(t.border_subtle))
                    .rounded(px(8.0))
                    .overflow_hidden()
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    // Header
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
                                    .child("⬛"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_family("Menlo")
                                    .text_color(gpui::rgb(t.text_muted))
                                    .child(title),
                            ),
                    )
                    // Content
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .max_h(px(400.0))
                            .overflow_hidden()
                            .children(rows),
                    ),
            )
    }
}
