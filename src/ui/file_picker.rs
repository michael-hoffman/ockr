//! Fuzzy file picker — path-based search over vault files (Ctrl-P).
//!
//! Unlike `QuickSwitch` (which matches note *titles* via Cmd-K), the file
//! picker matches the vault-relative *file path*, so partial directory
//! segments work: typing `"com/sec"` finds `"components/section.typ"`.
//!
//! Scoring: full-path fuzzy score, with a +300 bonus when the query also
//! matches the bare filename.  Within a tier, original order (recency from
//! the caller) breaks ties.
//!
//! Navigation: arrow keys or Ctrl-J/K.  Enter opens; Escape dismisses.

use std::path::PathBuf;

use gpui::{
    Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent,
    Render, Window, div, prelude::*, px,
};

use crate::ui::theme::ThemePalette;
use crate::vault::VaultFile;

// ── Events ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum FilePickerEvent {
    Close,
    Open(PathBuf),
}

impl EventEmitter<FilePickerEvent> for FilePicker {}

// ── View ──────────────────────────────────────────────────────────────────────

pub struct FilePicker {
    pub focus_handle: FocusHandle,
    query: String,
    /// Indices into `self.files` in match-score order.
    matches: Vec<usize>,
    selected: usize,
    /// Vault files snapshot taken at open time; recency-ordered by caller.
    files: Vec<VaultFile>,
}

impl FilePicker {
    pub fn new(files: Vec<VaultFile>, cx: &mut Context<Self>) -> Self {
        let mut fp = Self {
            focus_handle: cx.focus_handle(),
            query: String::new(),
            matches: Vec::new(),
            selected: 0,
            files,
        };
        fp.refresh_matches();
        fp
    }

    fn refresh_matches(&mut self) {
        let q = self.query.to_lowercase();
        let mut scored: Vec<(usize, i32)> = self
            .files
            .iter()
            .enumerate()
            .filter_map(|(i, f)| file_picker_score(&q, f).map(|s| (i, s)))
            .collect();
        // Stable sort: ties preserve caller's recency order.
        scored.sort_by(|a, b| b.1.cmp(&a.1));
        self.matches = scored.into_iter().map(|(i, _)| i).collect();
        if self.selected >= self.matches.len() {
            self.selected = self.matches.len().saturating_sub(1);
        }
    }

    fn handle_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let k = &event.keystroke;

        if k.key == "escape" {
            cx.emit(FilePickerEvent::Close);
            return;
        }

        if k.key == "enter" {
            if let Some(&idx) = self.matches.get(self.selected) {
                cx.emit(FilePickerEvent::Open(self.files[idx].abs_path.clone()));
            } else {
                cx.emit(FilePickerEvent::Close);
            }
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
        cx.emit(FilePickerEvent::Open(self.files[idx].abs_path.clone()));
    }
}

impl Focusable for FilePicker {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

impl Render for FilePicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.global::<ThemePalette>().clone();
        let max_rows = 16_usize;
        let mut rows = Vec::with_capacity(self.matches.len().min(max_rows));

        for (row_idx, &file_idx) in self.matches.iter().take(max_rows).enumerate() {
            let file = &self.files[file_idx];
            let is_selected = row_idx == self.selected;

            let bg = if is_selected { gpui::rgb(t.bg_hover) } else { gpui::rgb(t.bg_surface) };
            let accent = if is_selected { gpui::rgb(t.ochre) } else { gpui::rgb(t.border_subtle) };
            let name_color = if is_selected { gpui::rgb(t.text) } else { gpui::rgb(t.text_muted) };

            // Split path into filename and parent directory for display.
            let rel = file.rel_path.to_string_lossy();
            let (parent, filename) = match rel.rfind('/').or_else(|| rel.rfind('\\')) {
                Some(sep) => (&rel[..sep], &rel[sep + 1..]),
                None => ("", rel.as_ref()),
            };

            let filename = filename.to_string();
            let parent_str = if parent.is_empty() {
                String::new()
            } else {
                format!("{}/", parent)
            };

            let row = div()
                .flex()
                .flex_col()
                .px(px(16.0))
                .py(px(5.0))
                .bg(bg)
                .border_l(px(2.0))
                .border_color(accent)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event, window, cx| {
                        this.handle_row_click(file_idx, event, window, cx);
                    }),
                )
                // Filename on top — prominent.
                .child(
                    div()
                        .text_sm()
                        .font_family("Menlo")
                        .text_color(name_color)
                        .child(filename),
                )
                // Parent path below — subtle.
                .child(
                    div()
                        .text_xs()
                        .font_family("Menlo")
                        .text_color(gpui::rgb(t.text_faint))
                        .child(parent_str),
                );
            rows.push(row);
        }

        // Count line at bottom of the results.
        let total = self.matches.len();
        let shown = total.min(max_rows);
        let count_str = if self.query.is_empty() {
            format!("{} files", total)
        } else if total == 0 {
            "no matches".to_string()
        } else if total > shown {
            format!("{}/{} matches", shown, total)
        } else {
            format!("{} match{}", total, if total == 1 { "" } else { "es" })
        };

        let query_el = if self.query.is_empty() {
            div()
                .text_sm()
                .font_family("Menlo")
                .text_color(gpui::rgb(t.text_faint))
                .child("Open file…")
        } else {
            div()
                .text_sm()
                .font_family("Menlo")
                .text_color(gpui::rgb(t.text))
                .child(self.query.clone())
        };

        div()
            .absolute()
            .inset_0()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::handle_key))
            // Backdrop click dismisses.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(FilePickerEvent::Close)),
            )
            .flex()
            .flex_col()
            .items_center()
            .pt(px(48.0))
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
                    // Stop backdrop click from propagating inside the panel.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    // ── Header ─────────────────────────────────────────────
                    .child(
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
                                    .child("^P"),
                            )
                            .child(query_el)
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .justify_end()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_family("Menlo")
                                            .text_color(gpui::rgb(t.text_faint))
                                            .child(count_str),
                                    ),
                            ),
                    )
                    // ── Results list ───────────────────────────────────────
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .max_h(px(400.0))
                            .overflow_hidden()
                            .children(rows),
                    ),
            )
    }
}

