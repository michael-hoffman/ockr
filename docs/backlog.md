# ockr Backlog

Deferred features and known gaps, in rough priority order.

---

## Paged / PDF Mode

### Wikilink hyperlinks in paged / PDF preview
**Status:** ✅ Done — HTML mode and paged mode both working.
**Implementation:** `preprocess_wikilinks` emits `#link("ockr://rel/path.typ")[Title]`.
Typst stores these as `FrameItem::Link(Destination::Url, Size)` in the compiled frame.
`PreviewPane::set_document` extracts these annotations (recursing into `FrameItem::Group`),
stores them as `LinkRegion` values in typst-pt coordinates.  A GPUI canvas overlay
captures the element's screen bounds each paint pass; `on_mouse_down` on the container
maps the click through the `ObjectFit::Contain` transform back to typst-pt coords and
hit-tests against stored regions.  A match emits `PreviewEvent::OpenLink(url)` which
`MainWindow` subscribes to and routes to `open_path`, mirroring the HTML wikilink path.

---

## Unscoped / Future

### PDF export command
No `export-pdf` command exists yet — only the in-pane paged preview.
**Plan:** `ExportPdf` action → call `typst::export::pdf(doc)` → write `<stem>.pdf`
next to the source file → flash "Exported → foo.pdf" in the status bar.
