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
    FocusPaneRight, FocusPaneUp, NewNote, OpenBacklinks, OpenCommandPalette,
    OpenDailyNote, OpenFilePicker, OpenGraphView, OpenOutline, OpenPluginManager, OpenQuickSwitch, OpenRecentFiles,
    LineNumbersAbsolute, LineNumbersOff, LineNumbersRelative,
    OpenSettings, OpenVault, OpenVaultSearch, ReloadFile, SplitPaneHorizontal,
    SplitPaneVertical, TogglePreviewMode, ToggleSidebar, ToggleZenMode,
};
use crate::compiler::{spawn_compiler_thread, CompileResult, CompilerHandle, PreviewMode};
use crate::lsp::{self, LspHandle, LspMessage};
use crate::ui::backlink_panel::{BacklinkPanel, BacklinkPanelEvent};
use crate::ui::outline_panel::{OutlinePanel, OutlinePanelEvent};
use crate::ui::graph_view::{GraphView, GraphViewEvent};
use crate::ui::command_palette::{CommandPalette, PaletteEvent};
use crate::ui::html_preview::HtmlWebView;
use crate::ui::file_picker::{FilePicker, FilePickerEvent};
use crate::ui::quick_switch::{QuickSwitch, QuickSwitchEvent};
use crate::ui::rename_modal::{RenameModal, RenameModalEvent};
use crate::ui::settings_panel::{SettingKey, SettingsPanel, SettingsPanelEvent};
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

// ── Tooltip ───────────────────────────────────────────────────────────────────

/// Minimal single-line tooltip used by the activity rail's icon buttons.
struct RailTooltip {
    text: gpui::SharedString,
}

