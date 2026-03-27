//! Preview pane — rasterises a typst `PagedDocument` and displays it.
//!
//! ## Rasterisation
//!
//! `typst-render` converts a typst `Page` (which wraps a `Frame`) to a
//! `tiny_skia::Pixmap` (RGBA8). We render at 2 px/pt (≈ 144 PPI), matching
//! a standard 2× HiDPI display.
//!
//! The `Pixmap` bytes are wrapped in `image::Frame` → `gpui::RenderImage` and
//! drawn via `window.paint_image()` inside a GPUI `canvas` element, which also
//! captures the element's screen bounds for link hit-testing.
//!
//! ## Wikilink clicks
//!
//! `set_document` extracts `FrameItem::Link(Destination::Url(url), size)` items
//! from the first page's frame (recursing into groups) and stores them as
//! `LinkRegion` values in typst-pt coordinates.  On every mouse-down event the
//! click position is mapped through the `ObjectFit::Contain` transform back to
//! typst-pt coordinates and hit-tested against the stored regions.  A match
//! emits `PreviewEvent::OpenLink(ockr_url)` which `MainWindow` routes to
//! `open_path` just like HTML wikilink clicks.
//!
//! ## References
//! - typst-render crate: <https://docs.rs/typst-render>
//! - GPUI canvas element: gpui::elements::canvas

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    Bounds, ClipboardItem, Context, Corners, EventEmitter, MouseButton, MouseDownEvent, Pixels,
    Render, RenderImage, Window, canvas, div, prelude::*, px,
};

use crate::compiler::{Diagnostic, DiagnosticSeverity};
use crate::ui::theme::ThemePalette;
use image::Frame;
use typst::layout::{FrameItem, PagedDocument};
use typst::model::Destination;

/// Pixels per typst point for rasterisation (2 = 144 PPI, good for 2× HiDPI).
const PIXELS_PER_PT: f32 = 2.0;

// ── Public events ─────────────────────────────────────────────────────────────

/// Events emitted by `PreviewPane`.
#[derive(Debug, Clone)]
pub enum PreviewEvent {
    /// User clicked an `ockr://`-scheme link.  Value is the full URL string.
    OpenLink(String),
}

impl EventEmitter<PreviewEvent> for PreviewPane {}

// ── Link regions ──────────────────────────────────────────────────────────────

/// A single clickable link annotation extracted from the typst frame.
#[derive(Clone)]
struct LinkRegion {
    /// Left edge in typst pt.
    x_pt: f32,
    /// Top edge in typst pt.
    y_pt: f32,
    /// Width in typst pt.
    w_pt: f32,
    /// Height in typst pt.
    h_pt: f32,
    /// Full `ockr://` URL (vault-relative path with scheme prefix).
    url: String,
}

// ── View ──────────────────────────────────────────────────────────────────────

/// A GPUI view that renders the first page of a compiled typst document.
///
/// Updated by calling `set_document` (success) or `set_error` (failure).
/// Both methods call `cx.notify()` so GPUI schedules a re-render.
pub struct PreviewPane {
    /// Cached rasterised image. `None` = no document loaded yet.
    image: Option<Arc<RenderImage>>,
    /// Pixel dimensions of the rasterised image (physical pixels).
    image_px: Option<(u32, u32)>,
    /// `ockr://` link annotation rectangles extracted from the compiled document.
    link_regions: Vec<LinkRegion>,
    /// Bounds of the preview canvas captured during the most recent paint pass.
    ///
    /// Wrapped in `Rc<Cell<…>>` so the value can be updated from a `FnOnce`
    /// canvas closure (which cannot borrow `self`) and read back from the
    /// `on_mouse_down` listener in the same render frame cycle.
    preview_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Compiler diagnostics shown in place of the preview on failure.
    diagnostics: Vec<Diagnostic>,
}

impl PreviewPane {
    pub fn new() -> Self {
        Self {
            image: None,
            image_px: None,
            link_regions: Vec::new(),
            preview_bounds: Rc::new(Cell::new(None)),
            diagnostics: Vec::new(),
        }
    }

