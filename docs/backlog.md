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
**Status:** ✅ Done — `Cmd-Shift-E` / `export-pdf` in the command palette.
**Implementation:** `ExportPdf` action → stores last `Arc<PagedDocument>` on each paged
compile result → calls `typst_pdf::pdf(doc, options)` → writes `<stem>.pdf` beside the
source file → shows a transient toast overlay ("Exported → foo.pdf").
