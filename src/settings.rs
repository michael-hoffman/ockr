//! User settings — Story 22.
//!
//! Settings are loaded from two TOML files:
//!
//! 1. **Global** — `~/.config/ockr/settings.toml`
//! 2. **Vault** — `<vault>/.ockr/settings.toml`
//!
//! Vault-level values override global values where present.  Both files are
//! optional; missing keys fall back to struct defaults.
//!
//! The resolved `Settings` is stored as a GPUI global and can be read anywhere
//! with `cx.global::<Settings>()`.

use serde::Deserialize;
use std::path::{Path, PathBuf};

// ── Settings struct ──────────────────────────────────────────────────────────

/// Application settings, resolved from global + vault overlays.
///
/// Every field has a sensible default so the app works with no config files.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Keyboard mode: `"helix"` (default) or `"standard"`.
    pub keyboard_mode: String,
    /// Theme name (stem of a TOML file in `themes/` or `~/.config/ockr/themes/`).
    pub theme: String,
    /// Line number display: `"relative"`, `"absolute"`, or `"off"`.
    pub line_number_mode: String,
    /// Preview output format: `"html"` or `"paged"`.
    pub preview_mode: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            keyboard_mode: "helix".into(),
            theme: "oxide".into(),
            line_number_mode: "relative".into(),
            preview_mode: "html".into(),
        }
    }
}

impl gpui::Global for Settings {}

// ── Partial overlay ──────────────────────────────────────────────────────────

/// Same shape as `Settings` but every field is `Option`, allowing a TOML file
/// to override only the keys it cares about.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct SettingsOverlay {
    keyboard_mode: Option<String>,
    theme: Option<String>,
    line_number_mode: Option<String>,
    preview_mode: Option<String>,
}

impl Settings {
    /// Merge an overlay on top of self, replacing only the fields that are `Some`.
    fn merge(&mut self, overlay: SettingsOverlay) {
        if let Some(v) = overlay.keyboard_mode { self.keyboard_mode = v; }
        if let Some(v) = overlay.theme { self.theme = v; }
        if let Some(v) = overlay.line_number_mode { self.line_number_mode = v; }
        if let Some(v) = overlay.preview_mode { self.preview_mode = v; }
    }
}

// ── Loading ──────────────────────────────────────────────────────────────────

/// Path to the global settings file.
pub fn global_settings_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("ockr")
        .join("settings.toml")
}

/// Path to the vault-level settings file.
pub fn vault_settings_path(vault_root: &Path) -> PathBuf {
    vault_root.join(".ockr").join("settings.toml")
}

/// Load and merge settings: defaults ← global file ← vault file.
pub fn load_settings(vault_root: Option<&Path>) -> Settings {
    let mut settings = Settings::default();

    // Layer 1: global config.
    if let Some(overlay) = load_overlay(&global_settings_path()) {
        settings.merge(overlay);
    }

    // Layer 2: vault-level override.
    if let Some(root) = vault_root {
        if let Some(overlay) = load_overlay(&vault_settings_path(root)) {
            settings.merge(overlay);
        }
    }

    settings
}

/// Try to read and parse a TOML file into a `SettingsOverlay`.
/// Returns `None` if the file doesn't exist or can't be parsed.
fn load_overlay(path: &Path) -> Option<SettingsOverlay> {
    let text = std::fs::read_to_string(path).ok()?;
    match toml::from_str(&text) {
        Ok(overlay) => Some(overlay),
        Err(e) => {
            eprintln!("[ockr settings] error parsing {}: {e}", path.display());
            None
        }
    }
}

// ── Persistence ──────────────────────────────────────────────────────────────

/// Write a single key-value pair to the global settings file.
///
/// This does a read-modify-write: loads the existing file (or empty), updates
/// the key, and writes it back. The key must be a top-level TOML key.
pub fn save_global_setting(key: &str, value: &str) {
    let path = global_settings_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc = existing.parse::<toml::Table>().unwrap_or_default();
    // Try to parse as a native TOML value; fall back to string.
    let toml_value = if let Ok(v) = value.parse::<toml::Value>() {
        v
    } else {
        toml::Value::String(value.to_string())
    };
    doc.insert(key.to_string(), toml_value);
    let text = toml::to_string_pretty(&doc).unwrap_or_default();
    let _ = std::fs::write(&path, text);
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let s = Settings::default();
        assert_eq!(s.keyboard_mode, "helix");
        assert_eq!(s.theme, "oxide");
    }

    #[test]
    fn overlay_merges_partial() {
        let mut s = Settings::default();
        let overlay = SettingsOverlay {
            theme: Some("ochre".into()),
            ..Default::default()
        };
        s.merge(overlay);
        assert_eq!(s.theme, "ochre");
        // unchanged
        assert_eq!(s.keyboard_mode, "helix");
    }

    #[test]
    fn empty_toml_gives_defaults() {
        let s: Settings = toml::from_str("").unwrap();
        assert_eq!(s.keyboard_mode, "helix");
        assert_eq!(s.theme, "oxide");
    }

    #[test]
    fn partial_toml_only_overrides_present() {
        let overlay: SettingsOverlay = toml::from_str("theme = \"ochre\"").unwrap();
        assert_eq!(overlay.theme, Some("ochre".into()));
        assert!(overlay.keyboard_mode.is_none());
    }
}