impl Render for RailTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.global::<ThemePalette>().clone();
        div()
            .px_2()
            .py_1()
            .bg(gpui::rgb(t.bg_surface))
            .border_1()
            .border_color(gpui::rgb(t.border_subtle))
            .rounded(px(4.0))
            .shadow_lg()
            .text_xs()
            .font_family("Menlo")
            .text_color(gpui::rgb(t.text))
            .child(self.text.clone())
    }
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
    /// `false` until the first render seeds `preview_width` from the window size.
    preview_width_set: bool,
    drag: Option<DragState>,
    /// Whether Zen Mode (distraction-free writing) is active.
    zen_mode: bool,
    /// Sidebar visibility saved when entering Zen Mode; restored on exit.
    zen_saved_sidebar: bool,
    /// Preview width saved when entering Zen Mode; restored on exit.
    zen_saved_preview: f32,
    palette: Option<Entity<CommandPalette>>,
    /// True when a pane requested the palette; created in next render (needs `&mut Window`).
    open_palette_pending: bool,
    /// True when the palette was just dismissed and the active editor needs focus.
    refocus_editor_pending: bool,
    quick_switch: Option<Entity<QuickSwitch>>,
    file_picker: Option<Entity<FilePicker>>,
    template_picker: Option<Entity<TemplatePicker>>,
    backlinks: Option<Entity<BacklinkPanel>>,
    outline: Option<Entity<OutlinePanel>>,
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
    /// Handle to the `tinymist` LSP thread.  `None` if tinymist is not installed.
    lsp: Option<LspHandle>,
    /// Window-resize subscription (registered on first render).  Forces a full
    /// re-render on resize: GPUI's text-measure cache can go incoherent when
    /// the retained tree is re-laid-out at a new width, painting more wrapped
    /// lines than the row's height (overlapping rows).  Fresh elements per
    /// resize keep measurement and paint consistent.
    bounds_sub: Option<gpui::Subscription>,
    /// Active file-rename modal, if any.
    rename_modal: Option<Entity<RenameModal>>,
    /// True when `rename_modal` was just created and needs focus on next render.
    rename_modal_focus_pending: bool,
    /// Active settings panel, if any.
    settings_panel: Option<Entity<SettingsPanel>>,
    /// True when `settings_panel` was just created and needs focus on next render.
    settings_panel_focus_pending: bool,
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

        // ── Story 38: defer plugin loading to background ──────────────────────
        // Start with an empty plugin map; the background task below will probe
        // and instantiate vault plugins after the first frame is shown.
        let plugin_instances: HashMap<String, Arc<Mutex<PluginInstance>>> = HashMap::new();
        {
            let engine = wasmtime_engine.clone();
            let vault_root = vault.read(cx).root.clone();
            let event_tx = cx.global::<PluginRegistry>().event_tx.clone();
            let pkgs = Arc::clone(&plugin_packages);
            cx.spawn(async move |this, cx| {
                let (instances, info) = cx
                    .background_executor()
                    .spawn(async move {
                        Self::instantiate_vault_plugins(
                            &engine,
                            vault_root.as_deref(),
                            event_tx,
                            pkgs,
                        )
                    })
                    .await;
                cx.update(|cx| {
                    this.update(cx, |win, cx| {
                        win.plugin_instances = instances;
                        let reg = cx.global_mut::<PluginRegistry>();
                        for (meta, status) in info {
                            reg.mark_loaded(PluginInfo::from(&meta));
                            reg.plugin_statuses.insert(meta.id, status);
                        }
                        cx.notify();
                    })
                    .ok();
                })
                .ok();
            })
            .detach();
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
                SidebarEvent::RenameFile(abs_path) => {
                    this.open_rename_modal(abs_path.clone(), cx);
                }
                SidebarEvent::DeleteFile(abs_path) => {
                    this.delete_file(abs_path.clone(), cx);
                }
                SidebarEvent::RevealFile(abs_path) => {
                    // Reveal in Finder (best-effort; ignore failures).
                    let _ = std::process::Command::new("open")
                        .arg("-R")
                        .arg(abs_path)
                        .spawn();
                }
                SidebarEvent::OpenPluginPanel { plugin_id, panel_id } => {
                    // Look up the RegisteredPanel from the registry and open it.
                    let panel = cx
                        .global::<PluginRegistry>()
                        .plugin_panels
                        .get(plugin_id)
                        .and_then(|panels| panels.iter().find(|p| &p.panel_id == panel_id))
                        .cloned();
                    if let Some(p) = panel {
                        this.open_plugin_panel(p, cx);
                    }
                }
            }
        }).detach();

        // ── Vault change → reload plugins + kick background backlink build ────
        cx.observe(&vault, |this, vault_entity, cx| {
            let new_root = vault_entity.read(cx).root.clone();
            let old_root = this.vault.read(cx).root.clone();
            if new_root != old_root {
                this.reload_vault_plugins(cx);
            }
            // If another code path set indexing = true, start the build here.
            if vault_entity.read(cx).indexing {
                this.spawn_backlink_build_if_needed(cx);
            }
        }).detach();

        // ── Story 36: kick off background backlink build if vault was opened ─
        // Spawn immediately so the window is visible before the index builds.
        {
            let vault_for_idx = vault.clone();
            if vault_for_idx.read(cx).indexing {
                let files = vault_for_idx.read(cx).files.clone();
                cx.spawn(async move |_this, cx| {
                    let index = cx
                        .background_executor()
                        .spawn(async move { crate::vault::BacklinkIndex::build(&files) })
                        .await;
                    cx.update(|cx| {
                        vault_for_idx.update(cx, |vs, cx| {
                            vs.finish_backlink_build(index);
                            cx.notify();
                        });
                    })
                    .ok();
                })
                .detach();
            }
        }

        // ── LSP (tinymist) ────────────────────────────────────────────────────
        // Spawn a tinymist LSP client for the current vault root (if installed).
        // Results arrive via an unbounded channel and are dispatched to the
        // active editor pane on the GPUI UI thread.
        let (lsp_tx, mut lsp_rx) =
            futures::channel::mpsc::unbounded::<LspMessage>();
        let lsp_handle = lsp::spawn_lsp(
            vault.read(cx).root.clone(),
            move |msg| { let _ = lsp_tx.unbounded_send(msg); },
        );
        // Wire the handle into the initial pane so it can send notifications.
        if let Some(ref handle) = lsp_handle {
            editor.update(cx, |pane, _| pane.set_lsp(handle.clone()));
        }
        // Drain LSP messages on the UI thread.
        cx.spawn(async move |this, cx| {
            while let Some(msg) = lsp_rx.next().await {
                let _ = cx.update(|cx| {
                    let _ = this.update(cx, |win, cx| win.handle_lsp_message(msg, cx));
                });
            }
        })
        .detach();

        // NOTE: spell-check is intentionally left disabled.  The synchronous
        // NSSpellChecker.checkSpellingOfString call blocks the main thread
        // (XPC round-trip that never returns when invoked on the render
        // thread), which froze the window on launch.  Re-enable only once it
        // is reimplemented off the main thread.  See editor_pane::check_spelling.

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
            preview_width_set: false,
            drag: None,
            zen_mode: false,
            zen_saved_sidebar: true,
            zen_saved_preview: 420.0,
            palette: None,
            open_palette_pending: false,
            refocus_editor_pending: false,
            quick_switch: None,
            file_picker: None,
            template_picker: None,
            backlinks: None,
            outline: None,
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
            lsp: lsp_handle,
            bounds_sub: None,
            rename_modal: None,
            rename_modal_focus_pending: false,
            settings_panel: None,
            settings_panel_focus_pending: false,
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
                    this.open_palette_pending = true;
                    cx.notify();
                }
            }
        }).detach();
    }

    /// Spawn a new pane, wire compiler + vault + LSP, subscribe events.
    fn new_pane(&mut self, cx: &mut Context<Self>) -> Entity<EditorPane> {
        let editor = cx.new(|cx| EditorPane::new(cx));
        editor.update(cx, |pane, _cx| pane.set_vault(self.vault.clone()));
        editor.update(cx, |pane, _cx| {
            pane.set_compiler(self.compiler_handle.clone(), self.preview.clone());
        });
        if let Some(ref handle) = self.lsp {
            let h = handle.clone();
            editor.update(cx, |pane, _| pane.set_lsp(h));
        }
        Self::subscribe_pane(cx, &editor);
        editor
    }

    /// Returns the active editor entity.
    fn active_editor(&self) -> &Entity<EditorPane> {
        &self.panes[self.active_idx].editor
    }

    // ── LSP message dispatch ──────────────────────────────────────────────────

    fn handle_lsp_message(&mut self, msg: LspMessage, cx: &mut Context<Self>) {
        match msg {
            LspMessage::Diagnostics { uri, diags } => {
                // Forward to any pane whose current file matches the URI.
                for entry in &self.panes {
                    entry.editor.update(cx, |pane, cx| {
                        if pane.current_uri().as_deref() == Some(&uri) {
                            pane.set_lsp_diagnostics(diags.clone());
                            cx.notify();
                        }
                    });
                }
            }
            LspMessage::HoverResult { request_id, result } => {
                // Route to whichever pane is waiting for this id.
                for entry in &self.panes {
                    entry.editor.update(cx, |pane, cx| {
                        pane.set_hover_result(request_id, result.clone());
                        cx.notify();
                    });
                }
            }
            LspMessage::DefinitionResult { request_id, result } => {
                if let Some(def) = result {
                    // Find the pane that made this request.
                    let matched = self.panes.iter().enumerate().find(|(_, e)| {
                        e.editor.read(cx).is_waiting_for_definition(request_id)
                    }).map(|(i, _)| i);

                    if let Some(pane_idx) = matched {
                        let target_uri = crate::lsp::path_to_uri(&def.path);
                        let same_file = self.panes[pane_idx]
                            .editor
                            .read(cx)
                            .current_uri()
                            .is_some_and(|u| u == target_uri);

                        self.panes[pane_idx].editor.update(cx, |pane, _| {
                            pane.take_def_request(request_id);
                        });

                        if same_file {
                            self.panes[pane_idx].editor.update(cx, |pane, cx| {
                                pane.jump_to_lsp_pos(def.line, def.col);
                                cx.notify();
                            });
                        } else {
                            // Open the target file, then position at the definition.
                            self.open_path(def.path, cx);
                            let editor = self.active_editor().clone();
                            editor.update(cx, |pane, cx| {
                                pane.jump_to_lsp_pos(def.line, def.col);
                                cx.notify();
                            });
                        }
                    }
                }
            }
            LspMessage::CompletionResult { request_id, items } => {
                for entry in &self.panes {
                    entry.editor.update(cx, |pane, cx| {
                        pane.set_completion_result(request_id, items.clone());
                        cx.notify();
                    });
                }
            }
            LspMessage::Unavailable => {
                // tinymist not found or crashed — clear the handle silently.
                self.lsp = None;
            }
        }
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

    fn toggle_zen_mode(
        &mut self,
        _: &ToggleZenMode,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.zen_mode {
            // Exit Zen Mode — restore saved layout state.
            self.sidebar_visible = self.zen_saved_sidebar;
            self.preview_width = self.zen_saved_preview;
            self.zen_mode = false;
        } else {
            // Enter Zen Mode — save current layout state then hide chrome.
            self.zen_saved_sidebar = self.sidebar_visible;
            self.zen_saved_preview = self.preview_width;
            self.sidebar_visible = false;
            self.zen_mode = true;
        }
        cx.notify();
    }

    fn toggle_preview_mode(
        &mut self,
        _: &TogglePreviewMode,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Flip the canonical Settings value; set_preview_mode writes the
        // PreviewMode global + Settings + persists, so they never drift.
        let next = if cx.global::<crate::settings::Settings>().preview_mode == "paged" {
            "html"
        } else {
            "paged"
        };
        self.set_preview_mode(next, cx);
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
        self.spawn_backlink_build_if_needed(cx);

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
        // Restore the recent-paths list before opening tabs so that
        // open_tab_in_pane doesn't push session-restored paths to the front
        // of a list that is about to be overwritten anyway.
        self.recent_paths = crate::session::load_recent_paths();

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

        crate::session::save_recent_paths(&self.recent_paths);
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
        let tab_idx = self.panes[pane_idx].active_tab;
        self.close_tab_at(pane_idx, tab_idx, window, cx);
    }

    /// Close tab `tab_idx` in pane `pane_idx` — the shared logic behind
    /// `Cmd-W` (always closes the active tab) and the tab bar's `×` button
    /// (can close any tab, active or not).
    fn close_tab_at(
        &mut self,
        pane_idx: usize,
        tab_idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if pane_idx >= self.panes.len() {
            return;
        }
        let pane = &mut self.panes[pane_idx];
        if tab_idx >= pane.tabs.len() {
            return;
        }

        let was_active = tab_idx == pane.active_tab;
        pane.tabs.remove(tab_idx);

        if pane.tabs.is_empty() {
            // No more tabs — clear the editor so it doesn't keep showing the
            // now-closed buffer.
            pane.active_tab = 0;
            let editor = pane.editor.clone();
            editor.update(cx, |ep, c| {
                ep.close_document();
                c.notify();
            });
            self.persist_tabs();
            cx.notify();
            return;
        }

        if was_active {
            // Adjust active index after removing the currently-shown tab, then
            // load whichever tab now occupies that slot.
            let new_active = if tab_idx >= pane.tabs.len() {
                pane.tabs.len() - 1
            } else {
                tab_idx
            };
            let path = pane.tabs[new_active].path.clone();
            let editor = pane.editor.clone();
            open_file_in_editor(&path, &editor, &self.vault, cx);
            self.panes[pane_idx].active_tab = new_active;
            editor.read(cx).focus_handle(cx).focus(window);
        } else if tab_idx < pane.active_tab {
            // A tab before the active one was removed — shift the index down
            // so `active_tab` still points at the same (still-open) tab.
            self.panes[pane_idx].active_tab -= 1;
        }

        self.persist_tabs();
        cx.notify();
    }

    /// Move a vault file to the system Trash, drop any tabs showing it, and
    /// rescan the vault so the sidebar + backlinks reflect the removal.
    /// Trash (not `rm`) keeps the operation recoverable.
    fn delete_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if trash::delete(&path).is_err() {
            // Couldn't trash (e.g. permissions) — leave everything untouched.
            return;
        }

        // Drop tabs pointing at the deleted file; reload the active tab if its
        // pane changed.
        for pane_idx in 0..self.panes.len() {
            let before = self.panes[pane_idx].tabs.len();
            self.panes[pane_idx].tabs.retain(|t| t.path != path);
            if self.panes[pane_idx].tabs.len() == before {
                continue; // this pane wasn't showing the file
            }
            let pane = &mut self.panes[pane_idx];
            if pane.tabs.is_empty() {
                pane.active_tab = 0;
                let editor = pane.editor.clone();
                editor.update(cx, |p, c| { p.close_document(); c.notify(); });
            } else {
                if pane.active_tab >= pane.tabs.len() {
                    pane.active_tab = pane.tabs.len() - 1;
                }
                let tab_path = pane.tabs[pane.active_tab].path.clone();
                let editor = pane.editor.clone();
                open_file_in_editor(&tab_path, &editor, &self.vault, cx);
            }
        }
        self.persist_tabs();

        // Rescan the vault (rebuilds the file list; backlink cache self-heals).
        if let Some(root) = self.vault.read(cx).root.clone() {
            self.vault.update(cx, |vs, cx| {
                *vs = VaultState::open(root);
                cx.notify();
            });
            self.spawn_backlink_build_if_needed(cx);
        }
        cx.notify();
    }

    /// Open the rename modal for `path` and wire its events.
    fn open_rename_modal(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let modal = cx.new(|cx| RenameModal::new(path, cx));
        cx.subscribe(&modal, |this, _, event: &RenameModalEvent, cx| {
            match event {
                RenameModalEvent::Close => {
                    this.rename_modal = None;
                    cx.notify();
                }
                RenameModalEvent::Submit { original, new_stem } => {
                    this.rename_modal = None;
                    this.rename_file(original.clone(), new_stem.clone(), cx);
                    cx.notify();
                }
            }
        })
        .detach();
        self.rename_modal = Some(modal);
        self.rename_modal_focus_pending = true;
        cx.notify();
    }

    /// Rename a vault file to `new_stem` (extension + parent dir preserved),
    /// update any open tab pointing at it, and rescan the vault.
    fn rename_file(&mut self, original: PathBuf, new_stem: String, cx: &mut Context<Self>) {
        let parent = original.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let ext = original.extension().and_then(|e| e.to_str()).unwrap_or("typ");
        let dest = parent.join(format!("{new_stem}.{ext}"));

        if dest == original {
            return; // no change
        }
        if dest.exists() {
            self.export_status = Some(format!("Rename failed: {new_stem}.{ext} already exists"));
            cx.notify();
            return;
        }
        if std::fs::rename(&original, &dest).is_err() {
            self.export_status = Some("Rename failed".to_string());
            cx.notify();
            return;
        }

        // Repoint any tab showing the old path; reload affected panes.
        for pane_idx in 0..self.panes.len() {
            let mut changed = false;
            for tab in &mut self.panes[pane_idx].tabs {
                if tab.path == original {
                    tab.path = dest.clone();
                    tab.title = new_stem.clone();
                    changed = true;
                }
            }
            if changed {
                let pane = &self.panes[pane_idx];
                let active_path = pane.tabs.get(pane.active_tab).map(|t| t.path.clone());
                if active_path.as_deref() == Some(dest.as_path()) {
                    let editor = pane.editor.clone();
                    open_file_in_editor(&dest, &editor, &self.vault, cx);
                }
            }
        }
        self.persist_tabs();

        if let Some(root) = self.vault.read(cx).root.clone() {
            self.vault.update(cx, |vs, cx| {
                *vs = VaultState::open(root);
                cx.notify();
            });
            self.spawn_backlink_build_if_needed(cx);
        }
        cx.notify();
    }

    // ── Settings panel (S7) ────────────────────────────────────────────────────

    fn open_settings(&mut self, _: &OpenSettings, _window: &mut Window, cx: &mut Context<Self>) {
        // Toggle.
        if self.settings_panel.is_some() {
            self.settings_panel = None;
            self.refocus_editor_pending = true;
            cx.notify();
            return;
        }
        // Only one overlay at a time.
        self.quick_switch = None;
        self.file_picker = None;
        self.rename_modal = None;
        let panel = cx.new(|cx| SettingsPanel::new(cx));
        cx.subscribe(&panel, |this, _, event: &SettingsPanelEvent, cx| match event {
            SettingsPanelEvent::Close => {
                this.settings_panel = None;
                this.refocus_editor_pending = true;
                cx.notify();
            }
            SettingsPanelEvent::Cycle(key) => {
                this.apply_setting(*key, cx);
                cx.notify();
            }
        })
        .detach();
        self.settings_panel = Some(panel);
        self.settings_panel_focus_pending = true;
        cx.notify();
    }

    /// Advance one setting to its next value (cycle), driven off the canonical
    /// `Settings` global and applied via the shared `set_*` writers.
    fn apply_setting(&mut self, key: SettingKey, cx: &mut Context<Self>) {
        let s = cx.global::<crate::settings::Settings>();
        match key {
            SettingKey::Keyboard => {
                let next = if s.keyboard_mode == "standard" { "helix" } else { "standard" };
                self.set_keyboard_mode(next, cx);
            }
            SettingKey::Theme => {
                let next = if s.theme == "ochre" { "oxide" } else { "ochre" };
                self.set_theme(next, cx);
            }
            SettingKey::LineNumbers => {
                let next = match s.line_number_mode.as_str() {
                    "relative" => "absolute",
                    "absolute" => "off",
                    _ => "relative",
                };
                self.set_line_number_mode(next, cx);
            }
            SettingKey::Preview => {
                let next = if s.preview_mode == "paged" { "html" } else { "paged" };
                self.set_preview_mode(next, cx);
            }
        }
    }

    // ── Canonical setting writers ───────────────────────────────────────────────
    //
    // Every setting has exactly one writer.  Each applies the change live to all
    // affected views, updates the `Settings` global (single source of truth), and
    // persists to the global settings file.  All entry points — the settings
    // panel, palette commands, menu actions, keybindings — funnel through these,
    // so the displayed value, the live state, and the saved value can never drift.

    fn set_keyboard_mode(&mut self, mode: &str, cx: &mut Context<Self>) {
        let mode = if mode == "standard" { "standard" } else { "helix" };
        for entry in &self.panes {
            entry.editor.update(cx, |pane, c| {
                pane.set_keyboard_mode(mode);
                c.notify();
            });
        }
        cx.global_mut::<crate::settings::Settings>().keyboard_mode = mode.to_string();
        crate::settings::save_global_setting("keyboard_mode", mode);
        cx.notify();
    }

    fn set_theme(&mut self, name: &str, cx: &mut Context<Self>) {
        let name = if name == "ochre" { "ochre" } else { "oxide" };
        *cx.global_mut::<ThemePalette>() = crate::load_theme_by_name(name);
        cx.global_mut::<crate::settings::Settings>().theme = name.to_string();
        crate::settings::save_global_setting("theme", name);
        // Panes read the theme at render time and aren't global observers, so
        // nudge them (and self) to repaint with the new palette.
        for entry in &self.panes {
            entry.editor.update(cx, |_, c| c.notify());
        }
        cx.notify();
    }

    fn set_line_number_mode(&mut self, name: &str, cx: &mut Context<Self>) {
        use crate::ui::editor_pane::LineNumberMode;
        let (name, mode) = match name {
            "absolute" => ("absolute", LineNumberMode::Absolute),
            "off" => ("off", LineNumberMode::Off),
            _ => ("relative", LineNumberMode::Relative),
        };
        for entry in &self.panes {
            entry.editor.update(cx, |pane, c| {
                pane.set_line_number_mode(mode);
                c.notify();
            });
        }
        cx.global_mut::<crate::settings::Settings>().line_number_mode = name.to_string();
        crate::settings::save_global_setting("line_number_mode", name);
        cx.notify();
    }

    fn set_preview_mode(&mut self, mode: &str, cx: &mut Context<Self>) {
        let (name, pm) = if mode == "paged" {
            ("paged", PreviewMode::Paged)
        } else {
            ("html", PreviewMode::Html)
        };
        cx.set_global(pm);
        if let Some(ref wv) = self.html_webview {
            wv.set_hidden(pm == PreviewMode::Paged);
        }
        cx.global_mut::<crate::settings::Settings>().preview_mode = name.to_string();
        crate::settings::save_global_setting("preview_mode", name);
        self.active_editor().clone().update(cx, |pane, cx| pane.trigger_compile(cx));
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
            self.create_new_note_deferred(None, root, cx);
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
        self.spawn_backlink_build_if_needed(cx);

        // Route through tab management.
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
        self.refocus_editor_pending = false;

        cx.subscribe(&qs, |this, _, event: &QuickSwitchEvent, cx| {
            match event {
                QuickSwitchEvent::Close => {
                    this.quick_switch = None;
                    this.refocus_editor_pending = true;
                    cx.notify();
                }
                QuickSwitchEvent::Open(path) => {
                    this.quick_switch = None;
                    this.refocus_editor_pending = true;
                    cx.notify();
                    this.open_path(path.clone(), cx);
                }
            }
        }).detach();

        self.quick_switch = Some(qs);
        cx.notify();
    }

    fn open_file_picker(
        &mut self,
        _: &OpenFilePicker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Toggle: pressing Ctrl-P again closes the picker.
        if self.file_picker.is_some() {
            self.file_picker = None;
            cx.notify();
            return;
        }
        // Close quick_switch if open so only one picker is visible.
        self.quick_switch = None;

        // Build file list: recency-first, then the rest of the vault.
        let all_files = self.vault.read(cx).files.clone();
        let mut ordered: Vec<_> = self.recent_paths.iter()
            .filter_map(|p| all_files.iter().find(|f| &f.abs_path == p).cloned())
            .collect();
        for f in &all_files {
            if !self.recent_paths.contains(&f.abs_path) {
                ordered.push(f.clone());
            }
        }

        let fp = cx.new(|cx| FilePicker::new(ordered, cx));
        fp.read(cx).focus_handle.clone().focus(window);
        self.refocus_editor_pending = false;

        cx.subscribe(&fp, |this, _, event: &FilePickerEvent, cx| {
            match event {
                FilePickerEvent::Close => {
                    this.file_picker = None;
                    this.refocus_editor_pending = true;
                    cx.notify();
                }
                FilePickerEvent::Open(path) => {
                    this.file_picker = None;
                    this.refocus_editor_pending = true;
                    cx.notify();
                    this.open_path(path.clone(), cx);
                }
            }
        }).detach();

        self.file_picker = Some(fp);
        cx.notify();
    }

    fn open_recent_files(
        &mut self,
        _: &OpenRecentFiles,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Toggle: pressing again closes the panel.
        if self.quick_switch.is_some() {
            self.quick_switch = None;
            cx.notify();
            return;
        }

        // Build a file list of only the recently-opened paths, in recency order.
        // Filter to paths that are still in the vault (or still on disk).
        let all_files = self.vault.read(cx).files.clone();
        let recent: Vec<_> = self.recent_paths.iter()
            .filter_map(|p| all_files.iter().find(|f| &f.abs_path == p).cloned())
            .collect();

        if recent.is_empty() {
            // Nothing to show — open Quick Switch instead so the user still has
            // a useful picker available.
            self.open_quick_switch(&OpenQuickSwitch, window, cx);
            return;
        }

        let qs = cx.new(|cx| QuickSwitch::new(recent, cx));
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
        self.refocus_editor_pending = false;

        cx.subscribe(&panel, |this, _, event: &BacklinkPanelEvent, cx| {
            match event {
                BacklinkPanelEvent::Close => {
                    this.backlinks = None;
                    this.refocus_editor_pending = true;
                    cx.notify();
                }
                BacklinkPanelEvent::Open(path) => {
                    this.backlinks = None;
                    this.refocus_editor_pending = true;
                    cx.notify();
                    this.open_path(path.clone(), cx);
                }
            }
        }).detach();

        self.backlinks = Some(panel);
        cx.notify();
    }

    fn open_outline(
        &mut self,
        _: &OpenOutline,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.outline.is_some() {
            self.outline = None;
            cx.notify();
            return;
        }

        let text = self.active_editor().read(cx).buffer_text();
        let panel = cx.new(|cx| OutlinePanel::new(text, cx));
        panel.read(cx).focus_handle.clone().focus(window);
        // Cancel pending editor refocus so the outline panel keeps focus.
        self.refocus_editor_pending = false;

        cx.subscribe(&panel, |this, _, event: &OutlinePanelEvent, cx| {
            match event {
                OutlinePanelEvent::Close => {
                    this.outline = None;
                    this.refocus_editor_pending = true;
                    cx.notify();
                }
                OutlinePanelEvent::JumpToLine(line) => {
                    this.outline = None;
                    this.refocus_editor_pending = true;
                    cx.notify();
                    let line = *line;
                    this.active_editor().clone().update(cx, |pane, cx| {
                        pane.jump_to_line(line, cx);
                    });
                }
            }
        }).detach();

        self.outline = Some(panel);
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
        self.refocus_editor_pending = false;

        cx.subscribe(&panel, |this, _, event: &VaultSearchEvent, cx| {
            match event {
                VaultSearchEvent::Close => {
                    this.vault_search = None;
                    this.refocus_editor_pending = true;
                    cx.notify();
                }
                VaultSearchEvent::Open(path, line_no) => {
                    this.vault_search = None;
                    this.refocus_editor_pending = true;
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
            "write" | "save-file" => {
                // SaveFile's on_action handler lives on EditorPane, not MainWindow.
                // Call save() directly so focus state doesn't matter.
                self.active_editor().clone().update(cx, |pane, cx| {
                    pane.save(cx);
                    cx.notify();
                });
            }
            "write-quit" => {
                // Save the active editor, then quit — implement directly because the
                // SaveFileAndQuit App-level action is a TODO stub.
                self.active_editor().clone().update(cx, |pane, cx| {
                    pane.save(cx);
                });
                cx.quit();
            }
            "quit" | "quit-force" => cx.quit(),
            "reload" => cx.dispatch_action(&ReloadFile),
            "noh" | "nohlsearch" | "clear-search-highlight" => {
                self.active_editor().clone().update(cx, |pane, cx| {
                    pane.clear_search_highlight();
                    cx.notify();
                });
            }
            "open" | "open-vault" => cx.dispatch_action(&OpenVault),
            "new" | "new-note" => cx.dispatch_action(&NewNote),
            "buffer-next" => cx.dispatch_action(&BufferNext),
            "buffer-previous" => cx.dispatch_action(&BufferPrevious),
            "buffer-close" => cx.dispatch_action(&BufferClose),
            "toggle-sidebar" => cx.dispatch_action(&ToggleSidebar),
            "toggle-zen-mode" => cx.dispatch_action(&ToggleZenMode),
            "toggle-typewriter-mode" => {
                self.active_editor().clone().update(cx, |pane, cx| {
                    pane.toggle_typewriter_mode();
                    cx.notify();
                });
            }
            "toggle-preview-mode" => cx.dispatch_action(&TogglePreviewMode),
            "export-pdf" => cx.dispatch_action(&ExportPdf),
            "open-command-palette" => cx.dispatch_action(&OpenCommandPalette),
            "open-quick-switch" => cx.dispatch_action(&OpenQuickSwitch),
            "open-file-picker" => cx.dispatch_action(&OpenFilePicker),
            "settings" | "open-settings" => cx.dispatch_action(&OpenSettings),
            "open-recent-files" => cx.dispatch_action(&OpenRecentFiles),
            "follow-link" => {
                self.active_editor().clone().update(cx, |pane, cx| {
                    pane.follow_link_at_cursor(cx);
                });
            }
            "vault-search" => cx.dispatch_action(&OpenVaultSearch),
            "open-daily-note" => cx.dispatch_action(&OpenDailyNote),
            "split-pane-vertical" => cx.dispatch_action(&SplitPaneVertical),
            "split-pane-horizontal" => cx.dispatch_action(&SplitPaneHorizontal),
            "close-pane" => cx.dispatch_action(&ClosePane),
            "focus-pane-left"  => cx.dispatch_action(&FocusPaneLeft),
            "focus-pane-right" => cx.dispatch_action(&FocusPaneRight),
            "focus-pane-up"    => cx.dispatch_action(&FocusPaneUp),
            "focus-pane-down"  => cx.dispatch_action(&FocusPaneDown),
            "open-graph-view" => cx.dispatch_action(&OpenGraphView),
            "open-plugin-manager" => cx.dispatch_action(&OpenPluginManager),
            "open-backlinks" => cx.dispatch_action(&OpenBacklinks),
            "open-outline" => cx.dispatch_action(&OpenOutline),
            "open-search" => {
                self.active_editor().clone().update(cx, |pane, cx| {
                    pane.open_search(false);
                    cx.notify();
                });
            }
            "open-replace" => {
                self.active_editor().clone().update(cx, |pane, cx| {
                    pane.open_replace();
                    cx.notify();
                });
            }
            "line-numbers-relative" => self.set_line_number_mode("relative", cx),
            "line-numbers-absolute" => self.set_line_number_mode("absolute", cx),
            "line-numbers-off" => self.set_line_number_mode("off", cx),
            "reload-settings" => {
                let vault_root = self.vault.read(cx).root.clone();
                let new_settings = crate::settings::load_settings(vault_root.as_deref());
                *cx.global_mut::<crate::settings::Settings>() = new_settings;
            }
            "switch-keyboard-mode" => {
                let next = if cx.global::<crate::settings::Settings>().keyboard_mode == "standard" {
                    "helix"
                } else {
                    "standard"
                };
                self.set_keyboard_mode(next, cx);
            }
            "switch-theme" => {
                let next = if cx.global::<crate::settings::Settings>().theme == "ochre" {
                    "oxide"
                } else {
                    "ochre"
                };
                self.set_theme(next, cx);
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

    // ── Story 36: background backlink build ───────────────────────────────────

    /// If `vault.indexing` is true, spawn a background task that builds the
    /// backlink index and calls `finish_backlink_build` when done.
    fn spawn_backlink_build_if_needed(&self, cx: &mut Context<Self>) {
        if !self.vault.read(cx).indexing {
            return;
        }
        let vault = self.vault.clone();
        let files = vault.read(cx).files.clone();
        cx.spawn(async move |_this, cx| {
            let index = cx
                .background_executor()
                .spawn(async move { crate::vault::BacklinkIndex::build(&files) })
                .await;
            cx.update(|cx| {
                vault.update(cx, |vs, cx| {
                    vs.finish_backlink_build(index);
                    cx.notify();
                });
            })
            .ok();
        })
        .detach();
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
                    let entry = CommandEntry::new(id.clone(), name, hint);
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

    /// Reload the active editor's file from disk (`:reload` / `ReloadFile` action).
    fn reload_file(
        &mut self,
        _: &ReloadFile,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.active_editor().clone().update(cx, |pane, cx| {
            pane.reload_from_disk(cx);
        });
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
            self.refocus_editor_pending = true;
            cx.notify();
            return;
        }

        let palette = cx.new(|cx| CommandPalette::new(cx));
        palette.read(cx).focus_handle.clone().focus(window);

        cx.subscribe(&palette, |this, _, event: &PaletteEvent, cx| {
            match event {
                PaletteEvent::Close => {
                    this.palette = None;
                    this.refocus_editor_pending = true;
                    cx.notify();
                }
                PaletteEvent::Execute(id) => {
                    this.palette = None;
                    this.refocus_editor_pending = true;
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

impl MainWindow {
    /// The activity rail — a 40px icon column on the far left that launches
    /// each major surface.  Icons highlight ochre while their surface is open.
    fn render_activity_rail(&self, t: &ThemePalette, cx: &mut Context<Self>) -> gpui::AnyElement {
        // Handlers call the MainWindow method via cx.listener — NOT
        // cx.dispatch_action on a bare App, which falls back to main.rs's
        // no-op global stubs and the clicks go silently dead (shipped bug).
        type Btn = (&'static str, &'static str, bool, &'static str,
            fn(&mut MainWindow, &mut Window, &mut Context<MainWindow>));
        let buttons: [Btn; 6] = [
            ("rail-files", "☰", self.sidebar_visible, "Toggle file sidebar",
                |this, window, cx| this.toggle_sidebar(&ToggleSidebar, window, cx)),
            ("rail-search", "⌕", self.vault_search.is_some(), "Vault search",
                |this, window, cx| this.open_vault_search(&OpenVaultSearch, window, cx)),
            ("rail-graph", "◎", self.graph_view.is_some(), "Graph view",
                |this, window, cx| this.open_graph_view(&OpenGraphView, window, cx)),
            ("rail-outline", "≡", self.outline.is_some(), "Outline",
                |this, window, cx| this.open_outline(&OpenOutline, window, cx)),
            ("rail-backlinks", "↩", self.backlinks.is_some(), "Backlinks",
                |this, window, cx| this.open_backlinks(&OpenBacklinks, window, cx)),
            ("rail-plugins", "⬡", self.plugin_manager.is_some(), "Plugin manager",
                |this, window, cx| this.open_plugin_manager(&OpenPluginManager, window, cx)),
        ];

        let mut rail = div()
            .w(px(40.0))
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(10.0))
            .gap(px(4.0))
            .bg(gpui::rgb(t.bg_base))
            .border_r_1()
            .border_color(gpui::rgb(t.border_subtle));

        fn make_btn(
            (id, glyph, active, label, on_click): Btn,
            t: &ThemePalette,
            cx: &Context<MainWindow>,
        ) -> gpui::Stateful<gpui::Div> {
            let color = if active { t.ochre } else { t.text_subtle };
            let hover_bg = t.bg_hover;
            div()
                .id(id)
                .w(px(30.0))
                .h(px(30.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.0))
                .text_color(gpui::rgb(color))
                .cursor_pointer()
                .hover(move |s| s.bg(gpui::rgb(hover_bg)))
                .on_click(cx.listener(move |this, _event, window, cx| on_click(this, window, cx)))
                .tooltip(move |_window, cx| cx.new(|_| RailTooltip { text: label.into() }).into())
                .child(glyph)
        }

        for btn in buttons {
            rail = rail.child(make_btn(btn, t, cx));
        }

        // Settings pinned to the bottom.
        rail = rail.child(div().flex_1()).child(
            make_btn(
                ("rail-settings", "⚙", self.settings_panel.is_some(), "Settings",
                    |this, window, cx| this.open_settings(&OpenSettings, window, cx)),
                t,
                cx,
            )
            .mb(px(10.0)),
        );

        rail.into_any_element()
    }

    /// First-run welcome pane, shown when no vault is open (`vault.root` is
    /// `None`).  Without it the window renders an empty editor that looks
    /// broken to a new user.  The "Open Vault" button dispatches the existing
    /// `OpenVault` action (folder picker); `Cmd-O` does the same globally.
    fn render_welcome(&self, t: &ThemePalette, cx: &mut Context<Self>) -> gpui::AnyElement {
        let open_btn = div()
            .id("welcome-open-vault")
            .px(px(20.0))
            .py(px(10.0))
            .rounded(px(8.0))
            .bg(gpui::rgb(t.ochre))
            .text_color(gpui::rgb(t.cursor_fg))
            .text_sm()
            .font_family("Menlo")
            .cursor_pointer()
            .hover(|s| s.bg(gpui::rgb(t.ochre_border)))
            .child("Open Vault…  ⌘O")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _window, cx| {
                    cx.stop_propagation();
                    cx.dispatch_action(&OpenVault);
                }),
            );

        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(gpui::rgb(t.bg_panel))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(14.0))
            .child(
                div()
                    .text_color(gpui::rgb(t.text))
                    .text_2xl()
                    .font_family("Menlo")
                    .child("ockr"),
            )
            .child(
                div()
                    .text_color(gpui::rgb(t.text_muted))
                    .text_sm()
                    .font_family("Menlo")
                    .child("A Typst-native note editor."),
            )
            .child(div().h(px(8.0)))
            .child(open_btn)
            .child(
                div()
                    .text_color(gpui::rgb(t.text_faint))
                    .text_xs()
                    .font_family("Menlo")
                    .child("Open any folder of .typ files to begin."),
            )
            .into_any_element()
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.global::<ThemePalette>().clone();
        let preview_mode = cx.try_global::<PreviewMode>().copied().unwrap_or_default();
        // First-run: no vault open → show the welcome pane instead of an empty editor.
        let show_welcome = self.vault.read(cx).root.is_none();

        // Open the command palette if requested by a pane (deferred to render for Window access).
        if self.open_palette_pending {
            self.open_palette_pending = false;
            if self.palette.is_none() {
                let palette = cx.new(|cx| CommandPalette::new(cx));
                palette.read(cx).focus_handle.clone().focus(window);
                cx.subscribe(&palette, |this, _, event: &PaletteEvent, cx| {
                    match event {
                        PaletteEvent::Close => {
                            this.palette = None;
                            this.refocus_editor_pending = true;
                            cx.notify();
                        }
                        PaletteEvent::Execute(id) => {
                            this.palette = None;
                            this.refocus_editor_pending = true;
                            cx.notify();
                            this.handle_palette_execute(id, cx);
                        }
                    }
                }).detach();
                self.palette = Some(palette);
            }
        }

        // Refocus the active editor after any modal (palette, etc.) is dismissed.
        if self.refocus_editor_pending {
            self.active_editor().read(cx).focus_handle.clone().focus(window);
            self.refocus_editor_pending = false;
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

        // Focus the rename modal if it was just created.
        if self.rename_modal_focus_pending {
            if let Some(ref m) = self.rename_modal {
                m.read(cx).focus_handle.clone().focus(window);
            }
            self.rename_modal_focus_pending = false;
        }

        // Focus the settings panel if it was just created.
        if self.settings_panel_focus_pending {
            if let Some(ref p) = self.settings_panel {
                p.read(cx).focus_handle.clone().focus(window);
            }
            self.settings_panel_focus_pending = false;
        }

        // ── Window dimensions ─────────────────────────────────────────────────
        // Use GPUI's viewport_size — always current, never stale, no AppKit query needed.
        let vp = window.viewport_size();
        let content_w = f64::from(vp.width);
        let content_h = f64::from(vp.height);

        // First-render default: give the preview ~45% of the window so the
        // editor and preview share the space, instead of a fixed 420px slab.
        // Stays drag-resizable afterwards.
        if !self.preview_width_set {
            self.preview_width = (content_w as f32 * 0.45).clamp(320.0, 900.0);
            self.preview_width_set = true;
        }

        // Re-render everything on window resize (see `bounds_sub` docs — avoids
        // stale text-measure caches painting overlapping wrapped rows).
        if self.bounds_sub.is_none() {
            self.bounds_sub = Some(cx.observe_window_bounds(window, |this, _, cx| {
                for pane in &this.panes {
                    pane.editor.update(cx, |_, c| c.notify());
                }
                cx.notify();
            }));
        }

        // Any floating GPUI overlay open?  The WKWebView is a native NSView
        // layered above everything GPUI draws, so overlays extending into the
        // preview column get visually clipped at its edge.  Hide the webview
        // while an overlay is up; it returns when the overlay closes (this
        // render runs again).
        let overlay_open = self.palette.is_some()
            || self.quick_switch.is_some()
            || self.file_picker.is_some()
            || self.template_picker.is_some()
            || self.settings_panel.is_some()
            || self.rename_modal.is_some()
            || self.plugin_manager.is_some()
            || self.plugin_panel.is_some()
            || self.graph_view.is_some()
            || self.vault_search.is_some()
            || self.backlinks.is_some()
            || self.outline.is_some();

        // ── WKWebView lifecycle ───────────────────────────────────────────────
        let preview_x = content_w - self.preview_width as f64;
        match preview_mode {
            PreviewMode::Html => {
                if self.html_webview.is_none() {
                    self.html_webview = HtmlWebView::new(self.html_link_sender.clone());
                }
                if let Some(ref wv) = self.html_webview {
                    if self.zen_mode || overlay_open {
                        // Hide the webview in Zen Mode and under overlays.
                        wv.set_hidden(true);
                    } else {
                        wv.update_frame(preview_x, 0.0, self.preview_width as f64, content_h);
                        wv.set_hidden(false);
                    }
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
            .on_action(cx.listener(Self::open_file_picker))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(|this, _: &LineNumbersRelative, _, cx| this.set_line_number_mode("relative", cx)))
            .on_action(cx.listener(|this, _: &LineNumbersAbsolute, _, cx| this.set_line_number_mode("absolute", cx)))
            .on_action(cx.listener(|this, _: &LineNumbersOff, _, cx| this.set_line_number_mode("off", cx)))
            .on_action(cx.listener(Self::open_recent_files))
            .on_action(cx.listener(Self::open_backlinks))
            .on_action(cx.listener(Self::open_outline))
            .on_action(cx.listener(Self::open_vault_search))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::toggle_zen_mode))
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
            .on_action(cx.listener(Self::reload_file))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up));

        // ── Activity rail ─────────────────────────────────────────────────────
        // Unified launcher for the surfaces that were previously scattered
        // across shortcuts and the palette: files, search, graph, outline,
        // backlinks, plugins, settings.  Hidden in Zen Mode.
        if !self.zen_mode {
            root = root.child(self.render_activity_rail(&t, cx));
        }

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

        let rail_w = if self.zen_mode { 0.0 } else { 40.0 };
        let sidebar_w = if self.sidebar_visible { self.sidebar_width + 4.0 } else { 0.0 };
        // In Zen Mode the preview is hidden, so do not subtract preview_width.
        let effective_preview_w = if self.zen_mode { 0.0 } else { self.preview_width + 4.0 };
        let editor_area_w =
            (content_w as f32 - rail_w - sidebar_w - effective_preview_w).max(200.0);

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
                        let group_name = format!("tab-{pane_idx}-{i}");

                        let close_btn = div()
                            .id(("tab-close", i))
                            .w(px(14.0))
                            .h(px(14.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.0))
                            .text_color(gpui::rgb(t.text_faint))
                            .opacity(0.0)
                            .group_hover(group_name.clone(), |s| s.opacity(1.0))
                            .hover(|s| s.bg(gpui::rgb(t.bg_hover)).text_color(gpui::rgb(t.text)))
                            .child("×")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event, window, cx| {
                                    cx.stop_propagation();
                                    this.close_tab_at(pane_idx, i, window, cx);
                                }),
                            );

                        div()
                            .id(("tab", i))
                            .group(group_name)
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
                            .child(close_btn)
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
                    .id("tab-bar")
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_end()
                    .h(px(30.0))
                    .bg(gpui::rgb(t.bg_base))
                    .border_b_1()
                    .border_color(gpui::rgb(t.border_subtle))
                    .overflow_x_scroll()
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

                self.panes[0].editor.update(cx, |p, _| p.set_pane_width(w0));
                self.panes[1].editor.update(cx, |p, _| p.set_pane_width(w1));
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

                self.panes[0].editor.update(cx, |p, _| p.set_pane_width(editor_area_w));
                self.panes[1].editor.update(cx, |p, _| p.set_pane_width(editor_area_w));
                let col0 = pane_col(0, self, cx);
                let col1 = pane_col(1, self, cx);
                div().flex_1().min_w_0().h_full().flex().flex_col()
                    .child(div().w_full().h(px(h0)).overflow_hidden().child(col0))
                    .child(handle(DragTarget::PaneDivider, false, cx))
                    .child(div().w_full().h(px(h1)).overflow_hidden().child(col1))
                    .into_any_element()
            }
            _ => {
                // Zen mode centres the editor at ≤800px; otherwise the pane
                // fills the editor area.
                let w = if self.zen_mode {
                    (content_w as f32).min(800.0)
                } else {
                    editor_area_w
                };
                self.panes[0].editor.update(cx, |p, _| p.set_pane_width(w));
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

        let root = if show_welcome {
            // ── First run: welcome pane, full width, no preview ───────────────
            root.child(self.render_welcome(&t, cx))
        } else if self.zen_mode {
            // ── Zen Mode: centered editor, no preview ─────────────────────────
            // Cap the writing column at 800 px; centre it in the full window.
            let zen_col_w = (content_w as f32).min(800.0);
            let centered = div()
                .flex_1()
                .h_full()
                .flex()
                .flex_row()
                .justify_center()
                .overflow_hidden()
                .child(
                    div()
                        .w(px(zen_col_w))
                        .h_full()
                        .overflow_hidden()
                        .child(editor_area),
                );
            root.child(centered)
        } else {
            root.child(editor_area)
                .child(handle(DragTarget::Preview, true, cx))
                .child(preview_col)
        };

        root.when_some(export_toast, |root, toast| root.child(toast))
            .when_some(self.palette.clone(), |root, p| {
                root.child(gpui::deferred(p).with_priority(100))
            })
            .when_some(self.quick_switch.clone(), |root, qs| {
                root.child(gpui::deferred(qs).with_priority(100))
            })
            .when_some(self.file_picker.clone(), |root, fp| {
                root.child(gpui::deferred(fp).with_priority(100))
            })
            .when_some(self.template_picker.clone(), |root, picker| {
                root.child(gpui::deferred(picker).with_priority(100))
            })
            .when_some(self.backlinks.clone(), |root, panel| {
                root.child(gpui::deferred(panel).with_priority(100))
            })
            .when_some(self.outline.clone(), |root, panel| {
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
            .when_some(self.rename_modal.clone(), |root, m| {
                root.child(gpui::deferred(m).with_priority(170))
            })
            .when_some(self.settings_panel.clone(), |root, p| {
                root.child(gpui::deferred(p).with_priority(170))
            })
    }
}
