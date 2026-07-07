# Getting started

## Open a vault

A **vault** is any folder of `.typ` files. On first launch ockr shows a welcome
pane — click **Open Vault** (or press <kbd>⌘O</kbd>) and pick a folder. ockr
remembers it and reopens it next time.

The window has four regions:

- **Activity rail** (far left) — icon launchers for files, search, graph,
  outline, backlinks, plugins, and settings.
- **Sidebar** — the file tree. Click a file to open it; right-click for
  rename / reveal / delete. The `＋` button creates a new note.
- **Editor** — one or more tabs. Split with <kbd>⌘\\</kbd> (vertical) or
  <kbd>⌘⇧\\</kbd> (horizontal).
- **Preview** — the live-compiled document, sharing the window with the editor.

## Create and edit a note

Press <kbd>⌘N</kbd> (or the sidebar `＋`) to create a note. ockr starts in
**Normal** mode (Helix); press <kbd>i</kbd> to insert text, <kbd>Esc</kbd> to
return to Normal. If you'd rather not think about modes, switch to Standard mode
(below).

Notes are Typst, so:

```typst
= My first note

Some prose with inline math $E = m c^2$ and a link to [[another-note]].

#figure(
  table(columns: 2, [Key], [Action], [`gd`], [Go to definition]),
  caption: [A small table],
)
```

The preview updates as you type. Toggle HTML ↔ paged preview with
<kbd>⌘⌥H</kbd>; export a PDF with <kbd>⌘⇧E</kbd>.

## Move around

- **Files:** <kbd>⌃P</kbd> fuzzy file picker (by path), <kbd>⌘K</kbd> quick
  switch (by title), <kbd>⌘⇧F</kbd> vault-wide search.
- **Commands:** <kbd>⌘P</kbd> (or <kbd>:</kbd> in Normal mode) opens the command
  palette — every action is searchable there.
- **Within a note:** Helix motions (see [Modal editing](./modal-editing.md)),
  plus <kbd>⌘⇧O</kbd> for the outline and <kbd>⌃o</kbd>/<kbd>⌃i</kbd> for the
  jump list.

## Switch keyboard modes

ockr ships two keyboard models:

| Mode | Feel |
| --- | --- |
| **Helix** (default) | Modal — Normal / Insert / Visual, select-then-act |
| **Standard** | Non-modal, VS Code-style; Shift+arrow to select |

Switch any time from the command palette (`switch-keyboard-mode`) or the
[settings panel](./configuration.md) (<kbd>⌘,</kbd>).
