//! Template Picker — choose a note template when creating a new note (Cmd-N).
//!
//! Displays a floating list of templates found in `.ockr/templates/`.  A
//! "Blank note" option is always shown at the top.  If the vault has no
//! templates the picker is skipped entirely and a blank note is created
//! directly.
//!
//! Navigation: arrow keys or Ctrl-J/K.  Enter picks; Escape dismisses.

use std::path::PathBuf;

use gpui::{
    Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent,
    Render, Window, div, prelude::*, px,
};

use crate::ui::theme::ThemePalette;

// ── Data ──────────────────────────────────────────────────────────────────────

/// A single entry in the template list.
#[derive(Debug, Clone)]
pub struct TemplateEntry {
    /// Display name (template file stem, or "Blank note").
    pub name: String,
    /// Absolute path to the template file.  `None` for the blank-note option.
    pub path: Option<PathBuf>,
}

// ── Events ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum TemplatePickerEvent {
    /// User dismissed without picking.
    Close,
    /// User picked a template (or `None` = blank note).
    Pick(Option<PathBuf>),
}

impl EventEmitter<TemplatePickerEvent> for TemplatePicker {}

// ── View ──────────────────────────────────────────────────────────────────────

pub struct TemplatePicker {
    pub focus_handle: FocusHandle,
    query: String,
    /// Indices into `self.entries` ordered by match quality.
    matches: Vec<usize>,
    selected: usize,
    entries: Vec<TemplateEntry>,
}

impl TemplatePicker {
    /// Create a new picker.
    ///
    /// `templates` is the list of available template files.  A "Blank note"
    /// entry is always prepended.
    pub fn new(templates: Vec<TemplateEntry>, cx: &mut Context<Self>) -> Self {
        let mut entries = Vec::with_capacity(templates.len() + 1);
        // Blank note is always first.
        entries.push(TemplateEntry { name: "Blank note".to_string(), path: None });
        entries.extend(templates);

        let mut picker = Self {
            focus_handle: cx.focus_handle(),
            query: String::new(),
            matches: Vec::new(),
            selected: 0,
            entries,
        };
        picker.refresh_matches();
        picker
    }

    fn refresh_matches(&mut self) {
        let q = self.query.to_lowercase();
        let matches: Vec<usize> = if q.is_empty() {
            (0..self.entries.len()).collect()
        } else {
            self.entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.name.to_lowercase().contains(&q))
                .map(|(i, _)| i)
                .collect()
        };
        self.matches = matches;
        if self.selected >= self.matches.len() {
            self.selected = self.matches.len().saturating_sub(1);
        }
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        let path = self.matches
            .get(self.selected)
            .and_then(|&idx| self.entries.get(idx))
            .and_then(|e| e.path.clone());
        cx.emit(TemplatePickerEvent::Pick(path));
    }

    fn handle_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let k = &event.keystroke;

        if k.key == "escape" {
            cx.emit(TemplatePickerEvent::Close);
            return;
        }

        if k.key == "enter" {
            self.confirm(cx);
            return;
        }

        if k.key == "up" || (k.modifiers.control && k.key == "k") {
            self.selected = self.selected.saturating_sub(1);
            cx.notify();
            return;
        }

        if k.key == "down" || (k.modifiers.control && k.key == "j") {
            if !self.matches.is_empty() && self.selected + 1 < self.matches.len() {
                self.selected += 1;
            }
            cx.notify();
            return;
        }

        if k.key == "backspace" {
            self.query.pop();
            self.refresh_matches();
            cx.notify();
            return;
        }

        if let Some(ch) = &k.key_char {
            if !k.modifiers.control && !k.modifiers.platform {
                self.query.push_str(ch);
                self.refresh_matches();
                cx.notify();
            }
        }
    }

    fn handle_row_click(
        &mut self,
        idx: usize,
        _event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected = self.matches.iter().position(|&i| i == idx).unwrap_or(0);
        self.confirm(cx);
    }
}

impl Focusable for TemplatePicker {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

impl Render for TemplatePicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.global::<ThemePalette>().clone();
        let mut rows = Vec::with_capacity(self.matches.len().min(16));

