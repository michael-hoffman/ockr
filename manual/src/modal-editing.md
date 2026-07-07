# Modal editing

ockr's default keyboard model is **Helix-style**: you're in a mode, and keys are
commands unless you're in Insert mode. The distinctive Helix trait is
**select-then-act** — a motion *extends the selection*, and an operator acts on
whatever is selected. `w` selects to the next word; `dw` deletes that selection.

If you don't want modes, switch to **Standard** mode (VS Code-style) — see
[Configuration](./configuration.md).

## The modes

| Mode | Enter | Cursor | Purpose |
| --- | --- | --- | --- |
| **Normal** | <kbd>Esc</kbd> | block | Move and run commands (default) |
| **Insert** | `i` `a` `o` … | thin bar | Type text |
| **Visual** | `v` `V` `⌃v` | block | Extend a selection explicitly |

## Motions

Motions move the cursor (and in Visual mode, extend the selection):

- `h j k l` — left / down / up / right
- `w b e` — word start forward / back / end (`W B E` for WORD)
- `0 ^ $` — line start / first non-blank / line end
- `gg G` — document start / end; `<N>G` jumps to line N
- `f F t T <c>` — find / till a character on the line
- `{ }` — previous / next paragraph
- `%` — select the whole file

Counts work: `5j` moves down five lines, `3w` selects three words.

## Operators

An operator acts on the current selection (or takes a motion / text object):

- `d` delete · `c` change (delete + Insert) · `y` yank
- `dd yy cc` — linewise (whole line)
- `p P` — paste after / before · `r<c>` — replace the character
- `> <` — indent / outdent · `=` — auto-indent · `~` — switch case
- `u` undo · <kbd>⌃r</kbd> redo · `.` repeat last change

## Text objects

Select structured regions with `mi` (inner) / `ma` (around) + an object key:

```
mi"   inner double-quotes        ma(   around parentheses
mip   inner paragraph            maw   around word
```

Objects: `w W p ( ) { } [ ] < > " ' ` $ t` (t = Typst tag/element).

## Multi-cursor

- `C` — add a cursor on the line below · `Alt-C` — above
- `,` — collapse back to the primary cursor

## Macros

- `q<reg>` … `q` — record keystrokes into a register
- `@<reg>` — replay · `@@` — replay the last macro

For the exhaustive list, see the [Keymap reference](./keymap.md).
