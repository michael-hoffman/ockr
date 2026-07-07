# ockr

**ockr** is a Typst-native, keyboard-driven note editor for macOS. Think an
Obsidian-style linked knowledge base, but native to
[Typst](https://typst.app) instead of Markdown — so every note is a real
typesetting document with math, tables, and templates, edited the way you'd
edit code.

It pairs a Helix-style modal editing model with live Typst compilation,
wikilink navigation, an interactive graph of your vault, and a sandboxed WASM
plugin system.

## What makes it different

- **Notes are Typst.** A vault is a folder of `.typ` files. You get real math
  (`$x^2$`), tables, figures, and `#import`-able templates — not a Markdown
  subset.
- **Modal by default.** Full Helix grammar: select-then-act motions, text
  objects, multi-cursor, counts, macros, a jump list. A Standard (VS Code-style)
  mode is one toggle away if you don't want modes.
- **Live preview.** Every keystroke recompiles in the background — HTML for
  speed, paged for fidelity, PDF export on demand.
- **Linked knowledge.** `[[wikilinks]]` with autocomplete, per-note backlinks,
  and a whole-vault graph.
- **Plain files, no lock-in.** Everything is on your disk in a standard format.
  Nothing is uploaded anywhere.

## About this manual

Start with [Installation](./installation.md), then
[Getting started](./getting-started.md). If you already know Helix or Vim, skim
[Modal editing](./modal-editing.md) and keep the
[Keymap reference](./keymap.md) handy.

---

ockr is free and open source under the MIT license. Source, releases, and issue
tracker: <https://github.com/michael-hoffman/ockr>.

> Obsidian and Typst are trademarks of their respective owners. ockr is an
> independent project, not affiliated with or endorsed by either.
