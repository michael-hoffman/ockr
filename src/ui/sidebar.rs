//! File-tree sidebar.
//!
//! Story 02: shows vault `.typ` files as a flat sorted list.
//! Story 06: clicking a file emits `SidebarEvent::OpenFile(path)` so that
//!           `MainWindow` can load it into `EditorPane`.
//!
//! UI code is stateless — this view reads from `Entity<VaultState>` on every
//! render and holds no mutable file-list state of its own.

use std::path::PathBuf;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, MouseButton, MouseDownEvent, Pixels, Point,
    Render, Window, div, prelude::*, px,
};

use crate::actions::NewNote;
use crate::plugin::panel::PanelPosition;
use crate::plugin::registry::PluginRegistry;
use crate::ui::theme::ThemePalette;
use crate::vault::VaultState;

// ── Events ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum SidebarEvent {
    /// User clicked a file row.  Carries the absolute path of the file.
    OpenFile(PathBuf),
    /// User chose "Delete" — move the file to the system Trash.
    DeleteFile(PathBuf),
    /// User chose "Reveal in Finder".
    RevealFile(PathBuf),
    /// User clicked a plugin panel button in the sidebar.
    OpenPluginPanel {
        plugin_id: String,
        panel_id: String,
    },
}

/// Right-click context menu state: which file, anchored at which window point.
struct ContextMenu {
    path: PathBuf,
    pos: Point<Pixels>,
}

pub struct Sidebar {
    pub focus_handle: FocusHandle,
    vault: Entity<VaultState>,
    /// Open file context menu (right-click), if any.
    context_menu: Option<ContextMenu>,
}

