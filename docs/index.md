# ockr

A fast, modal Typst note editor built with GPUI.

---

## Features

- **Modal editing** — Helix-style Normal / Insert / Visual modes
- **Live Typst preview** — HTML (fast) or paged/PDF, toggled with `Cmd-Alt-H`
- **Wikilinks** — `[[note name]]` navigation with autocomplete and backlinks
- **Graph view** — interactive note-relationship graph (`Cmd-Shift-G`)
- **Plugin system** — sandboxed WASM plugins with typed capability declarations; network (`ockr_http_get`), filesystem, console, and typst-package capabilities; plugin manager panel via `open-plugin-manager`
- **In-buffer search & replace** — `/` · `?` · `Cmd-F` · `Cmd-H`
- **Auto-close pairs** — `(` `[` `{` `"` `$` with smart backspace
- **Tab persistence** — open tabs and undo history survive restarts

## Quick start

```
cargo run
```

Open a vault with `Cmd-O` or `open-vault` from the command palette.

## Documentation

- [Keymap reference](keymap.md)
- [Backlog](backlog.md)
