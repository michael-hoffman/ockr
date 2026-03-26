// Compiler API — integrated into the editor in Story 06.

//! typst background compiler.
//!
//! The compiler runs on a dedicated thread and communicates with the UI thread
//! via channels:
//! - `CompilerHandle::send(request)` — send new source text to compile.
//! - Results are delivered by updating the `Entity<CompileOutput>` GPUI entity.
//!
//! Debounce:
//! - The thread always consumes the *latest* queued request before compiling.
//! - A `recv_timeout(80ms)` loop means: compile 80ms after the last keystroke.
//! - This keeps the common typing case fast without burning CPU on every key.
//!
//! Thread safety:
//! - Thread panics inside `typst::compile` are caught with `std::panic::catch_unwind`
//!   and surfaced as `CompileResult::Panicked` without crashing the host process.

pub mod preprocess;
pub mod world;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use typst::layout::PagedDocument;

use self::world::OckrWorld;

// ── Public types ──────────────────────────────────────────────────────────────

/// Shared map of `"@plugin/<id>/<file>"` → typst source text, contributed by plugins.
pub type PluginPackages = Arc<RwLock<HashMap<String, String>>>;

/// Which output format the compiler should produce.
///
/// Stored as a GPUI global so any component can read it without threading it
/// through every call site.  Toggled by `TogglePreviewMode`.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum PreviewMode {
    /// Typst HTML export — faster (no page layout), displayed in WKWebView.
    #[default]
    Html,
    /// Typst paged/PDF export — rasterised to a bitmap and shown in PreviewPane.
    Paged,
}

/// A compilation request sent from the UI thread to the compiler thread.
pub struct CompileRequest {
    /// Pre-processed source text (wikilinks already resolved).
    pub source: String,
    /// Vault root at the time of the request (may change if user opens a new vault).
    pub vault_root: Option<PathBuf>,
    /// Vault-relative path of the file being compiled (e.g. `"notes/foo.typ"`).
    /// Used to set the correct virtual path on the main `FileId` so that
    /// relative imports like `#import "../_template.typ"` resolve correctly.
    pub file_path: Option<String>,
    /// Output format requested.
    pub mode: PreviewMode,
    /// Plugin-provided typst packages. Maps `"@plugin/<name>/lib.typ"` →
    /// source text. Injected by plugins via `ockr_register_typst_package`.
    pub plugin_packages: Option<PluginPackages>,
}

/// A diagnostic produced by the typst compiler.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    #[allow(dead_code)] pub severity: DiagnosticSeverity,
    pub message: String,
    #[allow(dead_code)] pub span_file: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
}

/// The result of one compilation.
#[derive(Clone)]
pub enum CompileResult {
    /// Successful paged compilation — callers can rasterise `doc` for the PDF preview.
    Ok(Arc<PagedDocument>),
    /// Successful HTML compilation — `String` is a complete `<!DOCTYPE html>` document.
    OkHtml(String),
    /// Compilation failed with one or more diagnostics.
    Err(Vec<Diagnostic>),
    /// The compiler thread panicked. The `String` contains the panic message.
    Panicked(String),
}

/// A handle to the background compiler thread.
/// Clone-able; all clones share the same underlying channel.
#[derive(Clone)]
pub struct CompilerHandle {
    tx: std::sync::mpsc::SyncSender<CompileRequest>,
}

impl CompilerHandle {
    /// Send source text for recompilation. Non-blocking: if the channel is
    /// full (previous request not yet consumed), the older request is dropped.
    pub fn send(&self, req: CompileRequest) {
        // Use `try_send`; if the single-slot channel is full, the compiler is
        // busy — the caller can retry or the debounce loop will pick up the
        // next change.
        let _ = self.tx.try_send(req);
    }
}

// ── Thread startup ────────────────────────────────────────────────────────────

