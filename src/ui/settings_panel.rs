//! Settings panel — a small GUI over the most-used `Settings` fields so users
//! don't have to hand-edit `~/.config/ockr/settings.toml`.
//!
//! Each row shows the current value (read live from the `Settings` global) and
//! cycles to the next on click.  The panel only emits a `Cycle(key)` intent;
//! `MainWindow` owns applying the change live and persisting it via
//! `save_global_setting`, so this view stays stateless.

use gpui::{
    Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton, Render, Window, div,
    prelude::*, px,
};

use crate::settings::Settings;
use crate::ui::theme::ThemePalette;

// ── Events ──────────────────────────────────────────────────────────────────

/// Which setting a row controls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SettingKey {
    Keyboard,
    Theme,
    LineNumbers,
    Preview,
}

#[derive(Debug, Clone)]
pub enum SettingsPanelEvent {
    Close,
    /// Advance the given setting to its next value.
    Cycle(SettingKey),
}

impl EventEmitter<SettingsPanelEvent> for SettingsPanel {}

// ── View ──────────────────────────────────────────────────────────────────────

pub struct SettingsPanel {
    pub focus_handle: FocusHandle,
}

impl SettingsPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self { focus_handle: cx.focus_handle() }
    }

    fn handle_key(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" {
            cx.emit(SettingsPanelEvent::Close);
        }
    }
}

impl Focusable for SettingsPanel {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

impl Render for SettingsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.global::<ThemePalette>().clone();
        let s = cx.global::<Settings>().clone();

        // Current display value per row.
        let kbd = if s.keyboard_mode == "standard" { "Standard" } else { "Helix" };
        let theme = if s.theme == "ochre" { "Ochre (light)" } else { "Oxide (dark)" };
        let lines = match s.line_number_mode.as_str() {
            "absolute" => "Absolute",
            "off" => "Off",
            _ => "Relative",
        };
        let preview = if s.preview_mode == "paged" { "Paged / PDF" } else { "HTML" };

        let rows = [
            (SettingKey::Keyboard, "Keyboard mode", kbd),
            (SettingKey::Theme, "Theme", theme),
            (SettingKey::LineNumbers, "Line numbers", lines),
            (SettingKey::Preview, "Preview", preview),
        ];

        let mut list = div().flex().flex_col();
        for (key, label, value) in rows {
            let bg_hover = t.bg_hover;
            list = list.child(
                div()
                    .id(label)
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .px(px(16.0))
                    .py(px(9.0))
                    .text_sm()
                    .font_family("Menlo")
                    .cursor_pointer()
                    .hover(move |st| st.bg(gpui::rgb(bg_hover)))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(SettingsPanelEvent::Cycle(key));
                    }))
                    .child(div().text_color(gpui::rgb(t.text_muted)).child(label))
                    .child(div().text_color(gpui::rgb(t.ochre)).child(value)),
            );
        }

        div()
            .absolute()
            .inset_0()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::handle_key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(SettingsPanelEvent::Close)),
            )
            .flex()
            .flex_col()
            .items_center()
            .pt(px(80.0))
            .child(
                div()
                    .w(px(440.0))
                    .bg(gpui::rgb(t.bg_surface))
                    .border_1()
                    .border_color(gpui::rgb(t.border_subtle))
                    .rounded(px(8.0))
                    .shadow_lg()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(11.0))
                            .border_b_1()
                            .border_color(gpui::rgb(t.border_subtle))
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .font_family("Menlo")
                            .text_color(gpui::rgb(t.text))
                            .child("Settings"),
                    )
                    .child(list)
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(7.0))
                            .border_t_1()
                            .border_color(gpui::rgb(t.border_subtle))
                            .text_xs()
                            .font_family("Menlo")
                            .text_color(gpui::rgb(t.text_faint))
                            .child("Click a row to cycle · saved to settings.toml · Esc to close"),
                    ),
            )
    }
}