    /// Replace the current document.
    ///
    /// Rasterises the first page, extracts link annotations, and schedules a
    /// re-render.
    pub fn set_document(&mut self, doc: Arc<PagedDocument>, cx: &mut Context<Self>) {
        self.diagnostics.clear();
        let (image, px_size) = rasterize(&doc);
        self.image = image;
        self.image_px = px_size;
        self.link_regions = doc
            .pages
            .first()
            .map(|page| {
                let mut regions = Vec::new();
                extract_links(&page.frame, 0.0, 0.0, &mut regions);
                regions
            })
            .unwrap_or_default();
        cx.notify();
    }

    /// Show compiler diagnostics instead of the preview.
    pub fn set_diagnostics(&mut self, diags: Vec<Diagnostic>, cx: &mut Context<Self>) {
        self.image = None;
        self.image_px = None;
        self.link_regions.clear();
        self.diagnostics = diags;
        cx.notify();
    }

    /// Convenience wrapper for a single plain error string (panics, etc.).
    pub fn set_error(&mut self, msg: String, cx: &mut Context<Self>) {
        self.set_diagnostics(
            vec![Diagnostic { severity: DiagnosticSeverity::Error, message: msg, span_file: None }],
            cx,
        );
    }


    // ── Click handling ────────────────────────────────────────────────────────

    /// Map a window-space click to typst-pt coordinates and hit-test against
    /// stored link regions.  Emits `PreviewEvent::OpenLink` on a match.
    fn handle_click(
        &mut self,
        win_pos: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(bounds) = self.preview_bounds.get() else { return };
        let Some((iw, ih)) = self.image_px else { return };
        if self.link_regions.is_empty() { return; }

        // GPUI reports positions in *logical* pixels; the display scale factor
        // converts between logical and physical pixels.
        let scale_factor = window.scale_factor();

        let cont_x0 = f32::from(bounds.origin.x);
        let cont_y0 = f32::from(bounds.origin.y);
        let cont_w  = f32::from(bounds.size.width);
        let cont_h  = f32::from(bounds.size.height);

        // Container-relative click position (logical px).
        let rel_x = f32::from(win_pos.x) - cont_x0;
        let rel_y = f32::from(win_pos.y) - cont_y0;

        // Logical size of the image (physical px ÷ display scale).
        let log_iw = iw as f32 / scale_factor;
        let log_ih = ih as f32 / scale_factor;

        // ObjectFit::Contain: uniform scale that fits the image within the
        // container, centred horizontally and vertically.
        let fit_scale = (cont_w / log_iw).min(cont_h / log_ih);
        let img_x0 = (cont_w - log_iw * fit_scale) / 2.0;
        let img_y0 = (cont_h - log_ih * fit_scale) / 2.0;

        // Click offset within the displayed image area (logical px).
        let img_rel_x = rel_x - img_x0;
        let img_rel_y = rel_y - img_y0;

        // Reject clicks outside the image.
        if img_rel_x < 0.0 || img_rel_y < 0.0
            || img_rel_x > log_iw * fit_scale
            || img_rel_y > log_ih * fit_scale
        {
            return;
        }

        // Convert logical px → typst pt.
        //
        //   physical_px = pt × PIXELS_PER_PT
        //   logical_px  = physical_px ÷ scale_factor
        //   ∴ pt = logical_px × scale_factor ÷ PIXELS_PER_PT
        //
        // `img_rel_x / fit_scale` removes the ObjectFit::Contain zoom to get
        // the position within the original logical-pixel image.
        let pt_x = (img_rel_x / fit_scale) * scale_factor / PIXELS_PER_PT;
        let pt_y = (img_rel_y / fit_scale) * scale_factor / PIXELS_PER_PT;

        for region in &self.link_regions {
            if pt_x >= region.x_pt
                && pt_x <= region.x_pt + region.w_pt
                && pt_y >= region.y_pt
                && pt_y <= region.y_pt + region.h_pt
            {
                cx.emit(PreviewEvent::OpenLink(region.url.clone()));
                return;
            }
        }
    }
}

