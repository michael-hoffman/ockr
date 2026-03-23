//! Root view of the main application window.
//!
//! ## Layout
//!
//!   [sidebar] [editor area] [preview]
//!
//! The editor area can contain 1 or 2 `EditorPane` views:
//!
//! - **Single** (default): one editor filling the area.
//! - **Vertical** (Cmd-\): two editors side-by-side with a drag handle.
//! - **Horizontal** (Cmd-Shift-\): two editors stacked with a drag handle.
//!
//! The preview column always shows the *active* pane's compiled output.
//! Each pane owns an independent `EditorPane` (cursor, mode, undo history).
//! All panes share one compiler thread and one preview pane; the most-recently
//! edited active pane wins.
//!
//! ## Resizable handles
//!
//! Three drag handles exist:
//! - `Sidebar`     — between sidebar and editor area.
//! - `PaneDivider` — between the two editor sub-panes (split mode only).
//! - `Preview`     — between editor area and preview.
//!
//! Each handle is a 4 px strip.  Dragging updates the adjacent width.

use std::path::PathBuf;

use futures::StreamExt as _;
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Render, Window, div, prelude::*, px,
};

use crate::actions::{
    BufferClose, BufferNext, BufferPrevious, ClosePane, FocusPaneDown, FocusPaneLeft,
    FocusPaneRight, FocusPaneUp, ForceQuit, LineNumbersAbsolute, LineNumbersOff,
    LineNumbersRelative, NewNote, OpenBacklinks, OpenCommandPalette, OpenDailyNote, OpenGraphView,
    OpenQuickSwitch, OpenVault, OpenVaultSearch, Quit, ReloadFile, SaveFile, SaveFileAndQuit,
    SplitPaneHorizontal, SplitPaneVertical, TogglePreviewMode, ToggleSidebar,
};
use crate::compiler::{spawn_compiler_thread, CompileResult, CompilerHandle, PreviewMode};
use crate::ui::backlink_panel::{BacklinkPanel, BacklinkPanelEvent};
use crate::ui::graph_view::{GraphView, GraphViewEvent};
use crate::ui::command_palette::{CommandPalette, PaletteEvent};
use crate::ui::html_preview::HtmlWebView;
use crate::ui::quick_switch::{QuickSwitch, QuickSwitchEvent};
use crate::ui::template_picker::{
    TemplatePicker, TemplatePickerEvent, heading_to_filename_stem, scan_templates,
};
use crate::ui::vault_search::{VaultSearch, VaultSearchEvent};
use crate::ui::editor_pane::{EditorPane, EditorPaneEvent};
use crate::ui::preview::PreviewPane;
use crate::ui::sidebar::{Sidebar, SidebarEvent};
use crate::ui::theme::ThemePalette;
use crate::vault::VaultState;

// ── Split layout ──────────────────────────────────────────────────────────────

/// How the editor area is divided.
#[derive(Clone, Copy, PartialEq, Default, Debug)]
enum SplitLayout {
    /// A single editor fills the area.
    #[default]
    Single,
    /// Two editors side-by-side (Cmd-\).
    Vertical,
    /// Two editors stacked (Cmd-Shift-\).
    Horizontal,
}

// ── Pane entry ────────────────────────────────────────────────────────────────

/// One slot in the editor area.
struct PaneEntry {
    editor: Entity<EditorPane>,
}

// ── Drag state ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum DragTarget {
    Sidebar,
    PaneDivider,
    Preview,
}

struct DragState {
    target: DragTarget,
    start_x: f32,
    start_y: f32,
    start_width: f32,
}

// ── View ──────────────────────────────────────────────────────────────────────

pub struct MainWindow {
    pub focus_handle: FocusHandle,
    sidebar: Entity<Sidebar>,
    /// All open editor panes, at least one.
    panes: Vec<PaneEntry>,
    /// Index into `panes` of the currently focused pane.
    active_idx: usize,
    /// Current split mode.
    split_layout: SplitLayout,
    /// Fraction [0.2, 0.8] of the editor area given to pane 0 in split mode.
    pane_split_frac: f32,
    /// Shared compiler handle — cloned into each new pane on creation.
    compiler_handle: CompilerHandle,
    /// PDF rasterised preview (paged mode).
    preview: Entity<PreviewPane>,
    /// HTML preview via WKWebView (lazily created on first HTML-mode render).
    html_webview: Option<HtmlWebView>,
    vault: Entity<VaultState>,
    sidebar_visible: bool,
    sidebar_width: f32,
    preview_width: f32,
    drag: Option<DragState>,
    palette: Option<Entity<CommandPalette>>,
    /// True when the palette was just created and needs focus on next render.
    palette_focus_pending: bool,
    quick_switch: Option<Entity<QuickSwitch>>,
    template_picker: Option<Entity<TemplatePicker>>,
    backlinks: Option<Entity<BacklinkPanel>>,
    vault_search: Option<Entity<VaultSearch>>,
    graph_view: Option<Entity<GraphView>>,
    recent_paths: Vec<PathBuf>,
}

