//! File-tree sidebar.
//!
//! Story 02: shows vault `.typ` files as a flat sorted list.
//! Story 06: clicking a file emits `SidebarEvent::OpenFile(path)` so that
//!           `MainWindow` can load it into `EditorPane`.
//!
//! UI code is stateless — this view reads from `Entity<VaultState>` on every
//! render and holds no mutable file-list state of its own.

use std::path::PathBuf;

use gpui::{App, Context, Entity, FocusHandle, Focusable, Render, Window, div, prelude::*};

use crate::ui::theme::ThemePalette;
use crate::vault::VaultState;

// ── Events ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum SidebarEvent {
    /// User clicked a file row.  Carries the absolute path of the file.
    OpenFile(PathBuf),
}

pub struct Sidebar {
    pub focus_handle: FocusHandle,
    vault: Entity<VaultState>,
}

impl Sidebar {
    pub fn new(vault: Entity<VaultState>, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            vault,
        }
    }
}

impl gpui::EventEmitter<SidebarEvent> for Sidebar {}

impl Focusable for Sidebar {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.global::<ThemePalette>().clone();
        let vault = self.vault.read(cx);

        let vault_name = vault
            .root
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("No vault")
            .to_owned();

        let header = div()
            .px_3()
            .py_2()
            .text_sm()
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(gpui::rgb(t.text_subtle))
            .child(vault_name);

        let body = if vault.files.is_empty() {
            div()
                .px_3()
                .py_2()
                .text_sm()
                .text_color(gpui::rgb(t.text_faint))
                .child(if vault.root.is_some() {
                    "No .typ files found"
                } else {
                    "Cmd-O to open vault"
                })
                .into_any_element()
        } else {
            let mut rows = div().flex().flex_col();
            for (i, file) in vault.files.iter().enumerate() {
                let abs_path = file.abs_path.clone();
                let bg_hover = t.bg_hover;
                let text_muted = t.text_muted;
                rows = rows.child(
                    div()
                        .id(gpui::ElementId::Integer(i as u64))
                        .px_3()
                        .py_1()
                        .text_sm()
                        .text_color(gpui::rgb(text_muted))
                        .hover(move |s| s.bg(gpui::rgb(bg_hover)))
                        .cursor_pointer()
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(SidebarEvent::OpenFile(abs_path.clone()));
                        }))
                        .child(file.title.clone()),
                );
            }
            rows.into_any_element()
        };

        div()
            .track_focus(&self.focus_handle)
            .flex()
            .flex_col()
            .h_full()
            .w(gpui::px(220.0))
            .bg(gpui::rgb(t.bg_surface))
            .border_r_1()
            .border_color(gpui::rgb(t.border_subtle))
            .child(header)
            .child(body)
    }
}
