//! Plugin registry — GPUI global tracking all loaded plugins.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::panel::RegisteredPanel;
use super::runtime::{CapabilitiesJson, PluginEvent, PluginMetadataJson};

// ── Plugin status ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum PluginStatus {
    Loaded,
    Failed(String),
}

// ── Plugin info snapshot (used by plugin manager UI) ─────────────────────────

#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub capabilities: CapabilitiesJson,
}

impl From<&PluginMetadataJson> for PluginInfo {
    fn from(m: &PluginMetadataJson) -> Self {
        Self {
            id: m.id.clone(),
            name: m.name.clone(),
            version: m.version.clone(),
            capabilities: m.capabilities.clone(),
        }
    }
}

// ── Registry ──────────────────────────────────────────────────────────────────

pub struct PluginRegistry {
    /// plugin_id → list of registered command ids.
    pub plugin_commands: HashMap<String, Vec<String>>,
    /// plugin_id → list of registered panels.
    pub plugin_panels: HashMap<String, Vec<RegisteredPanel>>,
    /// command_id → plugin_id (reverse map for dispatch).
    pub command_to_plugin: HashMap<String, String>,
    /// plugin_id → metadata snapshot (set on load).
    pub plugin_info: HashMap<String, PluginInfo>,
    /// plugin_id → current status.
    pub plugin_statuses: HashMap<String, PluginStatus>,
    pub event_tx: std::sync::mpsc::Sender<PluginEvent>,
    pub event_rx: Arc<Mutex<std::sync::mpsc::Receiver<PluginEvent>>>,
}

impl gpui::Global for PluginRegistry {}

impl PluginRegistry {
    pub fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            plugin_commands: HashMap::new(),
            plugin_panels: HashMap::new(),
            command_to_plugin: HashMap::new(),
            plugin_info: HashMap::new(),
            plugin_statuses: HashMap::new(),
            event_tx: tx,
            event_rx: Arc::new(Mutex::new(rx)),
        }
    }

    /// Record that a plugin loaded successfully (or replace a previous entry).
    pub fn mark_loaded(&mut self, info: PluginInfo) {
        let id = info.id.clone();
        self.plugin_info.insert(id.clone(), info);
        self.plugin_statuses.insert(id, PluginStatus::Loaded);
    }

    /// Record that a plugin failed to load or panicked at runtime.
    pub fn mark_failed(&mut self, plugin_id: &str, reason: String) {
        self.plugin_statuses.insert(plugin_id.to_string(), PluginStatus::Failed(reason));
    }

    /// Remove all tracking data for a set of plugin IDs (called on vault switch).
    pub fn remove_plugins(&mut self, ids: &[String]) {
        for id in ids {
            self.plugin_info.remove(id);
            self.plugin_statuses.remove(id);
            self.plugin_commands.remove(id);
            self.plugin_panels.remove(id);
        }
        self.command_to_plugin.retain(|_, v| !ids.contains(v));
    }
}
