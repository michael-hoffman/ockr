//! Session persistence — save and restore vault path and open tabs.
//!
//! The session file lives at `~/.local/share/ockr/session.json` (XDG-style).
//! All saves are best-effort: I/O errors are silently ignored so they never
//! crash the app.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default)]
struct Session {
    last_vault: Option<PathBuf>,
    /// Absolute paths of open tabs, in order, at the time of last save.
    #[serde(default)]
    open_tabs: Vec<PathBuf>,
    /// Index of the active tab within `open_tabs`.
    #[serde(default)]
    active_tab: usize,
}

fn session_path() -> Option<PathBuf> {
    // XDG_DATA_HOME or ~/.local/share
    let data_home = std::env::var("XDG_DATA_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".local").join("share"))
        })?;
    Some(data_home.join("ockr").join("session.json"))
}

fn write_session(session: &Session) {
    let Some(p) = session_path() else { return };
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(session) {
        let _ = std::fs::write(&p, json);
    }
}

fn read_session() -> Session {
    let Some(p) = session_path() else { return Session::default() };
    let Ok(json) = std::fs::read_to_string(&p) else { return Session::default() };
    serde_json::from_str(&json).unwrap_or_default()
}

/// Save the last-opened vault path. Preserves existing tab data.
pub fn save_last_vault(path: &PathBuf) {
    let mut session = read_session();
    session.last_vault = Some(path.clone());
    // When the vault changes, clear stale tab paths.
    session.open_tabs.clear();
    session.active_tab = 0;
    write_session(&session);
}

/// Save the current set of open tabs for the active vault.
pub fn save_open_tabs(tabs: &[PathBuf], active_tab: usize) {
    let mut session = read_session();
    session.open_tabs = tabs.to_vec();
    session.active_tab = active_tab.min(tabs.len().saturating_sub(1));
    write_session(&session);
}

/// Load the last-opened vault path, if any. Returns `None` if no session
/// file exists or the directory no longer exists.
pub fn load_last_vault() -> Option<PathBuf> {
    let session = read_session();
    session.last_vault.filter(|p| p.is_dir())
}

/// Load the previously open tabs for the current session.
///
/// Only returns paths that still exist on disk.
pub fn load_open_tabs() -> (Vec<PathBuf>, usize) {
    let session = read_session();
    let tabs: Vec<PathBuf> = session.open_tabs
        .into_iter()
        .filter(|p| p.exists())
        .collect();
    let active = session.active_tab.min(tabs.len().saturating_sub(1));
    (tabs, active)
}