        for (row_idx, &entry_idx) in self.matches.iter().take(16).enumerate() {
            let entry = &self.entries[entry_idx];
            let is_selected = row_idx == self.selected;
            let is_blank = entry.path.is_none();

            let bg = if is_selected {
                gpui::rgb(t.bg_hover)
            } else {
                gpui::rgb(t.bg_surface)
            };
            let accent = if is_selected {
                gpui::rgb(t.ochre)
            } else {
                gpui::rgb(t.border_subtle)
            };
            let name_color = if is_selected {
                gpui::rgb(t.text)
            } else {
                gpui::rgb(t.text_muted)
            };
            let tag_color = if is_blank {
                gpui::rgb(t.text_faint)
            } else {
                gpui::rgb(t.ochre_dim)
            };
            let tag_text = if is_blank { "blank" } else { "template" };

            let name = entry.name.clone();
            let row = div()
                .flex()
                .flex_row()
                .items_center()
                .px(px(16.0))
                .py(px(8.0))
                .bg(bg)
                .border_l(px(2.0))
                .border_color(accent)
                .gap(px(10.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event, window, cx| {
                        this.handle_row_click(entry_idx, event, window, cx);
                    }),
                )
                .child(
                    div()
                        .text_sm()
                        .font_family("Menlo")
                        .text_color(name_color)
                        .flex_1()
                        .child(name),
                )
                .child(
                    div()
                        .text_xs()
                        .font_family("Menlo")
                        .text_color(tag_color)
                        .child(tag_text),
                );
            rows.push(row);
        }

        let query_display = if self.query.is_empty() {
            div()
                .text_sm()
                .font_family("Menlo")
                .text_color(gpui::rgb(t.text_faint))
                .child("Choose template…")
        } else {
            div()
                .text_sm()
                .font_family("Menlo")
                .text_color(gpui::rgb(t.text))
                .child(self.query.clone())
        };

        // Transparent full-screen backdrop — clicks outside dismiss.
        div()
            .absolute()
            .inset_0()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::handle_key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_this, _, _, cx| {
                    cx.emit(TemplatePickerEvent::Close);
                }),
            )
            .flex()
            .flex_col()
            .items_center()
            .pt(px(80.0))
            .child(
                div()
                    .w(px(480.0))
                    .bg(gpui::rgb(t.bg_surface))
                    .border_1()
                    .border_color(gpui::rgb(t.border_subtle))
                    .rounded(px(8.0))
                    .overflow_hidden()
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        // Header row: icon + prompt
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .px(px(16.0))
                            .py(px(12.0))
                            .border_b_1()
                            .border_color(gpui::rgb(t.border_subtle))
                            .gap(px(10.0))
                            .child(
                                div()
                                    .text_xs()
                                    .font_family("Menlo")
                                    .text_color(gpui::rgb(t.ochre))
                                    .child("⌘N"),
                            )
                            .child(query_display),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .max_h(px(320.0))
                            .overflow_hidden()
                            .children(rows),
                    ),
            )
    }
}

// ── Template scanning ─────────────────────────────────────────────────────────

/// Scan `.ockr/templates/` for `.typ` files.  Returns entries sorted by name.
/// Returns an empty vec if the directory doesn't exist.
pub fn scan_templates(vault_root: &std::path::Path) -> Vec<TemplateEntry> {
    let templates_dir = vault_root.join(".ockr").join("templates");
    let Ok(entries) = std::fs::read_dir(&templates_dir) else {
        return Vec::new();
    };
    let mut result: Vec<TemplateEntry> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) == Some("typ") {
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("untitled")
                    .to_string();
                Some(TemplateEntry { name, path: Some(path) })
            } else {
                None
            }
        })
        .collect();
    result.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    result
}

/// Extract the first `= Heading` from template content (if present),
/// and convert it to a filesystem-safe filename stem.
pub fn heading_to_filename_stem(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("= ") {
            let stem: String = rest
                .chars()
                .map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
                .collect::<String>()
                .trim_matches('-')
                .to_string();
            if !stem.is_empty() {
                return Some(stem);
            }
        }
    }
    None
}
