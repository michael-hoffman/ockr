//! Vault abstraction — a directory of `.typ` files treated as a knowledge base.
//!
//! A vault is the top-level container for all notes. ockr adds a single
//! `.ockr/` directory inside the vault for its own metadata (index, backlinks,
//! lockfile, settings). Everything else is plain `.typ` files.

pub mod backlinks;

use std::path::{Path, PathBuf};

pub use backlinks::BacklinkIndex;

/// A single note file within the vault.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VaultFile {
    /// Vault-relative path, e.g. `"notes/my-note.typ"`.
    pub rel_path: PathBuf,
    /// Absolute path on disk.
    pub abs_path: PathBuf,
    /// Display name (stem of the filename, no extension).
    pub title: String,
}

/// The open vault.
///
/// Reactive: storing this in a GPUI `Entity<VaultState>` means any view that
/// reads it will be re-rendered when the vault is opened or files are added/removed.
pub struct VaultState {
    /// Root directory of the vault. `None` if no vault is open.
    pub root: Option<PathBuf>,
    /// All `.typ` files in the vault, sorted by title.
    pub files: Vec<VaultFile>,
    /// In-memory backlink graph. Updated on open and on each file save.
    pub backlinks: BacklinkIndex,
    /// `true` while the backlink index is being built on a background thread.
    /// The sidebar may show an "Indexing…" hint during this period.
    pub indexing: bool,
}

impl VaultState {
    pub fn empty() -> Self {
        Self {
            root: None,
            files: Vec::new(),
            backlinks: BacklinkIndex::new(),
            indexing: false,
        }
    }

    /// Fast open: scan files and check the on-disk backlink cache.
    ///
    /// If the cache is fresh (all file mtimes match), load it synchronously and
    /// return with `indexing = false`.  Otherwise return with an empty index and
    /// `indexing = true`; the caller is responsible for kicking off a background
    /// build via [`finish_backlink_build`].
    pub fn open(root: PathBuf) -> Self {
        let ockr_dir = root.join(".ockr");
        if !ockr_dir.exists() {
            let _ = std::fs::create_dir_all(&ockr_dir);
        }

        let files = scan_for_typ_files(&root);

        // Try the on-disk cache first (Story 37).
        if let Some(index) = backlinks::try_load_cache(&root, &files) {
            return Self {
                root: Some(root),
                files,
                backlinks: index,
                indexing: false,
            };
        }

        // Cache miss — return immediately with an empty index; the background
        // build will call `finish_backlink_build` when done.
        Self {
            root: Some(root),
            files,
            backlinks: BacklinkIndex::new(),
            indexing: true,
        }
    }

    /// Called by the background build task when indexing is complete.
    /// Stores the finished index and writes the on-disk cache.
    pub fn finish_backlink_build(&mut self, index: BacklinkIndex) {
        if let Some(ref root) = self.root {
            backlinks::save_cache(root, &self.files, &index);
        }
        self.backlinks = index;
        self.indexing = false;
    }

    /// Re-index a single file after it has been saved.
    /// `content` is the new raw source (before wikilink preprocessing).
    /// Also invalidates the on-disk cache so the next startup does a fresh build.
    pub fn reindex_file(&mut self, file: &VaultFile, content: &str) {
        let files = self.files.clone();
        self.backlinks.update_file(file, content, &files);
        // Invalidate the cache — it is now stale for this file.
        if let Some(ref root) = self.root {
            backlinks::invalidate_cache(root);
        }
    }
}

/// Recursively collect all `.typ` files under `root`, excluding the `.ockr/`
/// metadata directory. Files are sorted by their vault-relative title.
pub fn scan_for_typ_files(root: &Path) -> Vec<VaultFile> {
    let mut files = Vec::new();
    collect_typ_files(root, root, &mut files);
    files.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    files
}

fn collect_typ_files(root: &Path, dir: &Path, out: &mut Vec<VaultFile>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let abs = entry.path();
        // Skip hidden directories and the .ockr metadata directory.
        let name = match abs.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_owned(),
            None => continue,
        };
        if name.starts_with('.') {
            continue;
        }
        if abs.is_dir() {
            collect_typ_files(root, &abs, out);
        } else if abs.extension().and_then(|e| e.to_str()) == Some("typ") {
            let rel_path = abs.strip_prefix(root).unwrap_or(&abs).to_path_buf();
            let title = abs
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("untitled")
                .to_owned();
            out.push(VaultFile {
                rel_path,
                abs_path: abs,
                title,
            });
        }
    }
}
