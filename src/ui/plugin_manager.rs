//! Plugin manager panel — lists installed plugins with status and capabilities.
//!
//! Opened with `open-plugin-manager` from the command palette.
//! Shows each plugin's id, name, version, capabilities, and load status.

use gpui::{
    Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton,
    Render, Window, div, prelude::*, px,
};

use crate::plugin::registry::{PluginInfo, PluginStatus};
use crate::ui::theme::ThemePalette;

// ── Events ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum PluginManagerEvent {
    Close,
}

impl EventEmitter<PluginManagerEvent> for PluginManager {}

// ── View ──────────────────────────────────────────────────────────────────────

pub struct PluginManager {
    pub focus_handle: FocusHandle,
    /// Snapshot of plugin info taken when the panel was opened.
    plugins: Vec<(PluginInfo, PluginStatus)>,
}

impl PluginManager {
    pub fn new(
        mut plugins: Vec<(PluginInfo, PluginStatus)>,
        cx: &mut Context<Self>,
    ) -> Self {
        plugins.sort_by(|a, b| a.0.id.cmp(&b.0.id));
        Self {
            focus_handle: cx.focus_handle(),
            plugins,
        }
    }

    fn handle_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key == "escape" {
            cx.emit(PluginManagerEvent::Close);
        }
    }
}

impl Focusable for PluginManager {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

impl Render for PluginManager {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.global::<ThemePalette>().clone();

        let mut rows: Vec<gpui::AnyElement> = Vec::new();

        if self.plugins.is_empty() {
            rows.push(
                div()
                    .px(px(16.0))
                    .py(px(16.0))
                    .text_sm()
                    .font_family("Menlo")
                    .text_color(gpui::rgb(t.text_faint))
                    .child("No plugins installed in this vault.")
                    .into_any_element(),
            );
        } else {
            for (info, status) in &self.plugins {
                let (status_label, status_color) = match status {
                    PluginStatus::Loaded    => ("loaded",  t.mode_insert),
                    PluginStatus::Failed(_) => ("failed",  0xff5555u32),
                };
                let error_msg = match status {
                    PluginStatus::Failed(msg) => Some(msg.clone()),
                    _ => None,
                };

                // Capability pills
                let mut caps: Vec<&'static str> = Vec::new();
                if info.capabilities.file_read   { caps.push("file_read"); }
                if info.capabilities.vault_write { caps.push("vault_write"); }
                if info.capabilities.network     { caps.push("network"); }
                if info.capabilities.console     { caps.push("console"); }
                let caps_str = if caps.is_empty() {
                    "no capabilities".to_string()
                } else {
                    caps.join("  ")
                };

                let name = info.name.clone();
                let version = format!("v{}", info.version);
                let id_str = info.id.clone();

                let mut row = div()
                    .flex()
                    .flex_col()
                    .px(px(16.0))
                    .py(px(10.0))
                    .border_b_1()
                    .border_color(gpui::rgb(t.border_subtle))
                    // Name + status badge row
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_sm()
                                    .font_family("Menlo")
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(gpui::rgb(t.text))
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_family("Menlo")
                                    .text_color(gpui::rgb(t.text_faint))
                                    .child(version),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_family("Menlo")
                                    .text_color(gpui::rgb(status_color))
                                    .child(status_label),
                            ),
                    )
                    // ID row
                    .child(
                        div()
                            .mt(px(2.0))
                            .text_xs()
                            .font_family("Menlo")
                            .text_color(gpui::rgb(t.text_faint))
                            .child(id_str),
                    )
                    // Capabilities row
                    .child(
                        div()
                            .mt(px(3.0))
                            .text_xs()
                            .font_family("Menlo")
                            .text_color(gpui::rgb(t.text_muted))
                            .child(caps_str),
                    );

                // Error detail (if failed)
                if let Some(msg) = error_msg {
                    let short = if msg.len() > 120 { format!("{}…", &msg[..120]) } else { msg };
                    row = row.child(
                        div()
                            .mt(px(4.0))
                            .text_xs()
                            .font_family("Menlo")
                            .text_color(gpui::rgb(0xff5555u32))
                            .child(short),
                    );
                }

                rows.push(row.into_any_element());
            }
        }

        div()
            .absolute()
            .inset_0()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::handle_key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(PluginManagerEvent::Close)),
            )
            .flex()
            .flex_col()
            .items_center()
            .justify_start()
            .pt(px(60.0))
            .child(
                div()
                    .w(px(480.0))
                    .bg(gpui::rgb(t.bg_surface))
                    .border_1()
                    .border_color(gpui::rgb(t.border_subtle))
                    .rounded(px(8.0))
                    .overflow_hidden()
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    // Header
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(10.0))
                            .border_b_1()
                            .border_color(gpui::rgb(t.border_subtle))
                            .bg(gpui::rgb(t.bg_panel))
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_sm()
                                    .font_family("Menlo")
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(gpui::rgb(t.text_muted))
                                    .child("Plugin Manager"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_family("Menlo")
                                    .text_color(gpui::rgb(t.text_faint))
                                    .child("Esc to close"),
                            ),
                    )
                    // Plugin list (stop click propagation so overlay click-to-close doesn't fire)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .max_h(px(480.0))
                            .overflow_hidden()
                            .on_mouse_down(
                                MouseButton::Left,
                                |_, _, cx| { cx.stop_propagation(); },
                            )
                            .children(rows),
                    ),
            )
    }
}