impl Render for PreviewPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.global::<ThemePalette>().clone();
        let bg = gpui::rgb(t.bg_panel);

        // ── Diagnostics state ─────────────────────────────────────────────────
        if !self.diagnostics.is_empty() {
            let all_text = self.diagnostics
                .iter()
                .map(|d| d.message.clone())
                .collect::<Vec<_>>()
                .join("\n");
            let all_text_for_copy = all_text.clone();
            let bg_hover = t.bg_hover;
            let text_subtle = t.text_subtle;
            let text = t.text;

            let error_count = self.diagnostics.iter()
                .filter(|d| d.severity == DiagnosticSeverity::Error)
                .count();
            let warn_count = self.diagnostics.iter()
                .filter(|d| d.severity == DiagnosticSeverity::Warning)
                .count();
            let header_label = match (error_count, warn_count) {
                (e, 0) => format!("{e} error{}", if e == 1 { "" } else { "s" }),
                (0, w) => format!("{w} warning{}", if w == 1 { "" } else { "s" }),
                (e, w) => format!("{e} error{}, {w} warning{}", if e == 1 { "" } else { "s" }, if w == 1 { "" } else { "s" }),
            };

            let diag_rows: Vec<_> = self.diagnostics.iter().map(|d| {
                let (badge_color, msg_color) = match d.severity {
                    DiagnosticSeverity::Error   => (0xff5555u32, 0xffaaaau32),
                    DiagnosticSeverity::Warning => (0xffcc55u32, 0xffe8aau32),
                };
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(
                        div()
                            .text_color(gpui::rgb(badge_color))
                            .text_xs()
                            .font_family("Menlo")
                            .flex_shrink_0()
                            .child(match d.severity {
                                DiagnosticSeverity::Error   => "E",
                                DiagnosticSeverity::Warning => "W",
                            }),
                    )
                    .child(
                        div()
                            .text_color(gpui::rgb(msg_color))
                            .text_xs()
                            .font_family("Menlo")
                            .child(d.message.clone()),
                    )
            }).collect();

            let mut container = div()
                .size_full()
                .overflow_hidden()
                .bg(bg)
                .p_4()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_color(gpui::rgb(0xff5555))
                                .text_sm()
                                .font_family("Menlo")
                                .child(header_label),
                        )
                        .child(
                            div()
                                .px(px(6.0))
                                .py(px(2.0))
                                .bg(gpui::rgb(bg_hover))
                                .rounded(px(4.0))
                                .text_xs()
                                .font_family("Menlo")
                                .text_color(gpui::rgb(text_subtle))
                                .cursor_pointer()
                                .hover(move |s| s.text_color(gpui::rgb(text)))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |_: &mut PreviewPane, _, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            all_text_for_copy.clone(),
                                        ));
                                    }),
                                )
                                .child("copy all"),
                        ),
                );
            for row in diag_rows {
                container = container.child(row);
            }
            return container.into_any_element();
        }

        // ── Image state ───────────────────────────────────────────────────────
        if let (Some(image), Some(px_size)) = (self.image.clone(), self.image_px) {
            // Bounds-capture overlay: a transparent, full-size canvas placed on
            // top of the image.  Its sole purpose is to record the screen-space
            // bounds of this element during each paint pass so the click handler
            // can compute coordinate transforms.
            let bounds_cell = self.preview_bounds.clone();
            let overlay = canvas(
                move |bounds, _window, _app| {
                    bounds_cell.set(Some(bounds));
                },
                |_bounds, _prepaint, _window, _app| {
                    // No painting; we only need the prepaint bounds.
                },
            )
            .absolute()
            .inset_0()
            .size_full();

            // Draw the image via window.paint_image() with manual
            // ObjectFit::Contain computation so we control the exact display
            // rect (needed for accurate coordinate mapping on click).
            let image_canvas = canvas(
                move |bounds, _window, _app| (bounds, image.clone(), px_size),
                |_bounds, state, window, _app| {
                    let (bounds, image, (iw, ih)) = state;
                    let cont_w = f32::from(bounds.size.width);
                    let cont_h = f32::from(bounds.size.height);
                    if cont_w <= 0.0 || cont_h <= 0.0 || iw == 0 || ih == 0 {
                        return;
                    }
                    let scale_factor = window.scale_factor();
                    let log_iw = iw as f32 / scale_factor;
                    let log_ih = ih as f32 / scale_factor;
                    let fit_scale = (cont_w / log_iw).min(cont_h / log_ih);
                    let disp_w = log_iw * fit_scale;
                    let disp_h = log_ih * fit_scale;
                    let ox = f32::from(bounds.origin.x) + (cont_w - disp_w) / 2.0;
                    let oy = f32::from(bounds.origin.y) + (cont_h - disp_h) / 2.0;
                    let img_bounds = Bounds {
                        origin: gpui::point(px(ox), px(oy)),
                        size: gpui::size(px(disp_w), px(disp_h)),
                    };
                    let _ = window.paint_image(
                        img_bounds,
                        Corners::default(),
                        image,
                        0,
                        false,
                    );
                },
            )
            .size_full();

            return div()
                .size_full()
                .relative()
                .bg(bg)
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, window, cx| {
                        this.handle_click(event.position, window, cx);
                    }),
                )
                .child(image_canvas)
                .child(overlay)
                .into_any_element();
        }

        // ── Empty state ───────────────────────────────────────────────────────
        div()
            .size_full()
            .bg(bg)
            .flex()
            .items_center()
            .justify_center()
            .text_color(gpui::rgb(t.text_faint))
            .text_sm()
            .child("No preview — open a .typ file")
            .into_any_element()
    }
}

