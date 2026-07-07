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

The `ockr-plugin` crate is the Rust SDK; `ockr-plugin-example` in the repository
is a minimal working plugin. Plugins build for `wasm32-unknown-unknown`:

```sh
cargo build --target wasm32-unknown-unknown --release
```
