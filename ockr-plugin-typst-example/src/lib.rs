//! Callout Box ockr plugin — demonstrates the `typst-package` capability.
//!
//! Registers a small Typst package with a `callout` function for
//! Obsidian-style admonition boxes. Any note can pull it in with:
//!
//! ```typst
//! #import "@plugin/callout-box/lib.typ": callout
//!
//! #callout(kind: "warning")[Don't forget to back up first.]
//! ```

use ockr_plugin::{log, register_typst_package};

ockr_plugin::plugin_metadata! {
    id: "callout-box",
    name: "Callout Box",
    version: "0.1.0",
    capabilities: [console],
}

/// A colored, left-bordered admonition box. `kind` is `"note"` (default),
/// `"warning"`, or `"danger"` — anything else falls back to `"note"`.
const CALLOUT_TYPST: &str = r#"
#let callout(kind: "note", body) = {
  let (color, label) = if kind == "warning" {
    (orange, "Warning")
  } else if kind == "danger" {
    (red, "Danger")
  } else {
    (blue, "Note")
  }
  block(
    fill: color.lighten(85%),
    stroke: (left: 3pt + color),
    inset: 10pt,
    radius: 4pt,
    width: 100%,
  )[
    #text(weight: "bold", fill: color)[#label]
    #body
  ]
}
"#;

#[no_mangle]
pub extern "C" fn ockr_init() {
    register_typst_package("lib.typ", CALLOUT_TYPST);
    log("callout-box plugin loaded — #import \"@plugin/callout-box/lib.typ\": callout");
}

// No commands registered, so the host never looks up ockr_dispatch_command —
// it's only exported by plugins that call register_command (see
// ockr-plugin-example).
