//! Document Outline panel — shows the heading structure of the active note.
//!
//! Opened with `Cmd-Shift-O`.  Rendered as a centred overlay (same layout as
//! the backlink panel).  Selecting an entry jumps the editor to that heading's
//! line.
//!
//! Navigation: arrow keys / Ctrl-J / Ctrl-K; Enter jumps; Escape dismisses.

use gpui::{
    Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent,
    Render, Window, div, prelude::*, px,
};

use crate::ui::theme::ThemePalette;

// ── Heading ───────────────────────────────────────────────────────────────────

/// A parsed heading entry.
pub struct OutlineHeading {
    /// 0-indexed line number in the buffer.
    pub line: usize,
    /// Heading depth: 1 = `=`, 2 = `==`, etc.
    pub level: usize,
    /// Heading text (the part after the leading `= ` markers).
    pub title: String,
}

/// Parse all Typst headings from a buffer text.
///
/// A heading is any line whose first non-space characters are one or more `=`
/// followed by at least one space.  We respect up to level 6.
pub fn parse_headings(text: &str) -> Vec<OutlineHeading> {
    let mut out = Vec::new();
    for (line_no, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || !trimmed.starts_with('=') {
            continue;
        }
        let level = trimmed.chars().take_while(|&c| c == '=').count();
        let rest = &trimmed[level..];
        // Must be followed by at least one space to be a heading.
        if rest.starts_with(' ') {
            let title = rest.trim_start().to_string();
            if !title.is_empty() {
                out.push(OutlineHeading { line: line_no, level, title });
            }
        }
    }
    out
}

// ── Events ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum OutlinePanelEvent {
    Close,
    JumpToLine(usize),
}

impl EventEmitter<OutlinePanelEvent> for OutlinePanel {}

// ── View ──────────────────────────────────────────────────────────────────────

pub struct OutlinePanel {
    pub focus_handle: FocusHandle,
    headings: Vec<OutlineHeading>,
    selected: usize,
}

impl OutlinePanel {
    pub fn new(buffer_text: String, cx: &mut Context<Self>) -> Self {
        let headings = parse_headings(&buffer_text);
        Self {
            focus_handle: cx.focus_handle(),
            headings,
            selected: 0,
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
            cx.emit(OutlinePanelEvent::Close);
            return;
        }
        if k.key == "enter" {
            if let Some(h) = self.headings.get(self.selected) {
                cx.emit(OutlinePanelEvent::JumpToLine(h.line));
            } else {
                cx.emit(OutlinePanelEvent::Close);
            }
            return;
        }
        if k.key == "up" || (k.modifiers.control && k.key == "k") {
            self.selected = self.selected.saturating_sub(1);
            cx.notify();
        }
        if k.key == "down" || (k.modifiers.control && k.key == "j") {
            if !self.headings.is_empty() && self.selected + 1 < self.headings.len() {
                self.selected += 1;
            }
            cx.notify();
        }
    }

    fn handle_row_click(
        &mut self,
        idx: usize,
        _event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.emit(OutlinePanelEvent::JumpToLine(self.headings[idx].line));
    }
}

impl Focusable for OutlinePanel {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

impl Render for OutlinePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.global::<ThemePalette>().clone();
        let mut rows = Vec::new();

        if self.headings.is_empty() {
            rows.push(
                div()
                    .px(px(16.0))
                    .py(px(12.0))
                    .text_sm()
                    .font_family("Menlo")
                    .text_color(gpui::rgb(t.text_faint))
                    .child("No headings found.")
                    .into_any_element(),
            );
        } else {
            for (row_idx, heading) in self.headings.iter().enumerate() {
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
                let title_color = if is_selected { t.text } else { t.text_muted };

                // Indent by level: level 1 = 0px, level 2 = 12px, level 3 = 24px, …
                let indent = px(((heading.level.saturating_sub(1)) * 12) as f32);
                // Level label: "H1", "H2", …
                let level_label = format!("H{}", heading.level.min(6));
                let title = heading.title.clone();
                let line_label = format!(":{}", heading.line + 1);

                rows.push(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .px(px(16.0))
                        .py(px(5.0))
                        .bg(bg)
                        .border_l(px(2.0))
                        .border_color(accent)
                        .gap(px(6.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event, window, cx| {
                                this.handle_row_click(row_idx, event, window, cx);
                            }),
                        )
                        // Left indent based on heading level.
                        .child(div().w(indent))
                        // "H1" / "H2" level badge.
                        .child(
                            div()
                                .text_xs()
                                .font_family("Menlo")
                                .text_color(gpui::rgb(t.ochre))
                                .flex_shrink_0()
                                .child(level_label),
                        )
                        // Heading title — fills available space.
                        .child(
                            div()
                                .text_sm()
                                .font_family("Menlo")
                                .text_color(gpui::rgb(title_color))
                                .flex_grow()
                                .overflow_hidden()
                                .child(title),
                        )
                        // Line number hint on the right.
                        .child(
                            div()
                                .text_xs()
                                .font_family("Menlo")
                                .text_color(gpui::rgb(t.text_faint))
                                .flex_shrink_0()
                                .child(line_label),
                        )
                        .into_any_element(),
                );
            }
        }

        div()
            .absolute()
            .inset_0()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::handle_key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(OutlinePanelEvent::Close)),
            )
            .flex()
            .flex_col()
            .items_center()
            .pt(px(48.0))
            .child(
                div()
                    .w(px(440.0))
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
                                    .child("⌘⇧O"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_family("Menlo")
                                    .text_color(gpui::rgb(t.text_muted))
                                    .child("Document Outline"),
                            ),
                    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_typst_headings() {
        let text = "Some intro text\n= Chapter One\nsome body\n== Section A\n=== Subsection\n== Section B\n= Chapter Two";
        let headings = parse_headings(text);
        // Lines: 0=intro, 1="= Chapter One", 2=body, 3="== Section A",
        //        4="=== Subsection", 5="== Section B", 6="= Chapter Two" → 5 headings.
        assert_eq!(headings.len(), 5);
        assert_eq!(headings[0].level, 1);
        assert_eq!(headings[0].title, "Chapter One");
        assert_eq!(headings[0].line, 1);
        assert_eq!(headings[1].level, 2);
        assert_eq!(headings[1].title, "Section A");
        assert_eq!(headings[2].level, 3);
        assert_eq!(headings[2].title, "Subsection");
        assert_eq!(headings[4].level, 1);
        assert_eq!(headings[4].title, "Chapter Two");
    }

    #[test]
    fn ignores_non_headings() {
        let text = "=not a heading\n== \n= Valid";
        let headings = parse_headings(text);
        assert_eq!(headings.len(), 1);
        assert_eq!(headings[0].title, "Valid");
    }
}
