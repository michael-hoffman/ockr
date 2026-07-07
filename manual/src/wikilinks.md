# Wikilinks & the graph

ockr turns a folder of notes into a linked knowledge base.

## Wikilinks

Type `[[` to start a wikilink; an autocomplete popup offers matching note titles.
Accept with <kbd>Tab</kbd> or <kbd>Enter</kbd>:

```typst
See [[portal-gun-paradox]] and [[Dr-Wong]] for context.
```

A wikilink resolves to another `.typ` file in the vault by its title (the file
stem). In the editor, put the cursor on a link and press `gf` to follow it; in
the preview, click it.

Under the hood ockr preprocesses `[[name]]` into a real Typst
`#link("ockr://…")[name]`, so links work in both the HTML and paged/PDF preview.

## Backlinks

Open the **backlinks** panel (activity rail, or `open-backlinks`) to see every
note that links *to* the current one. The index is built in the background when
you open a vault and updated as you edit.

## Graph view

Press <kbd>⌘⇧G</kbd> (or `open-graph-view`) for an interactive graph of the whole
vault — nodes are notes, edges are wikilinks. Click a node to open that note.
It's a fast way to spot clusters and orphans.

## Outline

Within a note, <kbd>⌘⇧O</kbd> (`open-outline`) lists every heading with its level
and line number. Click or press <kbd>Enter</kbd> to jump.
