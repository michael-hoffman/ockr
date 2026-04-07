//! Backlink index — tracks which notes link to which.
//!
//! The index is built once on vault open (full scan) and updated
//! incrementally when a note is saved (`update_file`). All operations are
//! synchronous; at Phase 1 vault sizes (< 10 k notes) this is fast enough
//! to run on the main thread.
//!
//! ## Internals
//!
//! - `incoming`: normalised-title-key → list of `VaultFile`s whose source
//!   contains a `[[link]]` that resolves to that key.
//! - When re-indexing a file, its old outgoing links are removed first so
//!   stale entries do not accumulate.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::vault::VaultFile;

// ── Public types ──────────────────────────────────────────────────────────────

/// An in-memory graph of wikilink edges across the vault.
#[derive(Default, Clone, Serialize, Deserialize)]
pub struct BacklinkIndex {
    /// Maps the **source** file's abs-path to the set of normalised target keys
    /// it links to.  Used to remove stale entries on re-index.
    outgoing: HashMap<PathBuf, HashSet<String>>,

    /// Maps a normalised target key → list of source `VaultFile`s.
    incoming: HashMap<String, Vec<VaultFile>>,
}

impl BacklinkIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Full scan: read every vault file from disk and build the complete index.
    pub fn build(files: &[VaultFile]) -> Self {
        let mut idx = Self::new();
        for file in files {
            if let Ok(content) = std::fs::read_to_string(&file.abs_path) {
                idx.index_file(file, &content, files);
            }
        }
        idx
    }

    /// Incremental update after a single file has been modified.
    /// Call this after each save with the new file content.
    pub fn update_file(&mut self, file: &VaultFile, content: &str, all_files: &[VaultFile]) {
        // Remove old outgoing links from this source first.
        if let Some(old_keys) = self.outgoing.remove(&file.abs_path) {
            for key in &old_keys {
                if let Some(list) = self.incoming.get_mut(key) {
                    list.retain(|f| f.abs_path != file.abs_path);
                }
            }
        }
        self.index_file(file, content, all_files);
    }

    /// Return all notes that link **to** the given vault-relative path.
    /// Returns an empty slice if none found.
    pub fn incoming_links(&self, rel_path: &Path) -> Vec<VaultFile> {
        // Derive the normalised key from the file stem.
        let title = rel_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let key = normalise(title);
        self.incoming
            .get(&key)
            .cloned()
            .unwrap_or_default()
    }

    // ── Private ───────────────────────────────────────────────────────────────

    fn index_file(&mut self, source: &VaultFile, content: &str, all_files: &[VaultFile]) {
        // Build lookup for resolving links (same normalisation as preprocess).
        let file_keys: HashMap<String, &VaultFile> = all_files
            .iter()
            .map(|f| (normalise(&f.title), f))
            .collect();

        let mut new_keys: HashSet<String> = HashSet::new();

        // Extract all [[targets]] from the content.
        let targets = extract_wikilink_targets(content);
        for target in targets {
            let key = normalise(&target);
            if file_keys.contains_key(&key) {
                new_keys.insert(key.clone());
                self.incoming
                    .entry(key)
                    .or_default()
                    .retain(|f| f.abs_path != source.abs_path); // avoid duplicates
                self.incoming
                    .entry(normalise(&target))
                    .or_default()
                    .push(source.clone());
            }
        }

        self.outgoing.insert(source.abs_path.clone(), new_keys);
    }
}

// ── On-disk cache (Story 37) ──────────────────────────────────────────────────

/// Serialisable envelope written to `<vault>/.ockr/backlinks.cache`.
#[derive(Serialize, Deserialize)]
struct BacklinkCacheFile {
    /// Each entry is (abs_path_string, mtime_seconds) for every vault file at
    /// the time the cache was written.  If any entry mismatches the current
    /// file list/mtimes, the cache is rejected and a fresh build is triggered.
    manifest: Vec<(String, u64)>,
    index: BacklinkIndex,
}

fn cache_path(vault_root: &Path) -> PathBuf {
    vault_root.join(".ockr").join("backlinks.cache")
}

