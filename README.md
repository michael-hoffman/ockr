# ockr

A fast, keyboard-driven **Typst note editor** for macOS, built with [GPUI](https://www.gpui.rs/).
Think an Obsidian-style linked knowledge base, but native to [Typst](https://typst.app/) instead of Markdown.

ockr pairs a Helix-style modal editing model with live Typst compilation, wikilink
navigation, and a sandboxed WASM plugin system — a writing environment for people
who'd rather keep their hands on the keyboard.

> **Status:** early but usable. Single-author project, macOS-only for now.

---

## Highlights

- **Modal editing** — full Helix-style Normal/Insert/Visual grammar (select-then-act),
  with multi-cursor, counts, text objects, macros, and a Standard (VS Code–style) mode
  for non-modal users. Switch any time via `switch-keyboard-mode`.
- **Live Typst preview** — HTML (fast) or paged/PDF, toggled with `Cmd-Alt-H`. Export to
  PDF with `Cmd-Shift-E`.
- **Wikilinks** — `[[note name]]` with autocomplete, backlinks, and an interactive graph
  view (`Cmd-Shift-G`).
- **LSP** — optional [tinymist](https://github.com/Myriad-Dreamin/tinymist) integration
  for diagnostics, hover (`K`), and go-to-definition (`gd`).
- **Two themes** — Oxide (dark) and Ochre (light); runtime switch via `switch-theme`.
- **Plugins** — sandboxed WASM plugins with typed capability declarations.

See [docs/index.md](docs/index.md) for the full feature list and
[docs/keymap.md](docs/keymap.md) for the complete keymap.

---

## Install

### Download (recommended)

Grab the latest `ockr-<version>.dmg` from
[Releases](https://github.com/michael-hoffman/ockr/releases), open it, and drag
**ockr** to Applications.

The app is ad-hoc signed, so on first launch macOS may warn it's from an
unidentified developer. Right-click the app → **Open** → **Open** to bypass
(only needed once).

### Build from source

Requires the [Rust toolchain](https://rustup.rs/) (1.96+) and macOS 11 or newer.

```sh
git clone https://github.com/michael-hoffman/ockr.git ockr
cd ockr
cargo run --release
```

To produce a distributable `.app` and `.dmg`:

```sh
scripts/bundle.sh            # → dist/ockr.app + dist/ockr-<version>.dmg
scripts/bundle.sh --app-only # skip the .dmg
```

For a signed/notarizable build, set a Developer ID before bundling:

```sh
export CODESIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)"
scripts/bundle.sh
```

### Optional: LSP

Install [tinymist](https://github.com/Myriad-Dreamin/tinymist) and make sure it's
on your `PATH` to enable diagnostics, hover, and go-to-definition. ockr runs fine
without it — LSP features are simply disabled when tinymist isn't found.

---

## Usage

Launch ockr and open a vault (any folder of `.typ` files) via **File → Open Vault**.
Notes are plain Typst documents; wikilinks (`[[…]]`) and backlinks are derived from
their contents.

Key starting points:

| Action                | Key            |
|-----------------------|----------------|
| Command palette       | `Cmd-P` / `:`  |
| Open file (fuzzy)     | `Ctrl-P`       |
| Quick switch (titles) | `Cmd-K`        |
| Toggle preview mode   | `Cmd-Alt-H`    |
| Export PDF            | `Cmd-Shift-E`  |
| Graph view            | `Cmd-Shift-G`  |
| Switch keyboard mode  | palette → `switch-keyboard-mode` |

Full keymap: [docs/keymap.md](docs/keymap.md).

---

## Configuration

Settings load from `~/.config/ockr/settings.toml` (global) overlaid by
`<vault>/.ockr/settings.toml` (per-vault). Custom themes go in
`~/.config/ockr/themes/<name>.toml`.

---

## License

[MIT](LICENSE) © 2026 Michael Hoffman
