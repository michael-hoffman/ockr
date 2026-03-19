//! Root view of the main application window.
//!
//! Stories 05–06: horizontal split — sidebar | editor | preview.
//! The editor and preview are connected to the background compiler thread.
//!
//! ## Resizable panes
//!
//! Each pane divider is a 4 px drag handle.  Dragging it updates the pixel
//! width of its adjacent pane; the remaining space is divided by flex between
//! the other two panes.  Widths are clamped to sane minimums and maximums.
//!
//! `drag_state` tracks which handle is being dragged, the cursor's starting
//! x position, and the pane width at drag-start.  `MouseMove` on the root div
//! updates the width live; `MouseUp` clears the drag state.

use std::path::PathBuf;

use futures::StreamExt as _;
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Render, Window, deferred, div, prelude::*, px,
};

use crate::actions::{
    BufferClose, BufferNext, BufferPrevious, ForceQuit, NewNote, OpenCommandPalette, OpenVault,
    Quit, ReloadFile, SaveFile, SaveFileAndQuit, ToggleSidebar,
};
use crate::compiler::{spawn_compiler_thread, CompileResult};
use crate::ui::command_palette::{CommandPalette, PaletteEvent};
use crate::ui::editor_pane::EditorPane;
use crate::ui::preview::PreviewPane;
use crate::ui::sidebar::{Sidebar, SidebarEvent};
use crate::ui::theme;
use crate::vault::VaultState;

// ── Drag state ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum DragTarget {
    /// The handle between the sidebar and the editor.
    Sidebar,
    /// The handle between the editor and the preview.
    Preview,
}

struct DragState {
    target: DragTarget,
    /// Window-x at mouse-down.
    start_x: f32,
    /// Pane width at mouse-down.
    start_width: f32,
}

// ── View ──────────────────────────────────────────────────────────────────────

pub struct MainWindow {
    pub focus_handle: FocusHandle,
    sidebar: Entity<Sidebar>,
    editor: Entity<EditorPane>,
    preview: Entity<PreviewPane>,
    sidebar_visible: bool,
    /// Width of the sidebar pane in pixels.
    sidebar_width: f32,
    /// Width of the preview pane in pixels.
    preview_width: f32,
    drag: Option<DragState>,
    /// Command palette overlay — `Some` while the palette is open.
    palette: Option<Entity<CommandPalette>>,
}

impl MainWindow {
    pub fn new(vault: Entity<VaultState>, cx: &mut Context<Self>) -> Self {
        let sidebar = cx.new(|cx| Sidebar::new(vault.clone(), cx));
        let preview = cx.new(|_| PreviewPane::new());
        let editor = cx.new(|cx| EditorPane::new(cx));

        // ── Compiler thread ──────────────────────────────────────────────────
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<CompileResult>();

        let compiler_handle = spawn_compiler_thread(move |result| {
            let _ = tx.unbounded_send(result);
        });

        let preview_for_editor = preview.clone();
        editor.update(cx, |pane, _cx| {
            pane.set_compiler(compiler_handle, preview_for_editor);
        });

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
            sidebar_width: 220.0,
            preview_width: 420.0,
            drag: None,
            palette: None,
        }
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_visible = !self.sidebar_visible;
        cx.notify();
    }

    fn open_palette(
        &mut self,
        _: &OpenCommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // If already open, close it (toggle).
        if self.palette.is_some() {
            self.palette = None;
            cx.notify();
            return;
        }

        let palette = cx.new(|cx| CommandPalette::new(cx));

        // Focus the palette so it captures key events.
        palette.read(cx).focus_handle.clone().focus(window);

        cx.subscribe(&palette, |this, _, event: &PaletteEvent, cx| {
            match event {
                PaletteEvent::Close => {
                    this.palette = None;
                    cx.notify();
                }
                PaletteEvent::Execute(id) => {
                    this.palette = None;
                    cx.notify();
                    // Dispatch the corresponding GPUI action into the focus chain.
                    match *id {
                        // Helix :commands
                        "write" | "save-file" => cx.dispatch_action(&SaveFile),
                        "write-quit" => cx.dispatch_action(&SaveFileAndQuit),
                        "quit" => cx.dispatch_action(&Quit),
                        "quit-force" => cx.dispatch_action(&ForceQuit),
                        "reload" => cx.dispatch_action(&ReloadFile),
                        "open" | "open-vault" => cx.dispatch_action(&OpenVault),
                        "new" | "new-note" => cx.dispatch_action(&NewNote),
                        "buffer-next" => cx.dispatch_action(&BufferNext),
                        "buffer-previous" => cx.dispatch_action(&BufferPrevious),
                        "buffer-close" => cx.dispatch_action(&BufferClose),
                        // GUI commands
                        "toggle-sidebar" => cx.dispatch_action(&ToggleSidebar),
                        "open-command-palette" => cx.dispatch_action(&OpenCommandPalette),
                        _ => {} // other commands are stubs
                    }
                }
            }
        })
        .detach();

        self.palette = Some(palette);
        cx.notify();
    }

    fn on_handle_mouse_down(
        &mut self,
        target: DragTarget,
        event: &MouseDownEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let start_width = match target {
            DragTarget::Sidebar => self.sidebar_width,
            DragTarget::Preview => self.preview_width,
        };
        self.drag = Some(DragState {
            target,
            start_x: f32::from(event.position.x),
            start_width,
        });
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ref drag) = self.drag else { return };
        let dx = f32::from(event.position.x) - drag.start_x;
        match drag.target {
            DragTarget::Sidebar => {
                self.sidebar_width = (drag.start_width + dx).clamp(120.0, 480.0);
            }
            DragTarget::Preview => {
                // Dragging the preview handle right → preview shrinks.
                self.preview_width = (drag.start_width - dx).clamp(200.0, 900.0);
            }
        }
        cx.notify();
    }

    fn on_mouse_up(
        &mut self,
        _event: &MouseUpEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.drag = None;
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

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
        pane.open_file_no_focus(&file, vault_root, cx);
    });
}

// ── Focusable ─────────────────────────────────────────────────────────────────

impl Focusable for MainWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Drag handle: a 4px vertical strip with ew-resize cursor.
        let handle = |target: DragTarget, cx: &mut Context<Self>| {
            div()
                .w(px(4.0))
                .h_full()
                .cursor_ew_resize()
                .bg(gpui::rgb(theme::BORDER_SUBTLE))
                .hover(|s| s.bg(gpui::rgb(theme::OCHRE_DIM)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event, window, cx| {
                        this.on_handle_mouse_down(target, event, window, cx);
                    }),
                )
        };

        let mut root = div()
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_row()
            .bg(gpui::rgb(theme::BG_SURFACE))
            .on_action(cx.listener(Self::open_palette))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up));

        if self.sidebar_visible {
            root = root
                .child(
                    div()
                        .w(px(self.sidebar_width))
                        .h_full()
                        .overflow_hidden()
                        .child(self.sidebar.clone()),
                )
                .child(handle(DragTarget::Sidebar, cx));
        }

        root.child(
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .overflow_hidden()
                .child(self.editor.clone()),
        )
        .child(handle(DragTarget::Preview, cx))
        .child(
            div()
                .w(px(self.preview_width))
                .h_full()
                .overflow_hidden()
                .child(self.preview.clone()),
        )
        // Command palette overlay — painted after all other content via deferred().
        .when_some(self.palette.clone(), |root, palette| {
            root.child(gpui::deferred(palette).with_priority(100))
        })
    }
}
