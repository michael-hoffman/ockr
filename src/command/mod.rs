//! Command registry — the authoritative list of every action ockr can perform.
//!
//! Every operation (built-in or future plugin) registers a `CommandEntry` here.
//! The Command Palette (Story 08) is a fuzzy-search UI over this registry.
//! For now, search is a simple substring filter; BM25-weighted fuzzy matching
//! arrives with the full palette in Story 08.
// Mark as a GPUI global so it can be accessed from any App context.
impl gpui::Global for CommandRegistry {}

/// A single registered command.
pub struct CommandEntry {
    /// Kebab-case identifier, e.g. `"open-command-palette"`.
    pub id: String,
    /// Human-readable display name shown in the palette.
    pub name: String,
    /// Keybinding hint for display only (actual binding registered separately).
    pub keybinding_hint: Option<String>,
}

impl CommandEntry {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        keybinding_hint: Option<impl Into<String>>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            keybinding_hint: keybinding_hint.map(|h| h.into()),
        }
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

    /// Remove all commands matching the predicate.
    pub fn remove_where(&mut self, pred: impl Fn(&CommandEntry) -> bool) {
        self.entries.retain(|e| !pred(e));
    }

    pub fn entries(&self) -> &[CommandEntry] {
        &self.entries
    }

    /// Substring search over command names and ids.
    /// Returns indices into `entries()` in registration order.
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