impl MainWindow {
    pub fn new(vault: Entity<VaultState>, cx: &mut Context<Self>) -> Self {
        let sidebar = cx.new(|cx| Sidebar::new(vault.clone(), cx));
        let preview = cx.new(|_| PreviewPane::new());

        // ── Compiler thread ──────────────────────────────────────────────────
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<CompileResult>();
        let compiler_handle = spawn_compiler_thread(move |result| {
            let _ = tx.unbounded_send(result);
        });

        // ── Initial pane ─────────────────────────────────────────────────────
        let editor = cx.new(|cx| EditorPane::new(cx));
        editor.update(cx, |pane, _cx| pane.set_vault(vault.clone()));
        editor.update(cx, |pane, _cx| {
            pane.set_compiler(compiler_handle.clone(), preview.clone());
        });

        // ── Compiler result → preview ────────────────────────────────────────
        let preview_for_task = preview.clone();
        cx.spawn(async move |this, cx| {
            while let Some(result) = rx.next().await {
                let preview = preview_for_task.clone();
                cx.update(|cx| {
                    match result {
                        CompileResult::OkHtml(ref html) => {
                            let html = html.clone();
                            this.update(cx, |win, _cx| {
                                if let Some(ref wv) = win.html_webview {
                                    wv.load_html(&html);
                                }
                            }).ok();
                        }
                        CompileResult::Ok(ref doc) => {
                            let doc = doc.clone();
                            preview.update(cx, |pane, cx| pane.set_document(doc, cx));
                        }
                        CompileResult::Err(ref diags) => {
                            let msg = diags.first()
                                .map(|d| d.message.clone())
                                .unwrap_or_else(|| "Unknown error".to_string());
                            this.update(cx, |win, _cx| {
                                if let Some(ref wv) = win.html_webview {
                                    wv.load_error(&msg);
                                }
                            }).ok();
                            preview.update(cx, |pane, cx| pane.set_error(msg, cx));
                        }
                        CompileResult::Panicked(ref msg) => {
                            let msg = format!("Compiler panicked: {msg}");
                            this.update(cx, |win, _cx| {
                                if let Some(ref wv) = win.html_webview {
                                    wv.load_error(&msg);
                                }
                            }).ok();
                            preview.update(cx, |pane, cx| pane.set_error(msg, cx));
                        }
                    }
                }).ok();
            }
        }).detach();

        // ── Sidebar → active editor ───────────────────────────────────────────
        cx.subscribe(&sidebar, |this, _, event: &SidebarEvent, cx| {
            match event {
                SidebarEvent::OpenFile(abs_path) => {
                    this.open_path(abs_path.clone(), cx);
                }
            }
        }).detach();

        // ── Initial editor event subscription ────────────────────────────────
        Self::subscribe_pane(cx, &editor);

        let panes = vec![PaneEntry { editor }];

        Self {
            focus_handle: cx.focus_handle(),
            sidebar,
            panes,
            active_idx: 0,
            split_layout: SplitLayout::Single,
            pane_split_frac: 0.5,
            compiler_handle,
            preview,
            html_webview: None,
            vault,
            sidebar_visible: true,
            sidebar_width: 220.0,
            preview_width: 420.0,
            drag: None,
            palette: None,
            palette_focus_pending: false,
            quick_switch: None,
            template_picker: None,
            backlinks: None,
            vault_search: None,
            graph_view: None,
            recent_paths: Vec::new(),
        }
    }

    // ── Pane management ───────────────────────────────────────────────────────

