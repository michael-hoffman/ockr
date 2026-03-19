//! Vault abstraction — a directory of `.typ` files treated as a knowledge base.
//!
//! A vault is the top-level container for all notes. ockr adds a single
//! `.ockr/` directory inside the vault for its own metadata (index, backlinks,
//! lockfile, settings). Everything else is plain `.typ` files.

use std::path::{Path, PathBuf};

/// A single note file within the vault.
#[derive(Debug, Clone, PartialEq, Eq)]
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
}

impl VaultState {
    pub fn empty() -> Self {
        Self {
            root: None,
            files: Vec::new(),
        }
    }

    /// Open a directory as a vault: create the `.ockr/` metadata directory if
    /// absent and scan for `.typ` files.
    pub fn open(root: PathBuf) -> Self {
        let ockr_dir = root.join(".ockr");
        if !ockr_dir.exists() {
            // Best-effort: if we can't create the dir, we carry on anyway.
            let _ = std::fs::create_dir_all(&ockr_dir);
        }

        let files = scan_for_typ_files(&root);
        Self {
            root: Some(root),
            files,
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
