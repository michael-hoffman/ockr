//! Quick Switch — fuzzy note-title search, opened with Cmd-K.
//!
//! Displays a floating list of vault notes filtered by a typed query.
//! Results are ranked by match quality; recency is applied by the caller
//! (notes opened recently are placed first in the `files` list passed to
//! `QuickSwitch::new`).
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
pub enum QuickSwitchEvent {
    /// User dismissed without selecting.
    Close,
    /// User selected a note; carries the absolute path.
    Open(PathBuf),
}

impl EventEmitter<QuickSwitchEvent> for QuickSwitch {}

// ── View ──────────────────────────────────────────────────────────────────────

pub struct QuickSwitch {
    pub focus_handle: FocusHandle,
    query: String,
    /// Indices into `self.files` ordered by match quality.
    matches: Vec<usize>,
    selected: usize,
    /// Snapshot of vault files taken at open time.  Caller should pass
    /// recently-opened files first so they rank higher when quality ties.
    files: Vec<VaultFile>,
}

impl QuickSwitch {
    pub fn new(files: Vec<VaultFile>, cx: &mut Context<Self>) -> Self {
        let mut qs = Self {
            focus_handle: cx.focus_handle(),
            query: String::new(),
            matches: Vec::new(),
            selected: 0,
            files,
        };
        qs.refresh_matches();
        qs
    }

    fn refresh_matches(&mut self) {
        let q = self.query.to_lowercase();
        let mut scored: Vec<(usize, i32)> = self
            .files
            .iter()
            .enumerate()
            .filter_map(|(i, f)| fuzzy_score(&q, &f.title).map(|s| (i, s)))
            .collect();
        // Stable sort: ties preserve original order (recency from caller).
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
            cx.emit(QuickSwitchEvent::Close);
            return;
        }

        if k.key == "enter" {
            if let Some(&idx) = self.matches.get(self.selected) {
                cx.emit(QuickSwitchEvent::Open(self.files[idx].abs_path.clone()));
            } else {
                cx.emit(QuickSwitchEvent::Close);
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
        cx.emit(QuickSwitchEvent::Open(self.files[idx].abs_path.clone()));
    }
}

impl Focusable for QuickSwitch {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

impl Render for QuickSwitch {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.global::<ThemePalette>().clone();
        let mut rows = Vec::with_capacity(self.matches.len().min(16));

        for (row_idx, &file_idx) in self.matches.iter().take(16).enumerate() {
            let file = &self.files[file_idx];
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

            let title = file.title.clone();
            let subtitle = file.rel_path.to_string_lossy().into_owned();

            let row = div()
                .flex()
                .flex_col()
                .px(px(16.0))
                .py(px(6.0))
                .bg(bg)
                .border_l(px(2.0))
                .border_color(accent)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event, window, cx| {
                        this.handle_row_click(file_idx, event, window, cx);
                    }),
                )
                .child(
                    div()
                        .text_sm()
                        .font_family("Menlo")
                        .text_color(title_color)
                        .child(title),
                )
                .child(
                    div()
                        .text_xs()
                        .font_family("Menlo")
                        .text_color(gpui::rgb(t.text_faint))
                        .child(subtitle),
                );
            rows.push(row);
        }

        let query_display = if self.query.is_empty() {
            div()
                .text_sm()
                .font_family("Menlo")
                .text_color(gpui::rgb(t.text_faint))
                .child("Switch to note…")
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
                cx.listener(|this, _, _, cx| {
                    cx.emit(QuickSwitchEvent::Close);
                    let _ = this;
                }),
            )
            .flex()
            .flex_col()
            .items_end()
            .pt(px(8.0))
            .pr(px(8.0))
            .child(
                div()
                    .w(px(400.0))
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
                        // Search input row
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
                                    .child("⌘K"),
                            )
                            .child(query_display),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .max_h(px(380.0))
                            .overflow_hidden()
                            .children(rows),
                    ),
            )
    }
}

// ── Fuzzy scoring ─────────────────────────────────────────────────────────────

/// Returns `None` if `query` does not match `title` at all.
/// Higher scores = better match.
fn fuzzy_score(query: &str, title: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let q = query.to_lowercase();
    let t = title.to_lowercase();

    // Exact match.
    if q == t {
        return Some(10_000);
    }
    // Prefix match.
    if t.starts_with(&q) {
        return Some(5_000 + (200i32 - t.len() as i32).max(0));
    }
    // Substring match — earlier = better.
    if let Some(pos) = t.find(&q) {
        return Some(2_000 - pos as i32);
    }
    // Subsequence match — query chars appear in order within the title.
    let mut qi = q.chars().peekable();
    for ch in t.chars() {
        if qi.peek() == Some(&ch) {
            qi.next();
        }
    }
    if qi.peek().is_none() {
        return Some(100);
    }

    None
}