    /// Subscribe to events from an editor pane.
    fn subscribe_pane(cx: &mut Context<Self>, editor: &Entity<EditorPane>) {
        cx.subscribe(editor, |this, _, event: &EditorPaneEvent, cx| {
            match event {
                EditorPaneEvent::OpenFile(path) => {
                    this.open_path(path.clone(), cx);
                }
                EditorPaneEvent::OpenPalette => {
                    // Create palette without focusing yet; render() will
                    // focus it on the next pass (needs &mut Window).
                    if this.palette.is_some() {
                        // Already open — toggle off.
                        this.palette = None;
                        cx.notify();
                        return;
                    }
                    let palette = cx.new(|cx| CommandPalette::new(cx));
                    cx.subscribe(&palette, |this, _, event: &PaletteEvent, cx| {
                        match event {
                            PaletteEvent::Close => {
                                this.palette = None;
                                cx.notify();
                            }
                            PaletteEvent::Execute(id) => {
                                this.palette = None;
                                cx.notify();
                                this.handle_palette_execute(id, cx);
                            }
                        }
                    }).detach();
                    this.palette = Some(palette);
                    this.palette_focus_pending = true;
                    cx.notify();
                }
            }
        }).detach();
    }

    /// Spawn a new pane, wire compiler + vault, subscribe events. Returns entity.
    fn new_pane(&mut self, cx: &mut Context<Self>) -> Entity<EditorPane> {
        let editor = cx.new(|cx| EditorPane::new(cx));
        editor.update(cx, |pane, _cx| pane.set_vault(self.vault.clone()));
        editor.update(cx, |pane, _cx| {
            pane.set_compiler(self.compiler_handle.clone(), self.preview.clone());
        });
        Self::subscribe_pane(cx, &editor);
        editor
    }

    /// Returns the active editor entity.
    fn active_editor(&self) -> &Entity<EditorPane> {
        &self.panes[self.active_idx].editor
    }

    /// Focus a pane by index, triggering a recompile so the preview updates.
    fn focus_pane(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        if idx >= self.panes.len() { return; }
        self.active_idx = idx;
        let editor = self.panes[idx].editor.clone();
        editor.read(cx).focus_handle(cx).focus(window);
        editor.update(cx, |pane, cx| pane.trigger_compile(cx));
        cx.notify();
    }

    // ── Split actions ─────────────────────────────────────────────────────────

    fn split_pane_vertical(
        &mut self,
        _: &SplitPaneVertical,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.split_layout != SplitLayout::Single {
            // Already split — just focus the other pane.
            let other = 1 - self.active_idx.min(1);
            self.focus_pane(other, window, cx);
            return;
        }
        let new_editor = self.new_pane(cx);
        // Open the same file as the active pane in the new pane.
        self.copy_file_to_new_pane(&new_editor, cx);
        self.panes.push(PaneEntry { editor: new_editor });
        self.split_layout = SplitLayout::Vertical;
        self.pane_split_frac = 0.5;
        let new_idx = self.panes.len() - 1;
        self.focus_pane(new_idx, window, cx);
    }

    fn split_pane_horizontal(
        &mut self,
        _: &SplitPaneHorizontal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.split_layout != SplitLayout::Single {
            let other = 1 - self.active_idx.min(1);
            self.focus_pane(other, window, cx);
            return;
        }
        let new_editor = self.new_pane(cx);
        self.copy_file_to_new_pane(&new_editor, cx);
        self.panes.push(PaneEntry { editor: new_editor });
        self.split_layout = SplitLayout::Horizontal;
        self.pane_split_frac = 0.5;
        let new_idx = self.panes.len() - 1;
        self.focus_pane(new_idx, window, cx);
    }

    /// Open the active pane's current file (if any) in a freshly created pane.
    fn copy_file_to_new_pane(&self, target: &Entity<EditorPane>, cx: &mut Context<Self>) {
        let active = self.panes[self.active_idx].editor.read(cx);
        let rel_path = active.current_rel_path().map(|s| s.to_string());
        let vault_root = self.vault.read(cx).root.clone();
        drop(active);

        if let (Some(rel), Some(root)) = (rel_path, vault_root) {
            let abs = root.join(&rel);
            let vault_files = self.vault.read(cx).files.clone();
            if let Some(file) = vault_files.iter().find(|f| f.abs_path == abs).cloned() {
                target.update(cx, |pane, cx| {
                    pane.open_file_no_focus(&file, root, cx);
                });
            }
        }
    }

    fn close_pane(
        &mut self,
        _: &ClosePane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.panes.len() <= 1 {
            // Never close the last pane.
            return;
        }
        self.panes.remove(self.active_idx);
        self.active_idx = self.active_idx.saturating_sub(1).min(self.panes.len() - 1);
        self.split_layout = SplitLayout::Single;
        self.focus_pane(self.active_idx, window, cx);
    }

    // ── Focus navigation ──────────────────────────────────────────────────────

    fn focus_pane_left(&mut self, _: &FocusPaneLeft, window: &mut Window, cx: &mut Context<Self>) {
        if self.split_layout == SplitLayout::Vertical && self.active_idx == 1 {
            self.focus_pane(0, window, cx);
        }
    }

