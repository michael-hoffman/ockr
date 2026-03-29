# ockr

A fast, modal Typst note editor built with GPUI.

---

## Features

- **Two keyboard modes** — **Helix** (modal, select-then-act) or **Standard** (VS Code–style, Shift+arrow selection); switch any time via `switch-keyboard-mode` in the command palette
- **Runtime theme switching** — **Oxide** (dark) and **Ochre** (light); toggle via `switch-theme` in the command palette; persists across restarts
- **Live Typst preview** — HTML (fast) or paged/PDF, toggled with `Cmd-Alt-H`
- **Wikilinks** — `[[note name]]` navigation with autocomplete and backlinks
- **Graph view** — interactive note-relationship graph (`Cmd-Shift-G`)
- **Plugin system** — sandboxed WASM plugins with typed capability declarations; network (`ockr_http_get`), filesystem, console, and typst-package capabilities; plugin manager panel via `open-plugin-manager`
- **In-buffer search & replace** — `/` · `?` · `Cmd-F` · `Cmd-H`
- **Auto-close pairs** — `(` `[` `{` `"` `$` with smart backspace
- **Document stats** — live word count, character count, and line count in the status bar; when a selection is active, selected word / char count is shown alongside
- **Tab persistence** — open tabs and undo history survive restarts

## Quick start

```
cargo run
```

Open a vault with `Cmd-O` or `open-vault` from the command palette.

## Documentation

- [Keymap reference](keymap.md)
- [Backlog](backlog.md)