impl Sidebar {
    pub fn new(vault: Entity<VaultState>, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            vault,
            context_menu: None,
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

        let new_note_btn = div()
            .id("sidebar-new-note")
            .px_2()
            .text_sm()
            .text_color(gpui::rgb(t.text_faint))
            .cursor_pointer()
            .hover(|s| s.text_color(gpui::rgb(t.ochre)))
            .child("＋")
            .on_click(cx.listener(|_, _, _window, cx| {
                cx.dispatch_action(&NewNote);
            }));

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_3()
            .py_2()
            .text_sm()
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(gpui::rgb(t.text_subtle))
            .child(div().child(vault_name))
            .when(vault.root.is_some(), |d| d.child(new_note_btn));

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
                let abs_path_ctx = file.abs_path.clone();
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
                        // Right-click opens the file context menu at the cursor.
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, ev: &MouseDownEvent, _window, cx| {
                                cx.stop_propagation();
                                this.context_menu = Some(ContextMenu {
                                    path: abs_path_ctx.clone(),
                                    pos: ev.position,
                                });
                                cx.notify();
                            }),
                        )
                        .child(file.title.clone()),
                );
            }
            rows.into_any_element()
        };

        // "Indexing…" chip shown while backlink build is in progress (Story 36).
        let indexing_banner = if vault.indexing {
            div()
                .px_3()
                .py_1()
                .text_xs()
                .font_family("Menlo")
                .text_color(gpui::rgb(t.text_faint))
                .child("⟳ Indexing links…")
                .into_any_element()
        } else {
            div().into_any_element()
        };

        // ── Plugin panels registered for sidebar position ─────────────────────
        let plugin_panel_buttons: Vec<gpui::AnyElement> = if let Some(reg) = cx.try_global::<PluginRegistry>() {
            let mut buttons: Vec<gpui::AnyElement> = Vec::new();
            // Collect all sidebar panels from all plugins, sorted by title.
            let mut sidebar_panels: Vec<(String, String, String)> = reg.plugin_panels
                .iter()
                .flat_map(|(plugin_id, panels)| {
                    panels.iter()
                        .filter(|p| matches!(p.position, PanelPosition::Sidebar))
                        .map(|p| (plugin_id.clone(), p.panel_id.clone(), p.title.clone()))
                        .collect::<Vec<_>>()
                })
                .collect();
            sidebar_panels.sort_by(|a, b| a.2.cmp(&b.2));

            for (btn_idx, (plugin_id, panel_id, title)) in sidebar_panels.into_iter().enumerate() {
                let pid = plugin_id.clone();
                let panid = panel_id.clone();
                let bg_hover = t.bg_hover;
                let accent = t.ochre;
                // Offset element IDs past the file list (which uses 0..N).
                let elem_id = gpui::ElementId::Integer((1_000_000 + btn_idx) as u64);
                buttons.push(
                    div()
                        .id(elem_id)
                        .px_3()
                        .py_1()
                        .text_sm()
                        .text_color(gpui::rgb(accent))
                        .hover(move |s| s.bg(gpui::rgb(bg_hover)))
                        .cursor_pointer()
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(SidebarEvent::OpenPluginPanel {
                                plugin_id: pid.clone(),
                                panel_id: panid.clone(),
                            });
                        }))
                        .child(format!("⬡ {}", title))
                        .into_any_element(),
                );
            }
            buttons
        } else {
            Vec::new()
        };

        let plugin_section = if plugin_panel_buttons.is_empty() {
            div().into_any_element()
        } else {
            let mut col = div()
                .flex()
                .flex_col()
                .mt_2()
                .border_t_1()
                .border_color(gpui::rgb(t.border_subtle));
            col = col.child(
                div()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(gpui::rgb(t.text_faint))
                    .child("PLUGINS"),
            );
            for btn in plugin_panel_buttons {
                col = col.child(btn);
            }
            col.into_any_element()
        };

        // ── Right-click context menu overlay ──────────────────────────────────
        let context_menu = self.context_menu.as_ref().map(|menu| {
            let path_del = menu.path.clone();
            let path_rev = menu.path.clone();
            let item = |label: &str, danger: bool, t: &ThemePalette| {
                let color = if danger { 0xff6b6bu32 } else { t.text };
                div()
                    .px_3()
                    .py_1()
                    .text_sm()
                    .font_family("Menlo")
                    .text_color(gpui::rgb(color))
                    .cursor_pointer()
                    .hover(move |s| s.bg(gpui::rgb(t.bg_hover)))
                    .child(label.to_string())
            };

            // Backdrop: clicking anywhere (incl. left/right) dismisses.
            let backdrop = div()
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.context_menu = None;
                        cx.notify();
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, _, cx| {
                        this.context_menu = None;
                        cx.notify();
                    }),
                );

            let menu_box = div()
                .absolute()
                .left(menu.pos.x)
                .top(menu.pos.y)
                .min_w(px(160.0))
                .bg(gpui::rgb(t.bg_surface))
                .border_1()
                .border_color(gpui::rgb(t.border_subtle))
                .rounded(px(6.0))
                .shadow_lg()
                .py(px(4.0))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    item("Reveal in Finder", false, &t)
                        .id("ctx-reveal")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.emit(SidebarEvent::RevealFile(path_rev.clone()));
                            this.context_menu = None;
                            cx.notify();
                        })),
                )
                .child(
                    item("Delete to Trash", true, &t)
                        .id("ctx-delete")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.emit(SidebarEvent::DeleteFile(path_del.clone()));
                            this.context_menu = None;
                            cx.notify();
                        })),
                );

            gpui::deferred(div().absolute().inset_0().child(backdrop).child(menu_box))
                .with_priority(200)
        });

        div()
            .track_focus(&self.focus_handle)
            .relative()
            .flex()
            .flex_col()
            .h_full()
            .w(gpui::px(220.0))
            .bg(gpui::rgb(t.bg_surface))
            .border_r_1()
            .border_color(gpui::rgb(t.border_subtle))
            .child(header)
            .child(indexing_banner)
            .child(body)
            .child(plugin_section)
            .when_some(context_menu, |root, menu| root.child(menu))
    }
}