/// Spawn the compiler thread.
///
/// `on_result` is called on the compiler thread each time a compilation
/// completes (or panics). Callers typically use this to push a result into
/// a `futures::channel::mpsc` channel that a GPUI spawn task monitors.
pub fn spawn_compiler_thread(
    on_result: impl Fn(CompileResult) + Send + 'static,
) -> CompilerHandle {
    // Bounded channel with capacity 1 — we only need the latest request.
    let (tx, rx) = std::sync::mpsc::sync_channel::<CompileRequest>(1);

    std::thread::Builder::new()
        .name("ockr-compiler".into())
        .spawn(move || {
            compiler_loop(rx, on_result);
        })
        .expect("failed to spawn compiler thread");

    CompilerHandle { tx }
}

// ── Compiler loop ─────────────────────────────────────────────────────────────

fn compiler_loop(
    rx: std::sync::mpsc::Receiver<CompileRequest>,
    on_result: impl Fn(CompileResult),
) {
    let mut world = OckrWorld::new();
    let mut pending: Option<CompileRequest> = None;

    loop {
        // Collect the latest request from the channel, applying an 80ms
        // debounce: keep reading until no new messages arrive within the window.
        match rx.recv_timeout(Duration::from_millis(80)) {
            Ok(req) => {
                // Newer request arrived — replace the pending one and loop.
                pending = Some(req);
                continue;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // 80ms of silence — compile whatever is pending.
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // The handle was dropped; shut down cleanly.
                break;
            }
        }

        let Some(req) = pending.take() else { continue };

        // Apply request to the world.
        if let Some(root) = req.vault_root {
            world.set_vault_root(root);
        }
        world.set_plugin_packages(req.plugin_packages);
        let path = req.file_path.as_deref().unwrap_or("main.typ");
        world.set_source(path, req.source);
        let mode = req.mode;

        // Compile, catching any panics so the thread stays alive.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            compile(&world, mode)
        }));

        let outcome = match result {
            Ok(r) => r,
            Err(payload) => {
                let msg = payload
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| payload.downcast_ref::<String>().map(|s| s.as_str()))
                    .unwrap_or("<unknown panic>")
                    .to_owned();
                CompileResult::Panicked(msg)
            }
        };

        on_result(outcome);
    }
}

fn compile(world: &OckrWorld, mode: PreviewMode) -> CompileResult {
    match mode {
        PreviewMode::Html => compile_html(world),
        PreviewMode::Paged => compile_paged(world),
    }
}

fn compile_paged(world: &OckrWorld) -> CompileResult {
    let warned = typst::compile::<PagedDocument>(world);

    match warned.output {
        Ok(doc) => CompileResult::Ok(Arc::new(doc)),
        Err(errors) => {
            let diags = errors
                .iter()
                .map(|d| Diagnostic {
                    severity: DiagnosticSeverity::Error,
                    message: d.message.to_string(),
                    span_file: None,
                })
                .chain(warned.warnings.iter().map(|w| Diagnostic {
                    severity: DiagnosticSeverity::Warning,
                    message: w.message.to_string(),
                    span_file: None,
                }))
                .collect();
            CompileResult::Err(diags)
        }
    }
}

fn compile_html(world: &OckrWorld) -> CompileResult {
    let warned = typst::compile::<typst_html::HtmlDocument>(world);

    match warned.output {
        Ok(doc) => {
            match typst_html::html(&doc) {
                Ok(html_string) => return CompileResult::OkHtml(html_string),
                Err(extra_diags) => {
                    let diags: Vec<Diagnostic> = extra_diags
                        .iter()
                        .map(|d| Diagnostic {
                            severity: DiagnosticSeverity::Error,
                            message: d.message.to_string(),
                            span_file: None,
                        })
                        .collect();
                    return CompileResult::Err(diags);
                }
            }
        }
        Err(errors) => {
            let diags = errors
                .iter()
                .map(|d| Diagnostic {
                    severity: DiagnosticSeverity::Error,
                    message: d.message.to_string(),
                    span_file: None,
                })
                .chain(warned.warnings.iter().map(|w| Diagnostic {
                    severity: DiagnosticSeverity::Warning,
                    message: w.message.to_string(),
                    span_file: None,
                }))
                .collect();
            CompileResult::Err(diags)
        }
    }
}
