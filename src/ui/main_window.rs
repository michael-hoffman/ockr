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

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use futures::StreamExt as _;
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Render, Window, div, prelude::*, px,
};

use crate::actions::{
    BufferClose, BufferNext, BufferPrevious, ClosePane, ExportPdf, FocusPaneDown, FocusPaneLeft,
    FocusPaneRight, FocusPaneUp, ForceQuit, LineNumbersAbsolute, LineNumbersOff,
    LineNumbersRelative, NewNote, OpenBacklinks, OpenCommandPalette, OpenDailyNote, OpenGraphView,
    OpenPluginManager, OpenQuickSwitch, OpenReplace, OpenSearch, OpenVault, OpenVaultSearch, Quit, ReloadFile, SaveFile,
    SaveFileAndQuit, SplitPaneHorizontal, SplitPaneVertical, TogglePreviewMode, ToggleSidebar,
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
use crate::editor::state::Pos;
use crate::ui::editor_pane::{EditorPane, EditorPaneEvent};
use crate::ui::preview::{PreviewEvent, PreviewPane};
use crate::ui::sidebar::{Sidebar, SidebarEvent};
use crate::command::CommandEntry;
use crate::plugin::loader::load_vault_plugins;
use crate::plugin::registry::{PluginInfo, PluginRegistry, PluginStatus};
use crate::plugin::runtime::{PluginEvent, PluginInstance, PluginMetadataJson};
use crate::plugin::thread_pool::{PluginJob, PluginThreadPool};
use crate::ui::plugin_panel::{PluginPanel, PluginPanelEvent};
use crate::ui::plugin_manager::{PluginManager, PluginManagerEvent};
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

// ── Tab ───────────────────────────────────────────────────────────────────────

/// Saved state for one open file tab within a pane.
///
/// When the user switches away from a tab, its cursor position and viewport
/// are persisted here so they are restored on return.
#[derive(Clone)]
struct TabInfo {
    path: PathBuf,
    /// File stem shown in the tab label (e.g. `"my-note"`).
    title: String,
    /// Cursor position saved when leaving this tab.
    cursor: Pos,
    /// Viewport top (first visible line) saved when leaving this tab.
    viewport_top: usize,
}

// ── Pane entry ────────────────────────────────────────────────────────────────

/// One slot in the editor area: a single `EditorPane` entity plus the ordered
/// list of tabs it currently holds.
struct PaneEntry {
    editor: Entity<EditorPane>,
    /// Open tabs.  Always at least one entry while a file is loaded.
    tabs: Vec<TabInfo>,
    /// Index of the currently displayed tab.
    active_tab: usize,
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
    /// Sender half of the wikilink-click channel.  Cloned into `HtmlWebView`
    /// on creation so the JS message handler can forward clicked `ockr://`
    /// paths back to the async task in `new()`.
    html_link_sender: futures::channel::mpsc::UnboundedSender<String>,
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
    /// Last successfully compiled paged document (used for PDF export).
    last_paged_doc: Option<std::sync::Arc<typst::layout::PagedDocument>>,
    /// Transient status message shown after export (cleared on next notify cycle).
    export_status: Option<String>,
    /// Shared map of `"@plugin/<name>/lib.typ"` → source, passed to compiler.
    plugin_packages: Arc<RwLock<HashMap<String, String>>>,
    /// Live WASM plugin instances keyed by plugin_id.
    plugin_instances: HashMap<String, Arc<Mutex<PluginInstance>>>,
    /// Background thread pool for dispatching plugin commands.
    plugin_pool: PluginThreadPool,
    /// Reused Wasmtime engine (creating one per instantiation is expensive).
    wasmtime_engine: wasmtime::Engine,
    /// Currently visible plugin panel overlay, if any.
    plugin_panel: Option<gpui::Entity<PluginPanel>>,
    /// True when `plugin_panel` was just (re)created and needs focus on next render.
    plugin_panel_focus_pending: bool,
    /// Currently visible plugin manager overlay, if any.
    plugin_manager: Option<gpui::Entity<PluginManager>>,
    /// True when `plugin_manager` was just created and needs focus on next render.
    plugin_manager_focus_pending: bool,
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

        // ── Wikilink click channel (HTML preview → open_path) ────────────────
        // OckrLinkHandler posts vault-relative paths here when the user
        // clicks an ockr:// link in the HTML preview.  We keep the sender so
        // we can pass it to HtmlWebView when it is created lazily on first
        // render (stored in html_link_sender below).
        let (link_tx, mut link_rx) =
            futures::channel::mpsc::unbounded::<String>();

        // Spawn an async task to drain the channel and open clicked links.
        {
            let vault_for_links = vault.clone();
            cx.spawn(async move |this, cx| {
                while let Some(rel_path) = link_rx.next().await {
                    let path = cx.update(|cx| {
                        let root = vault_for_links.read(cx).root.clone()?;
                        Some(root.join(&rel_path))
                    }).ok().flatten();
                    if let Some(abs_path) = path {
                        cx.update(|cx| {
                            this.update(cx, |win, cx| win.open_path(abs_path, cx)).ok();
                        }).ok();
                    }
                }
            }).detach();
        }

        // ── Plugin system ─────────────────────────────────────────────────────
        let plugin_packages: Arc<RwLock<HashMap<String, String>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let plugin_pool = PluginThreadPool::new(4);
        let wasmtime_engine = wasmtime::Engine::default();

        // Install the plugin registry as a GPUI global.
        cx.set_global(PluginRegistry::new());

        // Load plugins already installed in the vault (probe caps, then real instance).
        let (plugin_instances, plugin_info) = Self::instantiate_vault_plugins(
            &wasmtime_engine,
            vault.read(cx).root.as_deref(),
            cx.global::<PluginRegistry>().event_tx.clone(),
            Arc::clone(&plugin_packages),
        );
        // Persist metadata and status in the registry for the plugin manager.
        {
            let reg = cx.global_mut::<PluginRegistry>();
            for (meta, status) in plugin_info {
                reg.mark_loaded(PluginInfo::from(&meta));
                reg.plugin_statuses.insert(meta.id, status);
            }
        }

        // Spawn a 100ms polling task to drain plugin events onto the UI thread.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let _ = cx.update(|cx| {
                    let _ = this.update(cx, |win, cx| win.drain_plugin_events(cx));
                });
            }
        })
        .detach();

        // ── Initial pane ─────────────────────────────────────────────────────
        let editor = cx.new(|cx| EditorPane::new(cx));
        editor.update(cx, |pane, _cx| pane.set_vault(vault.clone()));
        editor.update(cx, |pane, _cx| {
            pane.set_compiler(compiler_handle.clone(), preview.clone());
        });
        editor.update(cx, |pane, _cx| {
            pane.set_plugin_packages(Arc::clone(&plugin_packages));
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
                            this.update(cx, |win, cx| {
                                // Clear diagnostics in the active editor pane on success.
                                if let Some(pane) = win.panes.get(win.active_idx) {
                                    pane.editor.update(cx, |ep, _cx| ep.clear_diagnostics());
                                }
                                if let Some(ref wv) = win.html_webview {
                                    wv.load_html(&html);
                                }
                            }).ok();
                        }
                        CompileResult::Ok(ref doc) => {
                            let doc = doc.clone();
                            this.update(cx, |win, cx| {
                                // Clear diagnostics in the active editor pane on success.
                                if let Some(pane) = win.panes.get(win.active_idx) {
                                    pane.editor.update(cx, |ep, _cx| ep.clear_diagnostics());
                                }
                                win.last_paged_doc = Some(doc.clone());
                            }).ok();
                            preview.update(cx, |pane, cx| pane.set_document(doc, cx));
                        }
                        CompileResult::Err(ref diags) => {
                            let first_msg = diags.first()
                                .map(|d| d.message.clone())
                                .unwrap_or_else(|| "Unknown error".to_string());
                            let diags = diags.clone();
                            this.update(cx, |win, cx| {
                                // Forward diagnostics to the active editor pane.
                                if let Some(pane) = win.panes.get(win.active_idx) {
                                    pane.editor.update(cx, |ep, _cx| ep.set_diagnostics(diags.clone()));
                                }
                                if let Some(ref wv) = win.html_webview {
                                    wv.load_error(&first_msg);
                                }
                            }).ok();
                            preview.update(cx, |pane, cx| pane.set_diagnostics(diags, cx));
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

        // ── Paged preview → open wikilink ─────────────────────────────────────
        // When the user clicks an ockr:// link in the rasterised preview,
        // resolve the vault-relative path and open it in the active editor.
        cx.subscribe(&preview, {
            let vault = vault.clone();
            move |this, _, event: &PreviewEvent, cx| {
                let PreviewEvent::OpenLink(url) = event;
                let rel_path = url.strip_prefix("ockr://").unwrap_or(url.as_str());
                if let Some(root) = vault.read(cx).root.clone() {
                    let abs_path = root.join(rel_path);
                    this.open_path(abs_path, cx);
                }
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

        // ── Vault change → reload plugins ─────────────────────────────────────
        cx.observe(&vault, |this, vault_entity, cx| {
            let new_root = vault_entity.read(cx).root.clone();
            let old_root = this.vault.read(cx).root.clone();
            if new_root != old_root {
                this.reload_vault_plugins(cx);
            }
        }).detach();

        // ── Initial editor event subscription ────────────────────────────────
        Self::subscribe_pane(cx, &editor);

        let panes = vec![PaneEntry { editor, tabs: Vec::new(), active_tab: 0 }];

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
            last_paged_doc: None,
            export_status: None,
            plugin_packages,
            plugin_instances,
            plugin_pool,
            wasmtime_engine,
            plugin_panel: None,
            plugin_panel_focus_pending: false,
            plugin_manager: None,
            plugin_manager_focus_pending: false,
            html_link_sender: link_tx,
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
        // Mirror the active pane's tabs into the new pane entry.
        let tabs = self.panes[self.active_idx].tabs.clone();
        let active_tab = self.panes[self.active_idx].active_tab;
        self.panes.push(PaneEntry { editor: new_editor, tabs, active_tab });
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
        let tabs = self.panes[self.active_idx].tabs.clone();
        let active_tab = self.panes[self.active_idx].active_tab;
        self.panes.push(PaneEntry { editor: new_editor, tabs, active_tab });
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
        let _ = active;

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

    fn export_pdf(
        &mut self,
        _: &ExportPdf,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.last_paged_doc.clone() else {
            self.export_status = Some("No compiled document to export".to_string());
            cx.notify();
            return;
        };

        // Determine output path: same stem as source file, but with .pdf extension.
        let source_path = self.panes[self.active_idx]
            .tabs
            .get(self.panes[self.active_idx].active_tab)
            .map(|t| t.path.clone());

        let pdf_path = match source_path {
            Some(p) => p.with_extension("pdf"),
            None => {
                self.export_status = Some("No file path — save first".to_string());
                cx.notify();
                return;
            }
        };

        let options = typst_pdf::PdfOptions {
            ident: typst::foundations::Smart::Auto,
            timestamp: None,
            page_ranges: None,
            standards: typst_pdf::PdfStandards::default(),
            tagged: true,
        };

        match typst_pdf::pdf(&doc, &options) {
            Ok(bytes) => {
                match std::fs::write(&pdf_path, &bytes) {
                    Ok(()) => {
                        let name = pdf_path.file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("output.pdf")
                            .to_string();
                        self.export_status = Some(format!("Exported → {name}"));
                    }
                    Err(e) => {
                        self.export_status = Some(format!("Write failed: {e}"));
                    }
                }
            }
            Err(e) => {
                let msg = e.first()
                    .map(|d| d.message.to_string())
                    .unwrap_or_else(|| "Unknown error".to_string());
                self.export_status = Some(format!("PDF error: {msg}"));
            }
        }
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

        // Route through tab management so the daily note gets a tab.
        let _ = window; // Window not needed — open_tab_in_pane uses open_file_no_focus.
        self.open_tab_in_pane(self.active_idx, note_path, cx);
        cx.notify();
    }

    fn open_path(&mut self, abs_path: PathBuf, cx: &mut Context<Self>) {
        self.open_tab_in_pane(self.active_idx, abs_path, cx);
    }

    // ── Session persistence ───────────────────────────────────────────────────

    /// Open the previously saved tab set (called once after construction).
    pub fn restore_session_tabs(&mut self, cx: &mut Context<Self>) {
        let (tabs, active_idx) = crate::session::load_open_tabs();
        if tabs.is_empty() { return; }
        for path in tabs {
            self.open_tab_in_pane(0, path, cx);
        }
        // Switch to the previously active tab.
        if active_idx < self.panes[0].tabs.len() {
            self.switch_tab_in_pane(0, active_idx, cx);
        }
    }

    /// Persist the current tab list (active pane only) to the session file.
    fn persist_tabs(&self) {
        if self.panes.is_empty() { return; }
        let pane = &self.panes[self.active_idx];
        let paths: Vec<PathBuf> = pane.tabs.iter().map(|t| t.path.clone()).collect();
        crate::session::save_open_tabs(&paths, pane.active_tab);
    }

    // ── Tab management ────────────────────────────────────────────────────────

    /// Open `abs_path` as a new tab in `pane_idx`, or switch to it if already open.
    fn open_tab_in_pane(&mut self, pane_idx: usize, abs_path: PathBuf, cx: &mut Context<Self>) {
        // Derive a display title from the file stem.
        let title = abs_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("untitled")
            .to_string();

        // If the file is already open in this pane, just switch to that tab.
        if let Some(existing_idx) = self.panes[pane_idx]
            .tabs
            .iter()
            .position(|t| t.path == abs_path)
        {
            if existing_idx != self.panes[pane_idx].active_tab {
                self.switch_tab_in_pane(pane_idx, existing_idx, cx);
            }
            self.recent_paths.retain(|p| p != &abs_path);
            self.recent_paths.insert(0, abs_path);
            self.recent_paths.truncate(20);
            return;
        }

        // New file: load it into the editor.
        open_file_in_editor(&abs_path, &self.panes[pane_idx].editor, &self.vault, cx);

        // Add or replace a tab entry.
        // Strategy: add after the currently active tab so new tabs feel adjacent.
        let insert_at = self.panes[pane_idx].active_tab + 1;
        let new_tab = TabInfo { path: abs_path.clone(), title, cursor: Pos::new(0, 0), viewport_top: 0 };
        if insert_at >= self.panes[pane_idx].tabs.len() {
            self.panes[pane_idx].tabs.push(new_tab);
            self.panes[pane_idx].active_tab = self.panes[pane_idx].tabs.len() - 1;
        } else {
            self.panes[pane_idx].tabs.insert(insert_at, new_tab);
            self.panes[pane_idx].active_tab = insert_at;
        }

        self.recent_paths.retain(|p| p != &abs_path);
        self.recent_paths.insert(0, abs_path);
        self.recent_paths.truncate(20);

        self.persist_tabs();
    }

    /// Switch the active tab of `pane_idx` to `tab_idx`, loading that file.
    fn switch_tab_in_pane(&mut self, pane_idx: usize, tab_idx: usize, cx: &mut Context<Self>) {
        if pane_idx >= self.panes.len() { return; }
        let pane = &self.panes[pane_idx];
        if tab_idx >= pane.tabs.len() || tab_idx == pane.active_tab { return; }

        // Save cursor / viewport of the departing tab.
        let departing = pane.active_tab;
        let editor = self.panes[pane_idx].editor.read(cx);
        let saved_cursor = editor.cursor_pos();
        let saved_vp = editor.viewport_top();
        self.panes[pane_idx].tabs[departing].cursor = saved_cursor;
        self.panes[pane_idx].tabs[departing].viewport_top = saved_vp;

        // Load the target file into the editor.
        let path = self.panes[pane_idx].tabs[tab_idx].path.clone();
        open_file_in_editor(&path, &self.panes[pane_idx].editor, &self.vault, cx);

        // Restore cursor / viewport of the arriving tab.
        let arriving_cursor = self.panes[pane_idx].tabs[tab_idx].cursor;
        let arriving_vp = self.panes[pane_idx].tabs[tab_idx].viewport_top;
        self.panes[pane_idx].editor.update(cx, |pane, _| {
            pane.restore_cursor_and_viewport(arriving_cursor, arriving_vp);
        });

        self.panes[pane_idx].active_tab = tab_idx;
        self.persist_tabs();
    }

    fn buffer_next(
        &mut self,
        _: &BufferNext,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane = &self.panes[self.active_idx];
        if pane.tabs.len() <= 1 { return; }
        let next = (pane.active_tab + 1) % pane.tabs.len();
        self.switch_tab_in_pane(self.active_idx, next, cx);
        cx.notify();
    }

    fn buffer_prev(
        &mut self,
        _: &BufferPrevious,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane = &self.panes[self.active_idx];
        if pane.tabs.len() <= 1 { return; }
        let prev = if pane.active_tab == 0 {
            pane.tabs.len() - 1
        } else {
            pane.active_tab - 1
        };
        self.switch_tab_in_pane(self.active_idx, prev, cx);
        cx.notify();
    }

    fn buffer_close_tab(
        &mut self,
        _: &BufferClose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane_idx = self.active_idx;
        let pane = &mut self.panes[pane_idx];

        if pane.tabs.is_empty() {
            return;
        }

        let closing = pane.active_tab;
        pane.tabs.remove(closing);

        if pane.tabs.is_empty() {
            // No more tabs — nothing left to show; just notify.
            pane.active_tab = 0;
            cx.notify();
            return;
        }

        // Adjust active index after removal.
        let new_active = if closing >= pane.tabs.len() {
            pane.tabs.len() - 1
        } else {
            closing
        };
        let path = pane.tabs[new_active].path.clone();
        let editor = pane.editor.clone();
        open_file_in_editor(&path, &editor, &self.vault, cx);
        self.panes[pane_idx].active_tab = new_active;

        // Re-focus the editor after closing.
        editor.read(cx).focus_handle(cx).focus(window);
        self.persist_tabs();
        cx.notify();
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
        let raw = match &template_path {
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
            // Derive stem from date/datetime-substituted content (title not yet known).
            let partial = apply_template_substitutions(&raw, "");
            match heading_to_filename_stem(&partial) {
                Some(stem) => format!("{stem}.typ"),
                None => format!("untitled-{date_str}.typ"),
            }
        };

        // Derive title from filename stem and do full substitution.
        let title_stem = filename.trim_end_matches(".typ").to_string();
        let content = apply_template_substitutions(&raw, &title_stem);

        // Avoid clobbering an existing file by appending a counter.
        let note_path = unique_path(&vault_root, &filename);
        let _ = std::fs::write(&note_path, &content);

        // Rescan vault and open the new note.
        self.vault.update(cx, |vs, _cx| {
            *vs = crate::vault::VaultState::open(vault_root.clone());
        });

        // Route through tab management.
        self.open_tab_in_pane(self.active_idx, note_path, cx);
        cx.notify();
    }

    /// Create a new note when a `Window` is available (direct call path).
    fn create_new_note(
        &mut self,
        template_path: Option<PathBuf>,
        vault_root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let raw = match &template_path {
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
            let partial = apply_template_substitutions(&raw, "");
            match heading_to_filename_stem(&partial) {
                Some(stem) => format!("{stem}.typ"),
                None => format!("untitled-{date_str}.typ"),
            }
        };

        let title_stem = filename.trim_end_matches(".typ").to_string();
        let content = apply_template_substitutions(&raw, &title_stem);

        let note_path = unique_path(&vault_root, &filename);
        let _ = std::fs::write(&note_path, &content);

        self.vault.update(cx, |vs, _cx| {
            *vs = crate::vault::VaultState::open(vault_root.clone());
        });

        // Route through tab management.
        let _ = window; // open_tab_in_pane uses open_file_no_focus (no Window needed).
        self.open_tab_in_pane(self.active_idx, note_path, cx);
        cx.notify();
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
    fn handle_palette_execute(&mut self, id: &str, cx: &mut Context<Self>) {
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
            "export-pdf" => cx.dispatch_action(&ExportPdf),
            "open-command-palette" => cx.dispatch_action(&OpenCommandPalette),
            "vault-search" => cx.dispatch_action(&OpenVaultSearch),
            "open-daily-note" => cx.dispatch_action(&OpenDailyNote),
            "split-pane-vertical" => cx.dispatch_action(&SplitPaneVertical),
            "split-pane-horizontal" => cx.dispatch_action(&SplitPaneHorizontal),
            "close-pane" => cx.dispatch_action(&ClosePane),
            "open-graph-view" => cx.dispatch_action(&OpenGraphView),
            "open-plugin-manager" => cx.dispatch_action(&OpenPluginManager),
            "open-backlinks" => cx.dispatch_action(&OpenBacklinks),
            "open-search" => cx.dispatch_action(&OpenSearch),
            "open-replace" => cx.dispatch_action(&OpenReplace),
            "line-numbers-relative" => cx.dispatch_action(&LineNumbersRelative),
            "line-numbers-absolute" => cx.dispatch_action(&LineNumbersAbsolute),
            "line-numbers-off"      => cx.dispatch_action(&LineNumbersOff),
            "reload-settings" => {
                let vault_root = self.vault.read(cx).root.clone();
                let new_settings = crate::settings::load_settings(vault_root.as_deref());
                *cx.global_mut::<crate::settings::Settings>() = new_settings;
            }
            "switch-keyboard-mode" => {
                let editor = self.panes[self.active_idx].editor.clone();
                editor.update(cx, |pane, _cx| pane.switch_keyboard_mode());
            }
            _ => {
                // Plugin command?
                let plugin_id = cx
                    .global::<PluginRegistry>()
                    .command_to_plugin
                    .get(id)
                    .cloned();
                if let Some(pid) = plugin_id {
                    if let Some(inst) = self.plugin_instances.get(&pid) {
                        let job = PluginJob {
                            instance: Arc::clone(inst),
                            command_id: id.to_string(),
                        };
                        let _ = self.plugin_pool.dispatch(job);
                    }
                    // If the plugin has a registered panel, show it.
                    let panel = cx
                        .global::<PluginRegistry>()
                        .plugin_panels
                        .get(&pid)
                        .and_then(|v| v.first())
                        .cloned();
                    if let Some(p) = panel {
                        self.open_plugin_panel(p, cx);
                    }
                }
            }
        }
    }

    // ── Plugin helpers ────────────────────────────────────────────────────────

    /// Probe capabilities from WASM metadata, then instantiate with real caps.
    ///
    /// Returns both the live instances and a list of `(metadata, status)` pairs
    /// so the caller can update `PluginRegistry` on the UI thread.
    fn instantiate_vault_plugins(
        engine: &wasmtime::Engine,
        vault_root: Option<&std::path::Path>,
        event_tx: std::sync::mpsc::Sender<PluginEvent>,
        plugin_packages: Arc<RwLock<HashMap<String, String>>>,
    ) -> (
        HashMap<String, Arc<Mutex<PluginInstance>>>,
        Vec<(PluginMetadataJson, PluginStatus)>,
    ) {
        let mut instances = HashMap::new();
        let mut info_out: Vec<(PluginMetadataJson, PluginStatus)> = Vec::new();
        let Some(root) = vault_root else { return (instances, info_out); };
        for (entry, wasm) in load_vault_plugins(root) {
            // 1. Probe metadata to learn actual capabilities.
            let meta = match PluginInstance::probe_metadata(engine, &wasm) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("[ockr] failed to read metadata for '{}': {}", entry.id, e);
                    continue;
                }
            };
            let caps = meta.capabilities.clone();
            // 2. Create real instance with proper WASI capabilities.
            match PluginInstance::new(
                engine,
                &wasm,
                &meta.id,
                &caps,
                root,
                event_tx.clone(),
                Arc::clone(&plugin_packages),
            ) {
                Ok(mut inst) => {
                    let status = match inst.init() {
                        Ok(_) => PluginStatus::Loaded,
                        Err(e) => PluginStatus::Failed(format!("init failed: {e}")),
                    };
                    info_out.push((meta.clone(), status));
                    instances.insert(meta.id, Arc::new(Mutex::new(inst)));
                }
                Err(e) => {
                    eprintln!("[ockr] failed to load plugin '{}': {}", meta.id, e);
                    info_out.push((meta, PluginStatus::Failed(e)));
                }
            }
        }
        (instances, info_out)
    }

    /// Unload all plugins from the old vault and load from the new vault root.
    fn reload_vault_plugins(&mut self, cx: &mut Context<Self>) {
        // Collect command IDs owned by plugins being unloaded.
        let old_ids: Vec<String> = self.plugin_instances.keys().cloned().collect();
        let commands_to_remove: std::collections::HashSet<String> = {
            let reg = cx.global::<PluginRegistry>();
            reg.command_to_plugin
                .iter()
                .filter(|(_, pid)| old_ids.contains(pid))
                .map(|(cmd_id, _)| cmd_id.clone())
                .collect()
        };
        cx.global_mut::<crate::command::CommandRegistry>()
            .remove_where(|e| commands_to_remove.contains(&e.id));
        // Clear registry maps (including metadata / status).
        cx.global_mut::<PluginRegistry>().remove_plugins(&old_ids);
        // Clear plugin_packages contributed by old plugins.
        if let Ok(mut guard) = self.plugin_packages.write() {
            guard.clear();
        }
        // Drop old instances.
        self.plugin_instances.clear();

        // Load from new vault.
        let vault_root = self.vault.read(cx).root.clone();
        let event_tx = cx.global::<PluginRegistry>().event_tx.clone();
        let (new_instances, plugin_info) = Self::instantiate_vault_plugins(
            &self.wasmtime_engine,
            vault_root.as_deref(),
            event_tx,
            Arc::clone(&self.plugin_packages),
        );
        self.plugin_instances = new_instances;
        {
            let reg = cx.global_mut::<PluginRegistry>();
            for (meta, status) in plugin_info {
                reg.mark_loaded(PluginInfo::from(&meta));
                reg.plugin_statuses.insert(meta.id, status);
            }
        }

        // Push plugin_packages to all open editors.
        for pane in &self.panes {
            pane.editor.update(cx, |p, _| {
                p.set_plugin_packages(Arc::clone(&self.plugin_packages));
            });
        }
        cx.notify();
    }

    // ── Plugin event draining ─────────────────────────────────────────────────

    /// Drain pending plugin events and apply them to the UI + registries.
    fn drain_plugin_events(&mut self, cx: &mut Context<Self>) {
        let events: Vec<PluginEvent> = {
            let guard = cx.global::<PluginRegistry>().event_rx.lock().unwrap();
            guard.try_iter().collect()
        };
        if events.is_empty() {
            return;
        }
        for event in events {
            match event {
                PluginEvent::CommandRegistered { plugin_id, id, name, hint } => {
                    let entry = CommandEntry::new(id.clone(), name, hint, |_| {});
                    cx.global_mut::<crate::command::CommandRegistry>().register(entry);
                    let reg = cx.global_mut::<PluginRegistry>();
                    reg.plugin_commands.entry(plugin_id.clone()).or_default().push(id.clone());
                    reg.command_to_plugin.insert(id, plugin_id);
                }
                PluginEvent::PanelRegistered { plugin_id, panel } => {
                    let panels = cx.global_mut::<PluginRegistry>()
                        .plugin_panels
                        .entry(plugin_id)
                        .or_default();
                    if !panels.iter().any(|p| p.panel_id == panel.panel_id) {
                        panels.push(panel.clone());
                    }
                    self.open_plugin_panel(panel, cx);
                }
                PluginEvent::LogLine { message, .. } => {
                    self.export_status = Some(message);
                }
                PluginEvent::Panicked { plugin_id, message } => {
                    self.export_status = Some(format!("[plugin error] {plugin_id}: {message}"));
                    cx.global_mut::<PluginRegistry>().mark_failed(&plugin_id, message);
                }
            }
        }
        // Single notify for the whole batch.
        cx.notify();
    }

    /// Create and store a plugin panel overlay; focus is applied in render().
    fn open_plugin_panel(
        &mut self,
        panel: crate::plugin::panel::RegisteredPanel,
        cx: &mut Context<Self>,
    ) {
        let entity = cx.new(|cx| PluginPanel::new(panel, cx));
        cx.subscribe(&entity, |this, _, event: &PluginPanelEvent, cx| {
            match event {
                PluginPanelEvent::Close => {
                    this.plugin_panel = None;
                    cx.notify();
                }
                PluginPanelEvent::ExecuteCommand(cmd_id) => {
                    let id = cmd_id.clone();
                    this.handle_palette_execute(&id, cx);
                }
            }
        })
        .detach();
        self.plugin_panel = Some(entity);
        self.plugin_panel_focus_pending = true;
        cx.notify();
    }

    /// Open (or close) the plugin manager overlay.
    fn open_plugin_manager(
        &mut self,
        _: &OpenPluginManager,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.plugin_manager.is_some() {
            self.plugin_manager = None;
            cx.notify();
            return;
        }
        // Build a snapshot of current plugin info + status.
        let reg = cx.global::<PluginRegistry>();
        let plugins: Vec<(PluginInfo, PluginStatus)> = reg.plugin_info
            .values()
            .map(|info| {
                let status = reg.plugin_statuses
                    .get(&info.id)
                    .cloned()
                    .unwrap_or(PluginStatus::Loaded);
                (info.clone(), status)
            })
            .collect();
        let _ = reg;

        let entity = cx.new(|cx| PluginManager::new(plugins, cx));
        cx.subscribe(&entity, |this, _, event: &PluginManagerEvent, cx| {
            match event {
                PluginManagerEvent::Close => {
                    this.plugin_manager = None;
                    cx.notify();
                }
            }
        })
        .detach();
        self.plugin_manager = Some(entity);
        self.plugin_manager_focus_pending = true;
        cx.notify();
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

/// Substitute template placeholders in note content.
///
/// Supported tokens:
/// - `{{date}}`     → YYYY-MM-DD (today)
/// - `{{datetime}}` → YYYY-MM-DD HH:MM
/// - `{{title}}`    → note title derived from the filename stem (or empty)
fn apply_template_substitutions(content: &str, title: &str) -> String {
    let now = time::OffsetDateTime::now_local()
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let date_str = format!(
        "{:04}-{:02}-{:02}",
        now.year(), now.month() as u8, now.day()
    );
    let datetime_str = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        now.year(), now.month() as u8, now.day(),
        now.hour(), now.minute()
    );
    content
        .replace("{{date}}", &date_str)
        .replace("{{datetime}}", &datetime_str)
        .replace("{{title}}", title)
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

        // Focus the plugin panel if it was just created.
        if self.plugin_panel_focus_pending {
            if let Some(ref pp) = self.plugin_panel {
                pp.read(cx).focus_handle.clone().focus(window);
            }
            self.plugin_panel_focus_pending = false;
        }

        // Focus the plugin manager if it was just created.
        if self.plugin_manager_focus_pending {
            if let Some(ref pm) = self.plugin_manager {
                pm.read(cx).focus_handle.clone().focus(window);
            }
            self.plugin_manager_focus_pending = false;
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
                    self.html_webview = HtmlWebView::new(self.html_link_sender.clone());
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
            .on_action(cx.listener(Self::export_pdf))
            .on_action(cx.listener(Self::open_daily_note))
            .on_action(cx.listener(Self::split_pane_vertical))
            .on_action(cx.listener(Self::split_pane_horizontal))
            .on_action(cx.listener(Self::close_pane))
            .on_action(cx.listener(Self::focus_pane_left))
            .on_action(cx.listener(Self::focus_pane_right))
            .on_action(cx.listener(Self::focus_pane_up))
            .on_action(cx.listener(Self::focus_pane_down))
            .on_action(cx.listener(Self::buffer_next))
            .on_action(cx.listener(Self::buffer_prev))
            .on_action(cx.listener(Self::buffer_close_tab))
            .on_action(cx.listener(Self::open_plugin_manager))
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
        // Each sub-pane is rendered as a flex-col with a tab bar at the top.

        let sidebar_w = if self.sidebar_visible { self.sidebar_width + 4.0 } else { 0.0 };
        let editor_area_w = (content_w as f32 - sidebar_w - self.preview_width - 4.0).max(200.0);

        // Build a pane column (tab bar + editor) for pane at `idx`.
        let pane_col = |pane_idx: usize, this: &Self, cx: &mut Context<Self>| -> gpui::AnyElement {
            let pane = &this.panes[pane_idx];
            let editor = pane.editor.clone();
            let is_active = pane_idx == this.active_idx;

            // Tab bar (only shown when there are tabs to display).
            let tab_bar: gpui::AnyElement = if pane.tabs.is_empty() {
                div().into_any_element()
            } else {
                let tab_items: Vec<gpui::AnyElement> = pane.tabs
                    .iter()
                    .enumerate()
                    .map(|(i, tab)| {
                        let is_active_tab = i == pane.active_tab && is_active;
                        let label = tab.title.clone();
                        // Dirty indicator: read from editor only for the active tab.
                        let dirty = if i == pane.active_tab {
                            pane.editor.read(cx).is_dirty()
                        } else {
                            false
                        };

                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .px_3()
                            .py_1()
                            .text_xs()
                            .font_family("Menlo")
                            .cursor_pointer()
                            .border_b_2()
                            .border_color(if is_active_tab {
                                gpui::rgb(t.ochre)
                            } else {
                                gpui::rgb(t.bg_base)
                            })
                            .text_color(if is_active_tab {
                                gpui::rgb(t.text)
                            } else {
                                gpui::rgb(t.text_muted)
                            })
                            .hover(|s| s.text_color(gpui::rgb(t.text)))
                            .child(label)
                            .when(dirty, |d| {
                                d.child(
                                    div()
                                        .text_color(gpui::rgb(t.ochre))
                                        .child("●"),
                                )
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event, _window, cx| {
                                    this.switch_tab_in_pane(pane_idx, i, cx);
                                    cx.notify();
                                }),
                            )
                            .into_any_element()
                    })
                    .collect();

                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_end()
                    .h(px(30.0))
                    .bg(gpui::rgb(t.bg_base))
                    .border_b_1()
                    .border_color(gpui::rgb(t.border_subtle))
                    .overflow_x_hidden()
                    .children(tab_items)
                    .into_any_element()
            };

            div()
                .h_full()
                .w_full()
                .flex()
                .flex_col()
                .child(tab_bar)
                .child(div().flex_1().overflow_hidden().child(editor))
                .into_any_element()
        };

        let editor_area = match self.split_layout {
            SplitLayout::Vertical if self.panes.len() >= 2 => {
                let w0 = (editor_area_w * self.pane_split_frac).round();
                let w1 = editor_area_w - w0 - 4.0;

                if let Some(ref mut drag) = self.drag {
                    if drag.target == DragTarget::PaneDivider {
                        drag.start_y = editor_area_w;
                    }
                }

                let col0 = pane_col(0, self, cx);
                let col1 = pane_col(1, self, cx);
                div().flex_1().min_w_0().h_full().flex().flex_row()
                    .child(div().w(px(w0)).h_full().overflow_hidden().child(col0))
                    .child(handle(DragTarget::PaneDivider, true, cx))
                    .child(div().w(px(w1)).h_full().overflow_hidden().child(col1))
                    .into_any_element()
            }
            SplitLayout::Horizontal if self.panes.len() >= 2 => {
                let h0 = (content_h as f32 * self.pane_split_frac).round();
                let h1 = content_h as f32 - h0 - 4.0;

                let col0 = pane_col(0, self, cx);
                let col1 = pane_col(1, self, cx);
                div().flex_1().min_w_0().h_full().flex().flex_col()
                    .child(div().w_full().h(px(h0)).overflow_hidden().child(col0))
                    .child(handle(DragTarget::PaneDivider, false, cx))
                    .child(div().w_full().h(px(h1)).overflow_hidden().child(col1))
                    .into_any_element()
            }
            _ => {
                let col0 = pane_col(0, self, cx);
                div().flex_1().min_w_0().h_full().overflow_hidden()
                    .child(col0)
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

        // ── Export status toast ───────────────────────────────────────────────
        let export_toast = self.export_status.take().map(|msg| {
            gpui::deferred(
                div()
                    .absolute()
                    .bottom(px(24.0))
                    .left(px(0.0))
                    .right(px(0.0))
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .px_4()
                            .py_2()
                            .bg(gpui::rgb(t.bg_panel))
                            .border_1()
                            .border_color(gpui::rgb(t.border_subtle))
                            .rounded(px(6.0))
                            .text_xs()
                            .text_color(gpui::rgb(t.text))
                            .font_family("Menlo")
                            .child(msg),
                    )
            ).with_priority(150)
        });

        root.child(editor_area)
            .child(handle(DragTarget::Preview, true, cx))
            .child(preview_col)
            .when_some(export_toast, |root, toast| root.child(toast))
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
            .when_some(self.plugin_panel.clone(), |root, pp| {
                root.child(gpui::deferred(pp).with_priority(150))
            })
            .when_some(self.plugin_manager.clone(), |root, pm| {
                root.child(gpui::deferred(pm).with_priority(160))
            })
    }
}
