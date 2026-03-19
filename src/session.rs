//! Session persistence — save and restore the last-opened vault path.
//!
//! The session file lives at `~/.local/share/ockr/session.json` (XDG-style).
//! We do not use a crate dependency for this simple case; std I/O is sufficient.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default)]
struct Session {
    last_vault: Option<PathBuf>,
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

/// Save the last-opened vault path. Silently ignores I/O errors — session
/// persistence is best-effort and should never crash the app.
pub fn save_last_vault(path: &PathBuf) {
    let Some(p) = session_path() else { return };
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let session = Session {
        last_vault: Some(path.clone()),
    };
    if let Ok(json) = serde_json::to_string_pretty(&session) {
        let _ = std::fs::write(&p, json);
    }
}

/// Load the last-opened vault path, if any. Returns `None` if no session
/// file exists or parsing fails.
pub fn load_last_vault() -> Option<PathBuf> {
    let p = session_path()?;
    let json = std::fs::read_to_string(&p).ok()?;
    let session: Session = serde_json::from_str(&json).ok()?;
    // Only return the path if the directory still exists on disk.
    session
        .last_vault
        .filter(|p| p.is_dir())
}
