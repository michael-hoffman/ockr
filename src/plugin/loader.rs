//! Plugin loader — lockfile (ockr.lock), install, update.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// ── Lockfile types ────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Lockfile {
    #[serde(default)]
    pub plugins: Vec<LockfileEntry>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LockfileEntry {
    pub id: String,
    pub url: String,
    pub sha256: String,
    pub version: String,
}

impl Lockfile {
    pub fn load(vault_root: &Path) -> Self {
        let path = lockfile_path(vault_root);
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, vault_root: &Path) {
        let path = lockfile_path(vault_root);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(s) = toml::to_string_pretty(self) {
            let _ = std::fs::write(&path, s);
        }
    }
}

fn lockfile_path(vault_root: &Path) -> PathBuf {
    vault_root.join("ockr.lock")
}

fn plugin_dir(vault_root: &Path) -> PathBuf {
    vault_root.join(".ockr").join("plugins")
}

// ── SHA-256 verification ──────────────────────────────────────────────────────

pub fn verify_sha256(bytes: &[u8], expected: &str) -> Result<(), String> {
    let actual = hex::encode(Sha256::digest(bytes));
    if actual == expected {
        Ok(())
    } else {
        Err(format!("SHA-256 mismatch: expected {expected}, got {actual}"))
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

// ── install_plugin ────────────────────────────────────────────────────────────

/// Download a WASM plugin from `url`, verify / store, and upsert ockr.lock.
pub fn install_plugin(vault_root: &Path, url: &str) -> Result<LockfileEntry, String> {
    let bytes = fetch_bytes(url)?;
    let sha = sha256_hex(&bytes);

    // Instantiate briefly to read metadata.
    let meta = read_wasm_metadata(&bytes)?;

    let dir = plugin_dir(vault_root);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let wasm_path = dir.join(format!("{}.wasm", meta.id));
    std::fs::write(&wasm_path, &bytes).map_err(|e| e.to_string())?;

    let entry = LockfileEntry {
        id: meta.id.clone(),
        url: url.to_string(),
        sha256: sha,
        version: meta.version,
    };

    let mut lock = Lockfile::load(vault_root);
    lock.plugins.retain(|e| e.id != meta.id);
    lock.plugins.push(entry.clone());
    lock.save(vault_root);

    println!("Installed plugin '{}' v{}", entry.id, entry.version);
    Ok(entry)
}

// ── update_plugins ────────────────────────────────────────────────────────────

pub fn update_plugins(vault_root: &Path) -> Result<(), String> {
    let mut lock = Lockfile::load(vault_root);
    let dir = plugin_dir(vault_root);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    for entry in &mut lock.plugins {
        let bytes = match fetch_bytes(&entry.url) {
            Ok(b) => b,
            Err(e) => { eprintln!("Failed to fetch plugin '{}': {}", entry.id, e); continue; }
        };
        let new_sha = sha256_hex(&bytes);
        if new_sha == entry.sha256 {
            println!("Plugin '{}' is up to date", entry.id);
            continue;
        }
        let meta = match read_wasm_metadata(&bytes) {
            Ok(m) => m,
            Err(e) => { eprintln!("Failed to read metadata for '{}': {}", entry.id, e); continue; }
        };
        let wasm_path = dir.join(format!("{}.wasm", entry.id));
        if let Err(e) = std::fs::write(&wasm_path, &bytes) {
            eprintln!("Failed to write plugin '{}': {}", entry.id, e);
            continue;
        }
        entry.sha256 = new_sha;
        entry.version = meta.version.clone();
        println!("Updated plugin '{}' → v{}", entry.id, meta.version);
    }

    lock.save(vault_root);
    Ok(())
}

// ── load_vault_plugins ────────────────────────────────────────────────────────

/// Load all installed plugin WASM bytes from the vault's `.ockr/plugins/` dir.
pub fn load_vault_plugins(vault_root: &Path) -> Vec<(LockfileEntry, Vec<u8>)> {
    let lock = Lockfile::load(vault_root);
    let dir = plugin_dir(vault_root);
    let mut result = Vec::new();

    for entry in &lock.plugins {
        let path = dir.join(format!("{}.wasm", entry.id));
        match std::fs::read(&path) {
            Ok(bytes) => {
                // Verify sha256.
                if verify_sha256(&bytes, &entry.sha256).is_ok() {
                    result.push((entry.clone(), bytes));
                } else {
                    eprintln!("SHA-256 mismatch for plugin '{}', skipping", entry.id);
                }
            }
            Err(e) => {
                eprintln!("Failed to read plugin '{}': {}", entry.id, e);
            }
        }
    }
    result
}

// ── Vault root detection ──────────────────────────────────────────────────────

/// Walk up from CWD looking for a directory that contains `ockr.lock` or `.ockr/`.
pub fn detect_vault_root() -> Option<std::path::PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let mut dir = cwd.as_path();
    loop {
        if dir.join("ockr.lock").exists() || dir.join(".ockr").exists() {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    // Support file:// for local testing.
    if let Some(path) = url.strip_prefix("file://") {
        return std::fs::read(path).map_err(|e| e.to_string());
    }
    reqwest::blocking::get(url)
        .map_err(|e| e.to_string())?
        .bytes()
        .map_err(|e| e.to_string())
        .map(|b| b.to_vec())
}

fn read_wasm_metadata(wasm: &[u8]) -> Result<super::runtime::PluginMetadataJson, String> {
    let engine = wasmtime::Engine::default();
    super::runtime::PluginInstance::probe_metadata(&engine, wasm)
}
