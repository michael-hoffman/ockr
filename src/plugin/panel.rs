//! Plugin panel layout types.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PanelPosition {
    Sidebar,
    Bottom,
    Float,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum LayoutItem {
    Text { content: String },
    Button { label: String, command_id: String },
    List { items: Vec<String> },
    Divider,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginLayout {
    pub items: Vec<LayoutItem>,
}

#[derive(Debug, Clone)]
pub struct RegisteredPanel {
    pub plugin_id: String,
    pub panel_id: String,
    pub title: String,
    pub position: PanelPosition,
    pub layout: PluginLayout,
}