    fn focus_pane_right(&mut self, _: &FocusPaneRight, window: &mut Window, cx: &mut Context<Self>) {
        if self.split_layout == SplitLayout::Vertical && self.active_idx == 0 {
            self.focus_pane(1, window, cx);
        }
    }

    fn focus_pane_up(&mut self, _: &FocusPaneUp, window: &mut Window, cx: &mut Context<Self>) {
        if self.split_layout == SplitLayout::Horizontal && self.active_idx == 1 {
            self.focus_pane(0, window, cx);
        }
    }

    fn focus_pane_down(&mut self, _: &FocusPaneDown, window: &mut Window, cx: &mut Context<Self>) {
        if self.split_layout == SplitLayout::Horizontal && self.active_idx == 0 {
            self.focus_pane(1, window, cx);
        }
    }

    // ── Other action handlers ─────────────────────────────────────────────────

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_visible = !self.sidebar_visible;
        cx.notify();
    }

    fn toggle_preview_mode(
        &mut self,
        _: &TogglePreviewMode,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = cx.try_global::<PreviewMode>().copied().unwrap_or_default();
        let next = match current {
            PreviewMode::Html => PreviewMode::Paged,
            PreviewMode::Paged => PreviewMode::Html,
        };
        cx.set_global(next);
        match next {
            PreviewMode::Html => {
                if let Some(ref wv) = self.html_webview { wv.set_hidden(false); }
            }
            PreviewMode::Paged => {
                if let Some(ref wv) = self.html_webview { wv.set_hidden(true); }
            }
        }
        self.active_editor().clone().update(cx, |pane, cx| pane.trigger_compile(cx));
        cx.notify();
    }

    fn open_daily_note(
        &mut self,
        _: &OpenDailyNote,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.vault.read(cx).root.clone() else { return };

        let today = time::OffsetDateTime::now_local()
            .unwrap_or_else(|_| time::OffsetDateTime::now_utc());
        let date_str = format!(
            "{:04}-{:02}-{:02}",
            today.year(), today.month() as u8, today.day()
        );

        let daily_dir = root.join(".ockr").join("daily");
        let _ = std::fs::create_dir_all(&daily_dir);
        let note_path = daily_dir.join(format!("{date_str}.typ"));

        if !note_path.exists() {
            let _ = std::fs::write(&note_path, minimal_daily_template(&date_str));
        }

        self.vault.update(cx, |vs, _cx| {
            *vs = crate::vault::VaultState::open(root.clone());
        });

        let rel = PathBuf::from(".ockr/daily").join(format!("{date_str}.typ"));
        let vault_files = self.vault.read(cx).files.clone();
        if let Some(file) = vault_files.iter().find(|f| f.rel_path == rel).cloned() {
            self.active_editor().clone().update(cx, |pane, cx| {
                pane.open_file(&file, root, window, cx);
            });
            self.recent_paths.retain(|p| p != &note_path);
            self.recent_paths.insert(0, note_path);
            self.recent_paths.truncate(20);
            cx.notify();
        }
    }

    fn open_path(&mut self, abs_path: PathBuf, cx: &mut Context<Self>) {
        open_file_in_editor(&abs_path, self.active_editor(), &self.vault, cx);
        self.recent_paths.retain(|p| p != &abs_path);
        self.recent_paths.insert(0, abs_path);
        self.recent_paths.truncate(20);
    }

    // ── Story 18: Note Templates ──────────────────────────────────────────────

    fn new_note(
        &mut self,
        _: &NewNote,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.vault.read(cx).root.clone() else { return };

        let templates = scan_templates(&root);
        if templates.is_empty() {
            // No templates available — create blank note immediately.
            self.create_new_note(None, root, window, cx);
            return;
        }

        // Show template picker.
        if self.template_picker.is_some() {
            self.template_picker = None;
            cx.notify();
            return;
        }

        let picker = cx.new(|cx| TemplatePicker::new(templates, cx));
        picker.read(cx).focus_handle.clone().focus(window);

        cx.subscribe(&picker, move |this, _, event: &TemplatePickerEvent, cx| {
            let root = match this.vault.read(cx).root.clone() {
                Some(r) => r,
                None => {
                    this.template_picker = None;
                    cx.notify();
                    return;
                }
            };
            match event {
                TemplatePickerEvent::Close => {
                    this.template_picker = None;
                    cx.notify();
                }
                TemplatePickerEvent::Pick(path) => {
                    this.template_picker = None;
                    cx.notify();
                    let template_path = path.clone();
                    // We need a Window ref — defer into the next frame.
                    // Use open_new_note_from_template which doesn't need Window.
                    this.create_new_note_deferred(template_path, root, cx);
                }
            }
        }).detach();

        self.template_picker = Some(picker);
        cx.notify();
    }

    /// Create a new note without needing a `Window` reference (called from
    /// subscription callback where `Window` is unavailable).
    fn create_new_note_deferred(
        &mut self,
        template_path: Option<PathBuf>,
        vault_root: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let content = match &template_path {
            Some(path) => std::fs::read_to_string(path).unwrap_or_default(),
            None => String::new(),
        };

        let filename = {
            let today = time::OffsetDateTime::now_local()
                .unwrap_or_else(|_| time::OffsetDateTime::now_utc());
            let date_str = format!(
                "{:04}-{:02}-{:02}",
                today.year(), today.month() as u8, today.day()
            );
            match heading_to_filename_stem(&content) {
                Some(stem) => format!("{stem}.typ"),
                None => format!("untitled-{date_str}.typ"),
            }
        };

        // Avoid clobbering an existing file by appending a counter.
        let note_path = unique_path(&vault_root, &filename);
        let _ = std::fs::write(&note_path, &content);

        // Rescan vault and open the new note.
        self.vault.update(cx, |vs, _cx| {
            *vs = crate::vault::VaultState::open(vault_root.clone());
        });

        let abs_path = note_path.clone();
        let vault_files = self.vault.read(cx).files.clone();
        if let Some(file) = vault_files.iter().find(|f| f.abs_path == abs_path).cloned() {
            let editor = self.active_editor().clone();
            editor.update(cx, |pane, cx| {
                pane.open_file_no_focus(&file, vault_root, cx);
            });
            self.recent_paths.retain(|p| p != &abs_path);
            self.recent_paths.insert(0, abs_path);
            self.recent_paths.truncate(20);
            cx.notify();
        }
    }

    /// Create a new note when a `Window` is available (direct call path).
    fn create_new_note(
        &mut self,
        template_path: Option<PathBuf>,
        vault_root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let content = match &template_path {
            Some(path) => std::fs::read_to_string(path).unwrap_or_default(),
            None => String::new(),
        };

        let filename = {
            let today = time::OffsetDateTime::now_local()
                .unwrap_or_else(|_| time::OffsetDateTime::now_utc());
            let date_str = format!(
                "{:04}-{:02}-{:02}",
                today.year(), today.month() as u8, today.day()
            );
            match heading_to_filename_stem(&content) {
                Some(stem) => format!("{stem}.typ"),
                None => format!("untitled-{date_str}.typ"),
            }
        };

        let note_path = unique_path(&vault_root, &filename);
        let _ = std::fs::write(&note_path, &content);

        self.vault.update(cx, |vs, _cx| {
            *vs = crate::vault::VaultState::open(vault_root.clone());
        });

        let abs_path = note_path.clone();
        let vault_files = self.vault.read(cx).files.clone();
        if let Some(file) = vault_files.iter().find(|f| f.abs_path == abs_path).cloned() {
            let editor = self.active_editor().clone();
            editor.update(cx, |pane, cx| {
                pane.open_file(&file, vault_root, window, cx);
            });
            self.recent_paths.retain(|p| p != &abs_path);
            self.recent_paths.insert(0, abs_path);
            self.recent_paths.truncate(20);
            cx.notify();
        }
    }

    // ── Story 19: Graph View ──────────────────────────────────────────────────

    fn open_graph_view(
        &mut self,
        _: &OpenGraphView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.graph_view.is_some() {
            self.graph_view = None;
            cx.notify();
            return;
        }

        let (files, backlinks) = {
            let vault = self.vault.read(cx);
            (vault.files.clone(), vault.backlinks.clone())
        };
        let view = cx.new(|cx| GraphView::new(files, &backlinks, cx));
        view.read(cx).focus_handle.clone().focus(window);

        cx.subscribe(&view, |this, _, event: &GraphViewEvent, cx| {
            match event {
                GraphViewEvent::Close => {
                    this.graph_view = None;
                    cx.notify();
                }
                GraphViewEvent::Open(path) => {
                    this.graph_view = None;
                    cx.notify();
                    this.open_path(path.clone(), cx);
                }
            }
        }).detach();

        self.graph_view = Some(view);
        cx.notify();
    }

    fn open_quick_switch(
        &mut self,
        _: &OpenQuickSwitch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.quick_switch.is_some() {
            self.quick_switch = None;
            cx.notify();
            return;
        }

        let all_files = self.vault.read(cx).files.clone();
        let mut ordered: Vec<_> = self.recent_paths.iter()
            .filter_map(|p| all_files.iter().find(|f| &f.abs_path == p).cloned())
            .collect();
        for f in &all_files {
            if !self.recent_paths.contains(&f.abs_path) {
                ordered.push(f.clone());
            }
        }

        let qs = cx.new(|cx| QuickSwitch::new(ordered, cx));
        qs.read(cx).focus_handle.clone().focus(window);

        cx.subscribe(&qs, |this, _, event: &QuickSwitchEvent, cx| {
            match event {
                QuickSwitchEvent::Close => {
                    this.quick_switch = None;
                    cx.notify();
                }
                QuickSwitchEvent::Open(path) => {
                    this.quick_switch = None;
                    cx.notify();
                    this.open_path(path.clone(), cx);
                }
            }
        }).detach();

        self.quick_switch = Some(qs);
        cx.notify();
    }

    fn open_backlinks(
        &mut self,
        _: &OpenBacklinks,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.backlinks.is_some() {
            self.backlinks = None;
            cx.notify();
            return;
        }

        let (current_title, incoming) = {
            let pane = self.active_editor().read(cx);
            let rel_path = pane.current_rel_path().unwrap_or("").to_string();
            let title = std::path::Path::new(&rel_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(&rel_path)
                .to_string();
            let vault = self.vault.read(cx);
            let links = if rel_path.is_empty() { vec![] } else {
                vault.backlinks.incoming_links(std::path::Path::new(&rel_path))
            };
            (title, links)
        };

        let panel = cx.new(|cx| BacklinkPanel::new(current_title, incoming, cx));
        panel.read(cx).focus_handle.clone().focus(window);

        cx.subscribe(&panel, |this, _, event: &BacklinkPanelEvent, cx| {
            match event {
                BacklinkPanelEvent::Close => {
                    this.backlinks = None;
                    cx.notify();
                }
                BacklinkPanelEvent::Open(path) => {
                    this.backlinks = None;
                    cx.notify();
                    this.open_path(path.clone(), cx);
                }
            }
        }).detach();

        self.backlinks = Some(panel);
        cx.notify();
    }

    fn open_vault_search(
        &mut self,
        _: &OpenVaultSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.vault_search.is_some() {
            self.vault_search = None;
            cx.notify();
            return;
        }

        let files = self.vault.read(cx).files.clone();
        let panel = cx.new(|cx| VaultSearch::new(files, cx));
        panel.read(cx).focus_handle.clone().focus(window);

        cx.subscribe(&panel, |this, _, event: &VaultSearchEvent, cx| {
            match event {
                VaultSearchEvent::Close => {
                    this.vault_search = None;
                    cx.notify();
                }
                VaultSearchEvent::Open(path, line_no) => {
                    this.vault_search = None;
                    cx.notify();
                    this.open_path(path.clone(), cx);
                    let line = *line_no;
                    this.active_editor().clone().update(cx, |pane, cx| {
                        pane.jump_to_line(line, cx);
                    });
                }
            }
        }).detach();

        self.vault_search = Some(panel);
        cx.notify();
    }

    /// Shared: execute a palette command ID dispatched by the user.
    fn handle_palette_execute(&mut self, id: &'static str, cx: &mut Context<Self>) {
        match id {
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
            "toggle-sidebar" => cx.dispatch_action(&ToggleSidebar),
            "open-command-palette" => cx.dispatch_action(&OpenCommandPalette),
            "vault-search" => cx.dispatch_action(&OpenVaultSearch),
            "open-daily-note" => cx.dispatch_action(&OpenDailyNote),
            "split-pane-vertical" => cx.dispatch_action(&SplitPaneVertical),
            "split-pane-horizontal" => cx.dispatch_action(&SplitPaneHorizontal),
            "close-pane" => cx.dispatch_action(&ClosePane),
            "open-graph-view" => cx.dispatch_action(&OpenGraphView),
            "line-numbers-relative" => cx.dispatch_action(&LineNumbersRelative),
            "line-numbers-absolute" => cx.dispatch_action(&LineNumbersAbsolute),
            "line-numbers-off"      => cx.dispatch_action(&LineNumbersOff),
            _ => {}
        }
    }

    fn open_palette(
        &mut self,
        _: &OpenCommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.palette.is_some() {
            self.palette = None;
            cx.notify();
            return;
        }

        let palette = cx.new(|cx| CommandPalette::new(cx));
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
                    this.handle_palette_execute(id, cx);
                }
            }
        }).detach();

        self.palette = Some(palette);
        cx.notify();
    }

    // ── Drag handles ──────────────────────────────────────────────────────────

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
            DragTarget::PaneDivider => self.pane_split_frac * 1000.0, // sentinel
        };
        self.drag = Some(DragState {
            target,
            start_x: f32::from(event.position.x),
            start_y: f32::from(event.position.y),
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
        let dy = f32::from(event.position.y) - drag.start_y;
        match drag.target {
            DragTarget::Sidebar => {
                self.sidebar_width = (drag.start_width + dx).clamp(120.0, 480.0);
            }
            DragTarget::Preview => {
                self.preview_width = (drag.start_width - dx).clamp(200.0, 900.0);
            }
            DragTarget::PaneDivider => {
                // We encode the editor-area width at drag-start into start_width.
                // Here we approximate by using the delta and a stored editor area width.
                // The stored value is 1000 * frac; we adjust frac by dx / editor_area_w.
                // Use start_y field (repurposed) to store the editor area width.
                let editor_area_w = drag.start_y.max(100.0); // stored in start_y
                let delta_frac = dx / editor_area_w;
                let base_frac = drag.start_width / 1000.0;
                self.pane_split_frac = (base_frac + delta_frac).clamp(0.2, 0.8);
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

/// Return a path that doesn't already exist on disk.
/// If `root/name` exists, tries `root/name-2.typ`, `root/name-3.typ`, etc.
fn unique_path(root: &std::path::Path, filename: &str) -> PathBuf {
    let base = root.join(filename);
    if !base.exists() {
        return base;
    }
    let stem = std::path::Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled");
    for n in 2..=999 {
        let candidate = root.join(format!("{stem}-{n}.typ"));
        if !candidate.exists() {
            return candidate;
        }
    }
    base
}

fn minimal_daily_template(date: &str) -> String {
    format!("= {date}\n\n// Daily note — {date}\n\n")
}

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
    let rel_path = abs_path.strip_prefix(&vault_root).unwrap_or(abs_path).to_path_buf();
    let title = abs_path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_string();
    let file = crate::vault::VaultFile { rel_path, abs_path: abs_path.clone(), title };
    editor.update(cx, |pane, cx| pane.open_file_no_focus(&file, vault_root, cx));
}

// ── Focusable ─────────────────────────────────────────────────────────────────

impl Focusable for MainWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.global::<ThemePalette>().clone();
        let preview_mode = cx.try_global::<PreviewMode>().copied().unwrap_or_default();

        // Focus the palette if it was just created via the editor-pane event path
        // (subscriptions don't receive &mut Window, so we defer the focus to render).
        if self.palette_focus_pending {
            if let Some(ref p) = self.palette {
                p.read(cx).focus_handle.clone().focus(window);
            }
            self.palette_focus_pending = false;
        }

        // ── Window dimensions ─────────────────────────────────────────────────
        // Use GPUI's viewport_size — always current, never stale, no AppKit query needed.
        let vp = window.viewport_size();
        let content_w = f64::from(vp.width);
        let content_h = f64::from(vp.height);

        // ── WKWebView lifecycle ───────────────────────────────────────────────
        let preview_x = content_w - self.preview_width as f64;
        match preview_mode {
            PreviewMode::Html => {
                if self.html_webview.is_none() {
                    self.html_webview = HtmlWebView::new();
                }
                if let Some(ref wv) = self.html_webview {
                    wv.update_frame(preview_x, 0.0, self.preview_width as f64, content_h);
                    wv.set_hidden(false);
                }
            }
            PreviewMode::Paged => {
                if let Some(ref wv) = self.html_webview { wv.set_hidden(true); }
            }
        }

        // ── Handle factories ──────────────────────────────────────────────────
        let border_subtle = t.border_subtle;
        let ochre_dim = t.ochre_dim;

        let handle = |target: DragTarget, vertical: bool, cx: &mut Context<Self>| {
            let d = div()
                .bg(gpui::rgb(border_subtle))
                .hover(move |s| s.bg(gpui::rgb(ochre_dim)))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, event, window, cx| {
                    this.on_handle_mouse_down(target, event, window, cx);
                }));
            if vertical {
                d.w(px(4.0)).h_full().cursor_ew_resize()
            } else {
                d.h(px(4.0)).w_full().cursor_ns_resize()
            }
        };

        // ── Root ──────────────────────────────────────────────────────────────
        let mut root = div()
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_row()
            .bg(gpui::rgb(t.bg_surface))
            .on_action(cx.listener(Self::new_note))
            .on_action(cx.listener(Self::open_graph_view))
            .on_action(cx.listener(Self::open_palette))
            .on_action(cx.listener(Self::open_quick_switch))
            .on_action(cx.listener(Self::open_backlinks))
            .on_action(cx.listener(Self::open_vault_search))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::toggle_preview_mode))
            .on_action(cx.listener(Self::open_daily_note))
            .on_action(cx.listener(Self::split_pane_vertical))
            .on_action(cx.listener(Self::split_pane_horizontal))
            .on_action(cx.listener(Self::close_pane))
            .on_action(cx.listener(Self::focus_pane_left))
            .on_action(cx.listener(Self::focus_pane_right))
            .on_action(cx.listener(Self::focus_pane_up))
            .on_action(cx.listener(Self::focus_pane_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up));

        // ── Sidebar ───────────────────────────────────────────────────────────
        if self.sidebar_visible {
            root = root
                .child(div().w(px(self.sidebar_width)).h_full().overflow_hidden()
                    .child(self.sidebar.clone()))
                .child(handle(DragTarget::Sidebar, true, cx));
        }

        // ── Editor area ───────────────────────────────────────────────────────
        //
        // Sidebar width + 4 handle + [editor area] + 4 handle + preview width.
        // Editor area = flex_1 so it fills whatever remains.
        //
        // In split mode, the editor area is further divided into two sub-panes
        // separated by a PaneDivider handle.

        let sidebar_w = if self.sidebar_visible { self.sidebar_width + 4.0 } else { 0.0 };
        let editor_area_w = (content_w as f32 - sidebar_w - self.preview_width - 4.0).max(200.0);

        let pane0 = self.panes[0].editor.clone();
        let pane1 = self.panes.get(1).map(|p| p.editor.clone());

        let editor_area = match (self.split_layout, &pane1) {
            (SplitLayout::Vertical, Some(p1)) => {
                let w0 = (editor_area_w * self.pane_split_frac).round();
                let w1 = editor_area_w - w0 - 4.0; // minus divider handle

                // Store editor_area_w for the drag handler (encoded in start_y).
                // We do this by mutating the drag if PaneDivider is active.
                if let Some(ref mut drag) = self.drag {
                    if drag.target == DragTarget::PaneDivider {
                        drag.start_y = editor_area_w;
                    }
                }

                div().flex_1().min_w_0().h_full().flex().flex_row()
                    .child(div().w(px(w0)).h_full().overflow_hidden().child(pane0))
                    .child(handle(DragTarget::PaneDivider, true, cx))
                    .child(div().w(px(w1)).h_full().overflow_hidden().child(p1.clone()))
                    .into_any_element()
            }
            (SplitLayout::Horizontal, Some(p1)) => {
                let h0 = (content_h as f32 * self.pane_split_frac).round();
                let h1 = content_h as f32 - h0 - 4.0;

                div().flex_1().min_w_0().h_full().flex().flex_col()
                    .child(div().w_full().h(px(h0)).overflow_hidden().child(pane0))
                    .child(handle(DragTarget::PaneDivider, false, cx))
                    .child(div().w_full().h(px(h1)).overflow_hidden().child(p1.clone()))
                    .into_any_element()
            }
            _ => {
                div().flex_1().min_w_0().h_full().overflow_hidden()
                    .child(pane0)
                    .into_any_element()
            }
        };

        // ── Preview column ────────────────────────────────────────────────────
        let preview_col = match preview_mode {
            PreviewMode::Paged => div()
                .w(px(self.preview_width)).h_full().overflow_hidden()
                .child(self.preview.clone())
                .into_any_element(),
            PreviewMode::Html => div()
                .w(px(self.preview_width)).h_full()
                .bg(gpui::rgb(t.bg_panel))
                .into_any_element(),
        };

        root.child(editor_area)
            .child(handle(DragTarget::Preview, true, cx))
            .child(preview_col)
            .when_some(self.palette.clone(), |root, p| {
                root.child(gpui::deferred(p).with_priority(100))
            })
            .when_some(self.quick_switch.clone(), |root, qs| {
                root.child(gpui::deferred(qs).with_priority(100))
            })
            .when_some(self.template_picker.clone(), |root, picker| {
                root.child(gpui::deferred(picker).with_priority(100))
            })
            .when_some(self.backlinks.clone(), |root, panel| {
                root.child(gpui::deferred(panel).with_priority(100))
            })
            .when_some(self.vault_search.clone(), |root, panel| {
                root.child(gpui::deferred(panel).with_priority(100))
            })
            .when_some(self.graph_view.clone(), |root, gv| {
                root.child(gpui::deferred(gv).with_priority(200))
            })
    }
}