/// Compute a stable mtime signature for a slice of `VaultFile`s.
/// Returns `Vec<(abs_path_string, mtime_secs)>` sorted by path.
fn file_manifest(files: &[VaultFile]) -> Vec<(String, u64)> {
    let mut m: Vec<(String, u64)> = files
        .iter()
        .map(|f| {
            let mtime = f
                .abs_path
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            (f.abs_path.to_string_lossy().into_owned(), mtime)
        })
        .collect();
    m.sort_by(|a, b| a.0.cmp(&b.0));
    m
}

/// Try to load a valid cache for `vault_root`.  Returns `Some(index)` only if
/// the cache exists **and** its manifest exactly matches `current_files`.
pub fn try_load_cache(vault_root: &Path, current_files: &[VaultFile]) -> Option<BacklinkIndex> {
    let path = cache_path(vault_root);
    let bytes = std::fs::read(&path).ok()?;
    let cache: BacklinkCacheFile = serde_json::from_slice(&bytes).ok()?;
    let current_manifest = file_manifest(current_files);
    if cache.manifest == current_manifest {
        Some(cache.index)
    } else {
        None
    }
}

/// Persist `index` alongside its file manifest so future starts can skip
/// the full rebuild.  Failures are silently ignored (the app works fine
/// without a cache; the next start will just rebuild).
pub fn save_cache(vault_root: &Path, files: &[VaultFile], index: &BacklinkIndex) {
    let envelope = BacklinkCacheFile {
        manifest: file_manifest(files),
        index: index.clone(),
    };
    if let Ok(json) = serde_json::to_vec(&envelope) {
        let _ = std::fs::write(cache_path(vault_root), json);
    }
}

/// Delete the cache file so the next open triggers a fresh build.
/// Called after an incremental `update_file` so the stored graph stays consistent.
pub fn invalidate_cache(vault_root: &Path) {
    let _ = std::fs::remove_file(cache_path(vault_root));
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Pull the raw link targets out of `[[...]]` and `[[...|display]]` syntax.
fn extract_wikilink_targets(source: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut rest = source;
    while let Some(open) = rest.find("[[") {
        rest = &rest[open + 2..];
        let Some(close) = rest.find("]]") else { break };
        let inner = &rest[..close];
        let target = match inner.find('|') {
            Some(pipe) => inner[..pipe].trim(),
            None => inner.trim(),
        };
        targets.push(target.to_owned());
        rest = &rest[close + 2..];
    }
    targets
}

/// Case-fold + hyphen/underscore → space, collapse whitespace.
/// Must match the normalisation in `compiler::preprocess`.
fn normalise(s: &str) -> String {
    s.to_lowercase()
        .replace(['-', '_'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn vf(title: &str, rel: &str) -> VaultFile {
        VaultFile {
            title: title.to_string(),
            rel_path: PathBuf::from(rel),
            abs_path: PathBuf::from(format!("/vault/{rel}")),
        }
    }

    #[test]
    fn extracts_link_targets() {
        let targets = extract_wikilink_targets("See [[Foo]] and [[Bar|baz]] here.");
        assert_eq!(targets, vec!["Foo", "Bar"]);
    }

    #[test]
    fn incoming_links_found() {
        let alpha = vf("alpha", "alpha.typ");
        let beta = vf("beta", "beta.typ");

        let mut idx = BacklinkIndex::new();
        // beta links to alpha
        idx.index_file(&beta, "See [[Alpha]].", &[alpha.clone(), beta.clone()]);

        let incoming = idx.incoming_links(Path::new("alpha.typ"));
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0].title, "beta");
    }

    #[test]
    fn no_incoming_links_returns_empty() {
        let alpha = vf("alpha", "alpha.typ");
        let idx = BacklinkIndex::new();
        assert!(idx.incoming_links(&alpha.rel_path).is_empty());
    }

    #[test]
    fn update_removes_stale_entries() {
        let alpha = vf("alpha", "alpha.typ");
        let beta = vf("beta", "beta.typ");
        let files = vec![alpha.clone(), beta.clone()];

        let mut idx = BacklinkIndex::new();
        idx.index_file(&beta, "See [[Alpha]].", &files);

        // beta is updated and no longer links to alpha
        idx.update_file(&beta, "No links here.", &files);

        let incoming = idx.incoming_links(Path::new("alpha.typ"));
        assert!(incoming.is_empty(), "stale link should be removed");
    }
}
