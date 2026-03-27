//! Undo/redo history persistence — save and restore per-file undo stacks.
//!
//! History is stored in `~/.local/share/ockr/undo/<hex-hash>.json`.
//! The hash is a simple FNV-1a hash of the file's absolute path string,
//! giving a stable short filename without path separators.
//!
//! Only the most recent `MAX_PERSISTED` entries are written to keep file
//! sizes small. On load, missing files return empty stacks silently.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::editor::state::Pos;

/// Maximum undo/redo entries written per file.
const MAX_PERSISTED: usize = 50;

#[derive(Serialize, Deserialize)]
struct UndoEntry {
    text: String,
    line: usize,
    col: usize,
}

#[derive(Serialize, Deserialize, Default)]
struct UndoFile {
    undo: Vec<UndoEntry>,
    redo: Vec<UndoEntry>,
}

// ── Path helpers ──────────────────────────────────────────────────────────────

fn undo_dir() -> Option<PathBuf> {
    let data_home = std::env::var("XDG_DATA_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".local").join("share"))
        })?;
    Some(data_home.join("ockr").join("undo"))
}

/// FNV-1a 64-bit hash of a string — stable, dependency-free.
fn fnv1a(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in s.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn undo_path_for(file: &Path) -> Option<PathBuf> {
    let hash = fnv1a(&file.to_string_lossy());
    Some(undo_dir()?.join(format!("{hash:016x}.json")))
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Persist the undo and redo stacks for `file_path`.
///
/// Silently ignores I/O errors — undo persistence is best-effort.
pub fn save_undo_history(
    file_path: &Path,
    undo: &[(String, Pos)],
    redo: &[(String, Pos)],
) {
    let Some(p) = undo_path_for(file_path) else { return };
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let to_entries = |stack: &[(String, Pos)]| -> Vec<UndoEntry> {
        // Keep only the tail (most recent entries).
        stack
            .iter()
            .rev()
            .take(MAX_PERSISTED)
            .rev()
            .map(|(text, pos)| UndoEntry { text: text.clone(), line: pos.line, col: pos.col })
            .collect()
    };

    let file = UndoFile { undo: to_entries(undo), redo: to_entries(redo) };
    if let Ok(json) = serde_json::to_string(&file) {
        let _ = std::fs::write(&p, json);
    }
}

/// Load the persisted undo and redo stacks for `file_path`.
///
/// Returns empty vecs if no history file exists or parsing fails.
pub fn load_undo_history(file_path: &Path) -> (Vec<(String, Pos)>, Vec<(String, Pos)>) {
    let Some(p) = undo_path_for(file_path) else { return (vec![], vec![]) };
    let Ok(json) = std::fs::read_to_string(&p) else { return (vec![], vec![]) };
    let Ok(file) = serde_json::from_str::<UndoFile>(&json) else { return (vec![], vec![]) };

    let to_stack = |entries: Vec<UndoEntry>| -> Vec<(String, Pos)> {
        entries
            .into_iter()
            .map(|e| (e.text, Pos::new(e.line, e.col)))
            .collect()
    };

    (to_stack(file.undo), to_stack(file.redo))
}