// ── Scoring ───────────────────────────────────────────────────────────────────

/// Score `query` against a vault file's **relative path**.
/// Filename-only matches get a +300 bonus so short queries prefer exact filenames
/// over deep-path substring matches.
fn file_picker_score(query: &str, file: &VaultFile) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let rel = file.rel_path.to_string_lossy().to_lowercase();

    // Exact rel-path match is the clearest possible hit — rank it above everything.
    if query == rel.as_str() {
        return Some(20_000);
    }

    // Extract the bare filename for bonus scoring.
    let filename = file
        .rel_path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    let rel_score = fuzzy_score_str(query, &rel);
    // Filename match gets a +300 bonus so "foo" favours "foo.typ" over deep paths.
    let name_score = fuzzy_score_str(query, &filename).map(|s| s + 300);

    match (rel_score, name_score) {
        (None, None) => None,
        (Some(r), None) => Some(r),
        (None, Some(n)) => Some(n),
        (Some(r), Some(n)) => Some(r.max(n)),
    }
}

/// Generic fuzzy scorer: exact > prefix > substring > subsequence.
/// Returns `None` when there is no match.
fn fuzzy_score_str(query: &str, target: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    // Exact match.
    if query == target {
        return Some(10_000);
    }
    // Prefix match — shorter target is better (more specific).
    if target.starts_with(query) {
        return Some(5_000 + (200i32 - target.len() as i32).max(0));
    }
    // Substring match — earlier position = better.
    if let Some(pos) = target.find(query) {
        return Some(2_000 - pos as i32);
    }
    // Subsequence match — query chars appear in order within the target.
    let mut qi = query.chars().peekable();
    for ch in target.chars() {
        if qi.peek() == Some(&ch) {
            qi.next();
        }
    }
    if qi.peek().is_none() {
        return Some(100);
    }
    None
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn vf(rel: &str) -> VaultFile {
        VaultFile {
            title: rel.to_string(),
            rel_path: std::path::PathBuf::from(rel),
            abs_path: std::path::PathBuf::from(rel),
        }
    }

    #[test]
    fn exact_filename_beats_deep_path() {
        let foo = vf("foo.typ");
        let deep = vf("bar/baz/foo.typ");
        let s_exact = file_picker_score("foo.typ", &foo).unwrap();
        let s_deep = file_picker_score("foo.typ", &deep).unwrap();
        // Exact rel-path match scores 10_000 vs deep path substring + name bonus.
        assert!(s_exact > s_deep, "exact={} deep={}", s_exact, s_deep);
    }

    #[test]
    fn path_segment_matches() {
        let f = vf("notes/2024/january.typ");
        assert!(file_picker_score("2024/jan", &f).is_some());
        assert!(file_picker_score("notes/jan", &f).is_some());
    }

    #[test]
    fn non_matching_query_returns_none() {
        let f = vf("notes/foo.typ");
        assert!(file_picker_score("zzz", &f).is_none());
    }

    #[test]
    fn empty_query_matches_everything() {
        let f = vf("notes/foo.typ");
        assert_eq!(file_picker_score("", &f), Some(0));
    }

    #[test]
    fn filename_prefix_beats_path_substring() {
        // "jan" is a prefix of "january.typ" (+300 bonus) vs deep path substring.
        let shallow = vf("january.typ");
        let deep = vf("a/b/c/january.typ");
        let s_shallow = file_picker_score("jan", &shallow).unwrap();
        let s_deep = file_picker_score("jan", &deep).unwrap();
        assert!(s_shallow >= s_deep, "shallow={} deep={}", s_shallow, s_deep);
    }
}
