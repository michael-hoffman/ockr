# Plugins

ockr can load **sandboxed WebAssembly plugins** that add commands, panels, and
Typst packages. Plugins declare the capabilities they need up front, and the
host only grants those — a plugin can't touch anything it didn't ask for.

## Managing plugins

Open the **plugin manager** from the activity rail or `open-plugin-manager` in
the palette to see installed plugins and their status.

From a terminal, the `ockr` binary manages plugins for the detected vault:

```sh
ockr install <url>       # install a plugin from a URL
ockr update              # update all installed plugins
ockr list                # list installed plugins
ockr remove <plugin-id>  # remove a plugin
```

Installed plugins are recorded in a lockfile in the vault so the set is
reproducible.

## Capabilities

A plugin's manifest declares typed capabilities; the ones ockr exposes include:

- **network** — HTTP GET via `ockr_http_get`
- **filesystem** — scoped file access
- **console** — logging
- **typst-package** — contribute a Typst package (`@plugin/<name>/lib.typ`) that
  notes can `#import`

Plugins run on a background thread pool via Wasmtime, so a slow or misbehaving
plugin can't block the editor.

## Writing a plugin

The `ockr-plugin` crate is the Rust SDK. Two example plugins live in the
repository, each focused on one capability:

- **`ockr-plugin-example`** — `register_command` + `http_get` (the `console`
  and `network` capabilities): registers two palette commands, one that just
  logs and one that fetches a URL.
- **`ockr-plugin-typst-example`** — `register_typst_package` (the
  `typst-package` capability): contributes a small `callout` function so any
  note can write `#import "@plugin/callout-box/lib.typ": callout` and get an
  Obsidian-style admonition box.

A plugin crate is a `cdylib` with three pieces:

1. **`ockr_plugin::plugin_metadata!`** — declares `id`/`name`/`version` and
   the capabilities the plugin needs (`file_read`, `vault_write`, `network`,
   `console`). The host only grants what's declared here.
2. **`ockr_init`** — called once at load. Call the SDK's registration
   functions here: `register_command`, `register_typst_package`,
   `register_panel`, or just `log`.
3. **`ockr_dispatch_command`** — called when the user runs one of your
   registered commands from the palette. Only needed if you called
   `register_command`; a package-only plugin (like `ockr-plugin-typst-example`)
   can skip it entirely.

Each plugin crate needs its own empty `[workspace]` table in `Cargo.toml` —
without it, `cargo build` from inside the plugin directory fails with
*"current package believes it's in a workspace when it's not"*, since Cargo
otherwise walks up and finds the host app's workspace root. Plugins build for
`wasm32-unknown-unknown`:

```sh
cargo build --target wasm32-unknown-unknown --release
```

(Requires the `wasm32-unknown-unknown` target — `rustup target add
wasm32-unknown-unknown` if `rustc --print target-list` doesn't already show
it as installed.)
