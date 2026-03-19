//! Preview pane — rasterises a typst `PagedDocument` and displays it.
//!
//! ## Rasterisation
//!
//! `typst-render` converts a typst `Page` (which wraps a `Frame`) to a
//! `tiny_skia::Pixmap` (RGBA8). We render at 2 px/pt (≈ 144 PPI), matching
//! a standard 2× HiDPI display.
//!
//! The `Pixmap` bytes are wrapped in `image::Frame` → `gpui::RenderImage` and
//! handed to GPUI's `img()` element, which handles GPU upload and display.
//! Re-rasterisation only occurs when the document changes (`set_document`),
//! not on every GPUI frame.
//!
//! ## References
//! - typst-render crate: <https://docs.rs/typst-render>
//! - GPUI RenderImage / img element: gpui::elements::img

use std::sync::Arc;

use gpui::{Context, ObjectFit, Render, RenderImage, Window, div, img, prelude::*};

use crate::ui::theme;
use image::Frame;
use typst::layout::PagedDocument;

/// Pixels per typst point for rasterisation (2 = 144 PPI, good for 2× HiDPI).
const PIXELS_PER_PT: f32 = 2.0;

/// A GPUI view that renders the first page of a compiled typst document.
///
/// Updated by calling `set_document` (success) or `set_error` (failure).
/// Both methods call `cx.notify()` so GPUI schedules a re-render.
pub struct PreviewPane {
    /// Cached rasterised image. None = no document loaded yet.
    image: Option<Arc<RenderImage>>,
    /// Compiler error / panic message shown in place of the preview.
    error: Option<String>,
}

impl PreviewPane {
    pub fn new() -> Self {
        Self {
            image: None,
            error: None,
        }
    }

    /// Replace the current document.  Rasterises the first page immediately.
    pub fn set_document(&mut self, doc: Arc<PagedDocument>, cx: &mut Context<Self>) {
        self.error = None;
        self.image = rasterize(&doc);
        cx.notify();
    }

    /// Show a compiler error/warning instead of the preview.
    pub fn set_error(&mut self, msg: String, cx: &mut Context<Self>) {
        self.image = None;
        self.error = Some(msg);
        cx.notify();
    }

    /// Reset to the initial empty state (e.g. no file open).
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.image = None;
        self.error = None;
        cx.notify();
    }
}

impl Render for PreviewPane {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let bg = gpui::rgb(theme::BG_PANEL);

        if let Some(ref err) = self.error {
            return div()
                .size_full()
                .bg(bg)
                .p_4()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_color(gpui::rgb(0xff5555))
                        .text_sm()
                        .font_family("Menlo")
                        .child("Compiler error"),
                )
                .child(
                    div()
                        .text_color(gpui::rgb(0xffaaaa))
                        .text_xs()
                        .font_family("Menlo")
                        .child(err.clone()),
                )
                .into_any_element();
        }

        if let Some(ref image) = self.image {
            return div()
                .size_full()
                .bg(bg)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    img(image.clone())
                        .size_full()
                        .object_fit(ObjectFit::Contain),
                )
                .into_any_element();
        }

        div()
            .size_full()
            .bg(bg)
            .flex()
            .items_center()
            .justify_center()
            .text_color(gpui::rgb(theme::TEXT_FAINT))
            .text_sm()
            .child("No preview — open a .typ file")
            .into_any_element()
    }
}

// ── Rasterisation ─────────────────────────────────────────────────────────────

/// Rasterise the first page of `doc` at `PIXELS_PER_PT` resolution.
///
/// Returns `None` if the document has no pages or the raw bytes do not match
/// the expected RGBA8 layout.
fn rasterize(doc: &PagedDocument) -> Option<Arc<RenderImage>> {
    let page = doc.pages.first()?;

    // typst-render renders via tiny_skia; returns premultiplied RGBA8.
    // For fully-opaque typst output the premultiplication is a no-op, so the
    // bytes can be treated as straight RGBA8 for display purposes.
    let pixmap = typst_render::render(page, PIXELS_PER_PT);
    let w = pixmap.width();
    let h = pixmap.height();

    // typst-render produces premultiplied RGBA8; GPUI's sprite atlas expects BGRA8.
    // Swap R and B channels on each pixel (alpha and green are unchanged).
    let mut bytes = pixmap.data().to_vec();
    for pixel in bytes.chunks_exact_mut(4) {
        pixel.swap(0, 2); // R ↔ B
    }

    let rgba = image::RgbaImage::from_raw(w, h, bytes)?;
    let frame = Frame::new(rgba);

    // SmallVec<[Frame; 1]> via From<Vec<_>>
    Some(Arc::new(RenderImage::new(vec![frame])))
}
