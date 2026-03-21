//! Vault Search — full-text substring search across all vault notes.
//!
//! Opened with Cmd-Shift-F. Searches file content (case-insensitive substring)
//! and shows the first matching line as a snippet. Capped at 20 results.
//!
//! Navigation: arrow keys or Ctrl-J/K. Enter opens; Escape dismisses.

use std::path::PathBuf;

use gpui::{
    Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent,
    Render, Window, div, prelude::*, px,
};

use crate::ui::theme::ThemePalette;
use crate::vault::VaultFile;

// ── Events ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum VaultSearchEvent {
    Close,
    /// Open a file and jump the cursor to the given 0-based line number.
    Open(PathBuf, usize),
}

impl EventEmitter<VaultSearchEvent> for VaultSearch {}

// ── Data ──────────────────────────────────────────────────────────────────────

struct SearchMatch {
    file: VaultFile,
    /// The first line that matched the query.
    snippet: String,
    /// 1-based line number of the first match.
    line_no: usize,
}

// ── View ──────────────────────────────────────────────────────────────────────

pub struct VaultSearch {
    pub focus_handle: FocusHandle,
    query: String,
    results: Vec<SearchMatch>,
    selected: usize,
    /// Snapshot of vault files taken at open time.
    files: Vec<VaultFile>,
}

impl VaultSearch {
    pub fn new(files: Vec<VaultFile>, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            files,
        }
    }

    /// Re-run the search for `self.query` over all vault files.
    fn refresh_results(&mut self) {
        self.results.clear();
        self.selected = 0;

        if self.query.is_empty() {
            return;
        }

        let q = self.query.to_lowercase();

        'outer: for file in &self.files {
            let content = match std::fs::read_to_string(&file.abs_path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            for (idx, line) in content.lines().enumerate() {
                if line.to_lowercase().contains(&q) {
                    self.results.push(SearchMatch {
                        file: file.clone(),
                        snippet: line.trim().to_string(),
                        line_no: idx + 1,
                    });
                    if self.results.len() >= 20 {
                        break 'outer;
                    }
                    continue 'outer; // one match per file
                }
            }
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
            cx.emit(VaultSearchEvent::Close);
            return;
        }

        if k.key == "enter" {
            if let Some(m) = self.results.get(self.selected) {
                // line_no is 1-based; jump_to_line expects 0-based.
                cx.emit(VaultSearchEvent::Open(m.file.abs_path.clone(), m.line_no.saturating_sub(1)));
            } else {
                cx.emit(VaultSearchEvent::Close);
            }
            return;
        }

        if k.key == "up" || (k.modifiers.control && k.key == "k") {
            self.selected = self.selected.saturating_sub(1);
            cx.notify();
            return;
        }

        if k.key == "down" || (k.modifiers.control && k.key == "j") {
            if !self.results.is_empty() && self.selected + 1 < self.results.len() {
                self.selected += 1;
            }
            cx.notify();
            return;
        }

        if k.key == "backspace" {
            self.query.pop();
            self.refresh_results();
            cx.notify();
            return;
        }

        if let Some(ch) = &k.key_char {
            if !k.modifiers.control && !k.modifiers.platform {
                self.query.push_str(ch);
                self.refresh_results();
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
        if let Some(m) = self.results.get(idx) {
            cx.emit(VaultSearchEvent::Open(m.file.abs_path.clone(), m.line_no.saturating_sub(1)));
        }
    }
}

impl Focusable for VaultSearch {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

impl Render for VaultSearch {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.global::<ThemePalette>().clone();
        let mut rows = Vec::with_capacity(self.results.len());

        for (row_idx, m) in self.results.iter().enumerate() {
            let is_selected = row_idx == self.selected;
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
            let title_color = if is_selected {
                gpui::rgb(t.text)
            } else {
                gpui::rgb(t.text_muted)
            };

            let title = m.file.title.clone();
            let rel = m.file.rel_path.to_string_lossy().into_owned();
            let snippet = m.snippet.clone();
            let line_no = m.line_no;

            let row = div()
                .flex()
                .flex_col()
                .px(px(16.0))
                .py(px(6.0))
                .gap(px(2.0))
                .bg(bg)
                .border_l(px(2.0))
                .border_color(accent)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event, window, cx| {
                        this.handle_row_click(row_idx, event, window, cx);
                    }),
                )
                // Title line
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .text_sm()
                                .font_family("Menlo")
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(title_color)
                                .child(title),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_family("Menlo")
                                .text_color(gpui::rgb(t.text_faint))
                                .child(rel),
                        ),
                )
                // Snippet line with line number
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .text_xs()
                                .font_family("Menlo")
                                .text_color(gpui::rgb(t.ochre))
                                .child(format!("{line_no}:")),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_family("Menlo")
                                .text_color(gpui::rgb(t.text_muted))
                                .child(snippet),
                        ),
                );
            rows.push(row);
        }

        // The visible input field: darkest fill + ochre border so it reads as
        // an active text box against the panel surface.
        let input_field = div()
            .flex_1()
            .flex()
            .flex_row()
            .items_center()
            .bg(gpui::rgb(t.bg_base))
            .border_1()
            .border_color(gpui::rgb(t.ochre_border))
            .rounded(px(4.0))
            .px(px(10.0))
            .py(px(5.0))
            .gap(px(0.0))
            .child(if self.query.is_empty() {
                // Placeholder text
                div()
                    .text_sm()
                    .font_family("Menlo")
                    .text_color(gpui::rgb(t.text_faint))
                    .child("Search vault…")
            } else {
                // Typed query + block cursor glyph
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(0.0))
                    .child(
                        div()
                            .text_sm()
                            .font_family("Menlo")
                            .text_color(gpui::rgb(t.text))
                            .child(self.query.clone()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_family("Menlo")
                            .text_color(gpui::rgb(t.ochre))
                            .child("▌"),
                    )
            });

        let empty_hint = if !self.query.is_empty() && self.results.is_empty() {
            Some(
                div()
                    .px(px(16.0))
                    .py(px(12.0))
                    .text_xs()
                    .font_family("Menlo")
                    .text_color(gpui::rgb(t.text_faint))
                    .child("No matches found."),
            )
        } else {
            None
        };

        // Transparent full-screen backdrop — clicks outside dismiss.
        div()
            .absolute()
            .inset_0()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::handle_key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| {
                    cx.emit(VaultSearchEvent::Close);
                }),
            )
            .flex()
            .flex_col()
            .items_center()
            .pt(px(60.0))
            .child(
                div()
                    .w(px(600.0))
                    .bg(gpui::rgb(t.bg_surface))
                    .border_1()
                    // Lift the outer border one step so the panel has visible edges.
                    .border_color(gpui::rgb(t.border))
                    .rounded(px(8.0))
                    .overflow_hidden()
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        // Search input row — its own slightly-lighter background so it
                        // stands apart from the results list below.
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .px(px(14.0))
                            .py(px(10.0))
                            .bg(gpui::rgb(t.bg_hover))
                            .border_b_1()
                            .border_color(gpui::rgb(t.border))
                            .gap(px(10.0))
                            .child(
                                // ⌘⇧F badge
                                div()
                                    .text_xs()
                                    .font_family("Menlo")
                                    .text_color(gpui::rgb(t.ochre))
                                    .flex_shrink_0()
                                    .child("⌘⇧F"),
                            )
                            .child(input_field),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .max_h(px(420.0))
                            .overflow_hidden()
                            .children(rows)
                            .when_some(empty_hint, |d, hint| d.child(hint)),
                    ),
            )
    }
}
