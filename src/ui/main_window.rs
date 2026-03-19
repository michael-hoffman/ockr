//! Root view of the main application window.
//!
//! Stories 05–06: horizontal split — sidebar | editor | preview.
//! The editor and preview are connected to the background compiler thread.
//!
//! ## Compiler wiring
//!
//! 1. `spawn_compiler_thread` is called once with a callback that sends
//!    `CompileResult` values into an unbounded `futures::channel::mpsc` channel.
//! 2. A detached `cx.spawn` task continuously `await`s results from that
//!    channel and delivers them to `Entity<PreviewPane>` via `cx.update`.
//! 3. `EditorPane` holds the `CompilerHandle` and calls `handle.send(...)` on
//!    every buffer change.
//!
//! UI code is stateless — no rendering decisions are stored in this struct
//! beyond which panes are currently visible.

use std::path::PathBuf;
use std::sync::Arc;

use futures::StreamExt as _;
use gpui::{App, Context, Entity, FocusHandle, Focusable, Render, Window, div, prelude::*};

use crate::actions::{OpenCommandPalette, ToggleSidebar};
use crate::compiler::{spawn_compiler_thread, CompileResult};
use crate::ui::editor_pane::EditorPane;
use crate::ui::preview::PreviewPane;
use crate::ui::sidebar::{Sidebar, SidebarEvent};
use crate::ui::theme;
use crate::vault::VaultState;

pub struct MainWindow {
    pub focus_handle: FocusHandle,
    sidebar: Entity<Sidebar>,
    editor: Entity<EditorPane>,
    preview: Entity<PreviewPane>,
    sidebar_visible: bool,
}

impl MainWindow {
    pub fn new(vault: Entity<VaultState>, cx: &mut Context<Self>) -> Self {
        let sidebar = cx.new(|cx| Sidebar::new(vault.clone(), cx));
        let preview = cx.new(|_| PreviewPane::new());
        let editor = cx.new(|cx| EditorPane::new(cx));

        // ── Compiler thread ──────────────────────────────────────────────────
        // Channel for delivering CompileResult from the compiler thread to GPUI.
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<CompileResult>();

        let compiler_handle = spawn_compiler_thread(move |result| {
            // Called on the compiler thread — unbounded_send is non-blocking.
            let _ = tx.unbounded_send(result);
        });

        // Wire the compiler handle into the editor pane.
        let preview_for_editor = preview.clone();
        editor.update(cx, |pane, _cx| {
            pane.set_compiler(compiler_handle, preview_for_editor);
        });

        // Spawn a task to deliver compile results to the preview pane.
        // Context::spawn takes (WeakEntity<Self>, &mut AsyncApp); we ignore the first arg.
        let preview_for_task = preview.clone();
        cx.spawn(async move |_this, cx| {
            while let Some(result) = rx.next().await {
                let preview = preview_for_task.clone();
                cx.update(|cx| {
                    preview.update(cx, |pane, cx| match result {
                        CompileResult::Ok(doc) => pane.set_document(doc, cx),
                        CompileResult::Err(diags) => {
                            let msg = diags
                                .first()
                                .map(|d| d.message.clone())
                                .unwrap_or_else(|| "Unknown error".to_string());
                            pane.set_error(msg, cx);
                        }
                        CompileResult::Panicked(msg) => {
                            pane.set_error(format!("Compiler panicked: {msg}"), cx);
                        }
                    });
                })
                .ok();
            }
        })
        .detach();

        // ── Sidebar → editor wiring ──────────────────────────────────────────
        let editor_for_sub = editor.clone();
        let vault_for_sub = vault.clone();
        cx.subscribe(&sidebar, move |_, _, event: &SidebarEvent, cx| {
            match event {
                SidebarEvent::OpenFile(abs_path) => {
                    open_file_in_editor(abs_path, &editor_for_sub, &vault_for_sub, cx);
                }
            }
        })
        .detach();

        Self {
            focus_handle: cx.focus_handle(),
            sidebar,
            editor,
            preview,
            sidebar_visible: true,
        }
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_visible = !self.sidebar_visible;
        cx.notify();
    }
}

/// Load the file at `abs_path` into `editor`, using `vault` to resolve the
/// vault root.  No-op if the vault is not open.
fn open_file_in_editor(
    abs_path: &PathBuf,
    editor: &Entity<EditorPane>,
    vault: &Entity<VaultState>,
    cx: &mut App,
) {
    let vault_root = match vault.read(cx).root.clone() {
        Some(r) => r,
        None => return,
    };

    let rel_path = abs_path
        .strip_prefix(&vault_root)
        .unwrap_or(abs_path)
        .to_path_buf();
    let title = abs_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_string();

    let file = crate::vault::VaultFile {
        rel_path,
        abs_path: abs_path.clone(),
        title,
    };

    editor.update(cx, |pane, cx| {
        // `open_file` needs a Window reference for focus management.
        // We don't have one here (subscribe callback is App-only), so we
        // defer focus to the next render via cx.notify.
        // A proper focus transfer is wired in a future story via Window access.
        pane.open_file_no_focus(&file, vault_root, cx);
    });
}

impl Focusable for MainWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = div()
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_row()
            .bg(gpui::rgb(theme::BG_SURFACE))
            .on_action(cx.listener(|_this, _: &OpenCommandPalette, _window, _cx| {
                // Story 08: open command palette UI
            }))
            .on_action(cx.listener(Self::toggle_sidebar));

        if self.sidebar_visible {
            root = root.child(self.sidebar.clone());
        }

        root = root
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .child(self.editor.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .border_l_1()
                    .border_color(gpui::rgb(theme::BORDER_SUBTLE))
                    .child(self.preview.clone()),
            );

        root
    }
}
