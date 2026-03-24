//! Plugin registry — GPUI global tracking all loaded plugins.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::panel::RegisteredPanel;
use super::runtime::PluginEvent;

pub struct PluginRegistry {
    /// plugin_id → list of registered command ids.
    pub plugin_commands: HashMap<String, Vec<String>>,
    /// plugin_id → list of registered panels.
    pub plugin_panels: HashMap<String, Vec<RegisteredPanel>>,
    /// command_id → plugin_id (reverse map for dispatch).
    pub command_to_plugin: HashMap<String, String>,
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
            event_tx: tx,
            event_rx: Arc::new(Mutex::new(rx)),
        }
    }
}
