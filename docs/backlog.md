# ockr Backlog

Deferred features and known gaps, in rough priority order.

---

## After Tabs

### Wikilink hyperlinks in paged / PDF preview
**Status:** Known gap
**Context:** `preprocess_wikilinks` strips `[[Title]]` down to plain display text before
Typst sees it, so paged output has no link annotations.
**Plan:**
1. Emit `#link("ockr://title")[Title]` in `preprocess.rs` instead of bare text.
2. **HTML mode** — the webview already intercepts `ockr://` clicks (follow-link action works).
3. **Paged / PDF mode** — Typst encodes `#link` calls as PDF URI annotations.
   The rasterised preview pane needs a mouse-click handler that reads the annotation
   URL, resolves `ockr://` to a vault file, and opens it (same path as `FollowLink`).
4. **PDF export** (future) — exported files will carry real clickable links automatically
   once step 1 is in place.

---

## Unscoped / Future

### PDF export command
No `export-pdf` command exists yet — only the in-pane paged preview.
**Plan:** `ExportPdf` action → call `typst::export::pdf(doc)` → write `<stem>.pdf`
next to the source file → flash "Exported → foo.pdf" in the status bar.
