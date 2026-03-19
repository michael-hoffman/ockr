//! Command registry — the authoritative list of every action ockr can perform.
//!
//! Every operation (built-in or future plugin) registers a `CommandEntry` here.
//! The Command Palette (Story 08) is a fuzzy-search UI over this registry.
//! For now, search is a simple substring filter; BM25-weighted fuzzy matching
//! arrives with the full palette in Story 08.
// Fields and methods form the Command Palette API surface (Story 08).
#![allow(dead_code)]

use gpui::App;

// Mark as a GPUI global so it can be accessed from any App context.
impl gpui::Global for CommandRegistry {}

/// A single registered command.
pub struct CommandEntry {
    /// Kebab-case identifier, e.g. `"open-command-palette"`.
    pub id: &'static str,
    /// Human-readable display name shown in the palette.
    pub name: &'static str,
    /// Keybinding hint for display only (actual binding registered separately).
    pub keybinding_hint: Option<&'static str>,
    handler: Box<dyn Fn(&mut App) + 'static>,
}

impl CommandEntry {
    pub fn new(
        id: &'static str,
        name: &'static str,
        keybinding_hint: Option<&'static str>,
        handler: impl Fn(&mut App) + 'static,
    ) -> Self {
        Self {
            id,
            name,
            keybinding_hint,
            handler: Box::new(handler),
        }
    }

    pub fn invoke(&self, cx: &mut App) {
        (self.handler)(cx);
    }
}

pub struct CommandRegistry {
    entries: Vec<CommandEntry>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn register(&mut self, entry: CommandEntry) {
        self.entries.push(entry);
    }

    pub fn entries(&self) -> &[CommandEntry] {
        &self.entries
    }

    /// Substring search over command names and ids.
    /// Returns indices into `entries()` in registration order.
    ///
    /// Story 08 replaces this with BM25-weighted fuzzy matching (Tantivy or
    /// an in-memory equivalent) with typo tolerance and recency scoring.
    pub fn search(&self, query: &str) -> Vec<usize> {
        if query.is_empty() {
            return (0..self.entries.len()).collect();
        }
        let q = query.to_lowercase();
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                e.name.to_lowercase().contains(&q) || e.id.to_lowercase().contains(&q)
            })
            .map(|(i, _)| i)
            .collect()
    }
}
