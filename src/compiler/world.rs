//! `OckrWorld` — a minimal implementation of typst's `World` trait.
//!
//! Responsibilities:
//! - Provide the standard library and font book to the typst compiler.
//! - Serve the active document's source text from an in-memory `Source`.
//! - Load binary file resources (images, etc.) from the vault root on disk.
//! - Return the current date for typst's `datetime` function.
//!
//! The `source` method returns the same `Source` object between compilations
//! when the text hasn't changed, which allows comemo to reuse cached partial
//! compilation results (incremental compilation).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::PluginPackages;

use typst::diag::{EcoString, FileError, FileResult};
use typst::foundations::{Bytes, Datetime};
use typst::syntax::{FileId, Source, VirtualPath};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Feature, Features, Library, LibraryExt, World};


/// The in-memory World presented to the typst compiler.
///
/// This struct is `Send + Sync` and can be moved to the compiler background
/// thread. It is never accessed from the UI thread while a compilation is
/// running.
pub struct OckrWorld {
    /// The standard typst library (constructed once, immutable).
    library: LazyHash<Library>,
    /// Font metadata index (constructed once from the loaded fonts).
    book: LazyHash<FontBook>,
    /// All loaded fonts (indexed by `book` position).
    fonts: Vec<Font>,
    /// The virtual FileId for the in-memory main source.
    main_id: FileId,
    /// The active source file (updated via `replace_source`).
    source: Arc<Mutex<Source>>,
    /// Root directory of the vault, used to resolve file references.
    vault_root: Option<PathBuf>,
    /// Binary file cache (vault-relative path → Bytes). Cleared on vault change.
    ///
    /// `Mutex` is needed because `World::file()` takes `&self` but the World
    /// trait requires `Sync`.
    file_cache: Mutex<HashMap<PathBuf, Bytes>>,
    /// Source import cache (vault-relative path → text). Cleared on vault change.
    ///
    /// Imported `.typ` files rarely change during an editing session, so caching
    /// them avoids one `read_to_string` syscall per imported file per compilation.
    source_cache: Mutex<HashMap<PathBuf, String>>,
    /// Plugin-provided typst packages: `"@plugin/<name>/lib.typ"` → source.
    plugin_packages: Option<PluginPackages>,
}

impl OckrWorld {
    /// Construct a world, loading bundled fonts from `typst-assets`.
    ///
    /// Font loading happens once at construction time. This is deliberately
    /// synchronous — it happens on the compiler thread before the first
    /// compilation.
    pub fn new() -> Self {
        let (fonts, book) = load_bundled_fonts();
        // Start with a placeholder id; `set_source` assigns the real one.
        let main_id = FileId::new(None, VirtualPath::new("/main.typ"));
        let source = Source::new(main_id, String::new());

        // Enable the experimental HTML export feature so
        // `typst::compile::<HtmlDocument>()` succeeds without the warning
        // that blocks compilation.
        let library = Library::builder()
            .with_features(Features::from_iter([Feature::Html]))
            .build();

        Self {
            library: LazyHash::new(library),
            book: LazyHash::new(book),
            fonts,
            main_id,
            source: Arc::new(Mutex::new(source)),
            vault_root: None,
            file_cache: Mutex::new(HashMap::new()),
            source_cache: Mutex::new(HashMap::new()),
            plugin_packages: None,
        }
    }

    /// Update the vault root (called when the user opens a vault).
    pub fn set_vault_root(&mut self, root: PathBuf) {
        self.vault_root = Some(root);
        self.file_cache.lock().unwrap().clear();
        self.source_cache.lock().unwrap().clear();
    }

    /// Remove a single entry from the source import cache.
    ///
    /// Called before each compilation for any file that was saved to disk since
    /// the last compile, so the compiler re-reads the updated content.
    pub fn invalidate_source(&mut self, path: &Path) {
        self.source_cache.lock().unwrap().remove(path);
    }

    /// Update the plugin packages map (swapped each compile request).
    pub fn set_plugin_packages(&mut self, packages: Option<PluginPackages>) {
        self.plugin_packages = packages;
    }