// ── Link extraction ────────────────────────────────────────────────────────────

/// Walk `frame` and all nested groups, collecting `ockr://`-scheme link
/// annotations into `out`.
///
/// Positions are accumulated in typst-pt coordinates relative to the page
/// top-left.  `GroupItem::transform` is intentionally ignored (identity is
/// assumed) — typst generates pure-translation groups for normal body text,
/// so this is correct for wikilink anchors in prose documents.
fn extract_links(
    frame: &typst::layout::Frame,
    off_x: f32,
    off_y: f32,
    out: &mut Vec<LinkRegion>,
) {
    for (pt, item) in frame.items() {
        let x = off_x + pt.x.to_pt() as f32;
        let y = off_y + pt.y.to_pt() as f32;
        match item {
            FrameItem::Link(Destination::Url(url), size) => {
                let url_str = url.as_str().to_string();
                if url_str.starts_with("ockr://") {
                    out.push(LinkRegion {
                        x_pt: x,
                        y_pt: y,
                        w_pt: size.x.to_pt() as f32,
                        h_pt: size.y.to_pt() as f32,
                        url: url_str,
                    });
                }
            }
            FrameItem::Group(group) => {
                extract_links(&group.frame, x, y, out);
            }
            _ => {}
        }
    }
}

// ── Rasterisation ─────────────────────────────────────────────────────────────

/// Rasterise the first page of `doc` at `PIXELS_PER_PT` resolution.
///
/// Returns `(image, pixel_dimensions)`.  Both are `None` if the document has
/// no pages or the raw bytes do not match the expected RGBA8 layout.
fn rasterize(doc: &PagedDocument) -> (Option<Arc<RenderImage>>, Option<(u32, u32)>) {
    let page = match doc.pages.first() {
        Some(p) => p,
        None => return (None, None),
    };

    // typst-render renders via tiny_skia; returns premultiplied RGBA8.
    let pixmap = typst_render::render(page, PIXELS_PER_PT);
    let w = pixmap.width();
    let h = pixmap.height();

    // typst-render produces premultiplied RGBA8; GPUI's sprite atlas expects BGRA8.
    // Swap R and B channels on each pixel (alpha and green are unchanged).
    let mut bytes = pixmap.data().to_vec();
    for pixel in bytes.chunks_exact_mut(4) {
        pixel.swap(0, 2); // R ↔ B
    }

    let Some(rgba) = image::RgbaImage::from_raw(w, h, bytes) else {
        return (None, None);
    };
    let frame = Frame::new(rgba);
    let image = Arc::new(RenderImage::new(vec![frame]));
    (Some(image), Some((w, h)))
}