    /// Set the source text for the given vault-relative path.
    ///
    /// `vault_rel_path` is a path like `"notes/foo.typ"` relative to the
    /// vault root. It is used to construct a virtual path (`/notes/foo.typ`)
    /// so that relative imports inside the document resolve correctly — e.g.
    /// `#import "../_template.typ"` from `/notes/foo.typ` resolves to
    /// `/_template.typ` → `vault_root/_template.typ`.
    ///
    /// If the path hasn't changed since the last call, the source is updated
    /// in-place so typst's incremental parser can diff the previous parse tree.
    pub fn set_source(&mut self, vault_rel_path: &str, text: String) {
        let vpath = format!("/{}", vault_rel_path.trim_start_matches('/'));
        let new_id = FileId::new(None, VirtualPath::new(&vpath));

        let mut src = self.source.lock().unwrap();
        if new_id == self.main_id {
            // Same file — incremental replace preserves the parse cache.
            src.replace(&text);
        } else {
            // New file — create a fresh Source; incremental cache is cold.
            self.main_id = new_id;
            *src = Source::new(new_id, text);
        }
    }
}

impl World for OckrWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.book
    }

    fn main(&self) -> FileId {
        self.main_id
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main_id {
            return Ok(self.source.lock().unwrap().clone());
        }

        // Check for `@plugin/<name>/...` packages contributed by plugins.
        if let Some(pkg) = id.package() {
            if pkg.namespace.as_str() == "plugin" {
                if let Some(ref pkgs) = self.plugin_packages {
                    let guard = pkgs.read().unwrap();
                    let key = format!(
                        "@plugin/{}/{}",
                        pkg.name,
                        id.vpath().as_rootless_path().display()
                    );
                    if let Some(src) = guard.get(&key) {
                        return Ok(Source::new(id, src.clone()));
                    }
                }
                return Err(FileError::NotFound(
                    id.vpath().as_rootless_path().to_path_buf(),
                ));
            }
        }

        // For imported files, read from disk relative to the vault root.
        let path = resolve_vault_path(&self.vault_root, id)?;
        if let Some(cached) = self.source_cache.lock().unwrap().get(&path).cloned() {
            return Ok(Source::new(id, cached));
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| file_io_error(e, &path))?;
        self.source_cache.lock().unwrap().insert(path, text.clone());
        Ok(Source::new(id, text))
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        let path = resolve_vault_path(&self.vault_root, id)?;
        if let Some(cached) = self.file_cache.lock().unwrap().get(&path).cloned() {
            return Ok(cached);
        }
        let data = std::fs::read(&path)
            .map_err(|e| file_io_error(e, &path))?;
        let bytes = Bytes::new(data);
        self.file_cache.lock().unwrap().insert(path, bytes.clone());
        Ok(bytes)
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.get(index).cloned()
    }

    fn today(&self, offset: Option<i64>) -> Option<Datetime> {
        // Use the `time` crate to get the current local or UTC date.
        let now = if offset.is_none() {
            // Local time — may fail on some platforms; fall back to UTC.
            time::OffsetDateTime::now_local()
                .unwrap_or_else(|_| time::OffsetDateTime::now_utc())
        } else {
            let offset_hours = offset.unwrap_or(0);
            let tz_offset =
                time::UtcOffset::from_hms(offset_hours as i8, 0, 0).unwrap_or(time::UtcOffset::UTC);
            time::OffsetDateTime::now_utc().to_offset(tz_offset)
        };
        Datetime::from_ymd_hms(
            now.year(),
            now.month() as u8,
            now.day(),
            now.hour(),
            now.minute(),
            now.second(),
        )
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Load all bundled fonts from `typst-assets` and build the font book.
fn load_bundled_fonts() -> (Vec<Font>, FontBook) {
    let mut fonts = Vec::new();
    for font_data in typst_assets::fonts() {
        // Each font file may contain multiple faces (e.g. TTC).
        let bytes = Bytes::new(font_data);
        let mut index = 0u32;
        loop {
            match Font::new(bytes.clone(), index) {
                Some(f) => {
                    fonts.push(f);
                    index += 1;
                }
                None => break,
            }
        }
    }
    let book = FontBook::from_fonts(&fonts);
    (fonts, book)
}

/// Resolve a `FileId` to a real path on disk using the vault root.
fn resolve_vault_path(
    vault_root: &Option<PathBuf>,
    id: FileId,
) -> FileResult<PathBuf> {
    let root = vault_root.as_deref().ok_or_else(|| {
        FileError::NotFound(Path::new("<no vault>").to_path_buf())
    })?;
    // The virtual path always starts with '/', strip it.
    let rel = id.vpath().as_rootless_path();
    Ok(root.join(rel))
}

fn file_io_error(e: std::io::Error, path: &Path) -> FileError {
    match e.kind() {
        std::io::ErrorKind::NotFound => FileError::NotFound(path.to_path_buf()),
        std::io::ErrorKind::PermissionDenied => FileError::AccessDenied,
        _ => FileError::Other(Some(EcoString::from(e.to_string()))),
    }
}
