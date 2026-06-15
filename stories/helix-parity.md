# Helix Parity Tracker

Tracks which Helix editor operations are implemented in ockr.
Status: ✅ done · 🚧 partial · ❌ not started

---

## Normal Mode — Movement

| Key(s)        | Helix action                        | Status | Notes |
|---------------|-------------------------------------|--------|-------|
| `h`           | Move left                           | ✅     | |
| `j`           | Move down                           | ✅     | |
| `k`           | Move up                             | ✅     | |
| `l`           | Move right                          | ✅     | |
| `w`           | Move to next word start             | ✅     | |
| `b`           | Move to previous word start         | ✅     | |
| `e`           | Move to next word end               | ✅     | |
| `W`           | Move to next WORD start             | ✅     | Whitespace-delimited |
| `B`           | Move to previous WORD start         | ✅     | Whitespace-delimited |
| `E`           | Move to next WORD end               | ✅     | Whitespace-delimited |
| `0`           | Move to line start (col 0)          | ✅     | |
| `^`           | Move to first non-whitespace        | ✅     | |
| `$`           | Move to line end                    | ✅     | |
| `gg`          | Move to document start              | ✅     | Two-key sequence via `pending_g` |
| `G`           | Move to document end                | ✅     | |
| `f<c>`        | Find next char on line              | ✅     | Two-key sequence |
| `F<c>`        | Find prev char on line              | ✅     | Two-key sequence |
| `t<c>`        | Move to before next char on line    | ✅     | Two-key sequence |
| `T<c>`        | Move to before prev char on line    | ✅     | Two-key sequence |
| `{`           | Move to previous paragraph          | ✅     | Blank-line delimited |
| `}`           | Move to next paragraph              | ✅     | Blank-line delimited |
| `%`           | Select entire file                  | ✅     | Enters Visual(Char) across full file |
| `Ctrl-d`      | Scroll half-page down               | ✅     | Moves cursor 20 lines |
| `Ctrl-u`      | Scroll half-page up                 | ✅     | Moves cursor 20 lines |
| `Ctrl-f`      | Scroll page down                    | ✅     | Moves cursor 40 lines |
| `Ctrl-b`      | Scroll page up                      | ✅     | Moves cursor 40 lines |
| `<N>G`        | Go to line N                        | ✅     | Count prefix → `GotoLine(N)` |
| `<N>gg`       | Go to line N                        | ✅     | Count survives `pending_g` |
| `<N>j`        | Move down N lines                   | ✅     | Count `min(500)` |
| `<N>k`        | Move up N lines                     | ✅     | Count `min(500)` |

---

## Normal Mode — Viewport Scroll (`z` prefix)

Repositions viewport, cursor stays put (except `zj`/`zk`, which pull cursor
back into view if it scrolls off). Pure `viewport_top` ops, not `EditorCommand`
— via `KeymapResult::ScrollViewport(ViewportAlign)`.

| Key(s)        | Helix action                        | Status | Notes |
|---------------|-------------------------------------|--------|-------|
| `zt` / `z⏎`   | Cursor line to viewport top         | ✅     | |
| `zz` / `z.`   | Cursor line centred                 | ✅     | |
| `zb` / `z-`   | Cursor line to viewport bottom      | ✅     | |
| `zj`          | Scroll viewport down one line       | ✅     | Cursor clamped into view |
| `zk`          | Scroll viewport up one line         | ✅     | Cursor clamped into view |

---

## Normal Mode — Selection

| Key(s)        | Helix action                        | Status | Notes |
|---------------|-------------------------------------|--------|-------|
| `x`           | Select current line                 | ✅     | Enters Visual(Line) |
| `X`           | Extend selection to line            | ✅     | Normal: select line; Visual Line: extend down |
| `v`           | Enter Visual (char) mode            | ✅     | |
| `V`           | Enter Visual Line mode              | ✅     | |
| `Ctrl-v`      | Enter Visual Block mode             | ✅     | |
| `gv`          | Reselect previous selection         | ✅     | |
| `;`           | Collapse selection to cursor        | ✅     | |
| `Alt-;`       | Flip selection direction            | ✅     | Swaps anchor and cursor |
| `mi<obj>`     | Select inner text object            | ✅     | Helix select-then-act grammar |
| `ma<obj>`     | Select around text object           | ✅     | Objects: w W p ( { [ < " ' \` $ t |
| `C`           | Add cursor below                    | ✅     | Multi-cursor; `state.extra_cursors` |
| `Alt-C`       | Add cursor above                    | ✅     | Multi-cursor |
| `,`           | Keep only primary cursor            | ✅     | Collapse multi-cursor |
| `Alt-,`       | Remove primary cursor               | ✅     | |

---

## Normal Mode — Operators

| Key(s)        | Helix action                        | Status | Notes |
|---------------|-------------------------------------|--------|-------|
| `d`           | Delete (line / selection)           | ✅     | `d` deletes line; `d` in Visual deletes selection |
| `D`           | Delete to line end                  | ✅     | |
| `c`           | Change (line)                       | ✅     | `cc` analogue |
| `C`           | Change to line end                  | ✅     | |
| `y`           | Yank (line)                         | ✅     | `yy` analogue |
| `Y`           | Yank to line end                    | ✅     | |
| `p`           | Paste after                         | ✅     | |
| `P`           | Paste before                        | ✅     | |
| `r<c>`        | Replace char under cursor           | ✅     | Two-key sequence via `pending_replace` |
| `R`           | Replace with yanked text            | ✅     | Register unchanged; works in Normal & Visual |
| `u`           | Undo                                | ✅     | |
| `Ctrl-r`      | Redo                                | ✅     | |
| `>`           | Indent                              | ✅     | Works in Normal (current line) and Visual |
| `<`           | Outdent                             | ✅     | Works in Normal (current line) and Visual |
| `=`           | Format / auto-indent                | ✅     | Re-indents to match previous non-empty line |
| `~`           | Switch case                         | ✅     | Toggles char under cursor (or Visual selection) |
| `.`           | Repeat last change                  | ✅     | Replays last buffer-mutating command |

---

## Normal Mode — Search

| Key(s)        | Helix action                        | Status | Notes |
|---------------|-------------------------------------|--------|-------|
| `/`           | Search forward                      | ✅     | Live-update; Enter confirms, Escape restores cursor |
| `?`           | Search backward                     | ✅     | Same as `/` but initial match is before cursor |
| `n`           | Repeat search forward               | ✅     | Repeats in the original search direction |
| `N`           | Repeat search backward              | ✅     | Repeats in the opposite direction |
| `*`           | Search word under cursor (fwd)      | ✅     | Whole-word match |
| `#`           | Search word under cursor (back)     | ✅     | Whole-word match |
| `:noh`        | Clear search highlights             | ✅     | Palette `noh`/`nohlsearch` |

Persistent highlights: matches stay dimmed after search bar closes (Enter);
`n`/`N` navigate them; cleared by `:noh` or new search.

---

## Normal Mode — Insert Entry

| Key(s)        | Helix action                        | Status | Notes |
|---------------|-------------------------------------|--------|-------|
| `i`           | Insert before selection             | ✅     | |
| `a`           | Insert after selection              | ✅     | |
| `I`           | Insert at line start                | ✅     | |
| `A`           | Insert at line end                  | ✅     | |
| `o`           | Open line below                     | ✅     | |
| `O`           | Open line above                     | ✅     | |

---

## Normal Mode — Goto (`g` prefix)

| Key(s)        | Helix action                        | Status | Notes |
|---------------|-------------------------------------|--------|-------|
| `gg`          | Go to file start                    | ✅     | `<N>gg` → line N |
| `ge`          | Go to word end (forward)            | ✅     | `MoveWordEnd` |
| `gE`          | Go to WORD end (forward)            | ✅     | `MoveWORDEnd` |
| `gl`          | Go to line end                      | ✅     | |
| `gh`          | Go to line start                    | ✅     | |
| `gs`          | Go to first non-whitespace          | ✅     | |
| `gj`          | Move down (visual-line)             | ✅     | `MoveDown` |
| `gk`          | Move up (visual-line)               | ✅     | `MoveUp` |
| `gm`          | Go to middle of line                | ✅     | `GotoMiddleOfLine` |
| `gi`          | Go to last insert position          | ✅     | `GotoLastInsert` |
| `g.`          | Go to last modified position        | ✅     | `GotoLastModified` |
| `gc`          | Toggle comment                      | ✅     | |
| `gf` / `gx`   | Follow link / open file at cursor   | ✅     | `KeymapResult::FollowLink` |
| `gn` / `gp`   | Next / previous buffer              | ✅     | `KeymapResult::BufferNav` |
| `gd`          | Go to definition (LSP)              | ✅     | tinymist `textDocument/definition` |
| `gv`          | Reselect last visual selection      | ✅     | |

---

## LSP (tinymist)

Background JSON-RPC client (`src/lsp/mod.rs`). Disabled silently if `tinymist`
not in `PATH`. `didOpen` on file load, `didChange` on every compile trigger.

| Key(s)        | Action                              | Status | Notes |
|---------------|-------------------------------------|--------|-------|
| `K`           | Hover info popup                    | ✅     | `textDocument/hover`; dismiss on next key |
| `gd`          | Go to definition                    | ✅     | Same-file jump or open target file |
| `[d` / `]d`   | Prev / next diagnostic              | ✅     | Spans LSP + compiler diagnostics |
| —             | Gutter diagnostic stripes           | ✅     | `publishDiagnostics` merged into gutter |

---

## Bracket Navigation (`[` / `]` prefix)

| Key(s)        | Action                              | Status | Notes |
|---------------|-------------------------------------|--------|-------|
| `[d` / `]d`   | Prev / next diagnostic              | ✅     | |
| `[p` / `]p`   | Prev / next paragraph               | ✅     | |

---

## Navigation / Pickers (app-level)

| Key(s)        | Action                              | Status | Notes |
|---------------|-------------------------------------|--------|-------|
| `Ctrl-P`      | Fuzzy file picker (path-based)      | ✅     | `OpenFilePicker`; distinct from Cmd-K |
| `Cmd-K`       | Quick switch (title-based)          | ✅     | `OpenQuickSwitch` |

---

## Visual Mode

| Key(s)        | Helix action                        | Status | Notes |
|---------------|-------------------------------------|--------|-------|
| `h/j/k/l`    | Extend selection                    | ✅     | Moves cursor; selection anchor stays |
| `w/b/e`      | Extend selection by word            | ✅     | |
| `W/B/E`      | Extend selection by WORD            | ✅     | |
| `f/F/t/T<c>` | Extend selection by find-char       | ✅     | |
| `0/$/^`      | Extend to line start/end/first-nws  | ✅     | |
| `G`           | Extend to document end              | ✅     | |
| `{/}`        | Extend to paragraph back/forward    | ✅     | |
| `%`           | Extend to select whole file         | ✅     | |
| `Ctrl-d/u`   | Scroll and extend selection         | ✅     | |
| `Ctrl-f/b`   | Page scroll and extend              | ✅     | |
| `~`           | Switch case of selection            | ✅     | |
| `d` / `x`    | Delete selection                    | ✅     | |
| `y`           | Yank selection                      | ✅     | |
| `c`           | Change selection                    | ✅     | |
| `>`           | Indent selection                    | ✅     | |
| `<`           | Outdent selection                   | ✅     | |
| `;`           | Collapse to cursor, return Normal   | ✅     | |
| `v/V`        | Switch visual sub-mode              | ✅     | |
| `Ctrl-v`     | Switch to Visual Block              | ✅     | |
| `escape`      | Return to Normal                    | ✅     | |
| `mi/ma<obj>` | Select inner/around object          | ✅     | |

---

## Insert Mode

| Key(s)        | Helix action                        | Status | Notes |
|---------------|-------------------------------------|--------|-------|
| Printable     | Insert character                    | ✅     | |
| `Backspace`   | Delete char before cursor           | ✅     | |
| `Delete`      | Delete char at cursor               | ✅     | |
| `Enter`       | Insert newline                      | ✅     | |
| `Escape`      | Return to Normal                    | ✅     | |
| Arrow keys    | Move cursor                         | ✅     | |
| `Home`        | Move to line start                  | ✅     | |
| `End`         | Move to line end                    | ✅     | |
| `Ctrl-w`      | Delete previous word                | ✅     | |
| `Ctrl-u`      | Delete to line start                | ✅     | Insert mode only |
| `Ctrl-k`      | Delete to line end                  | ✅     | Insert mode, no yank |
| `Ctrl-j`      | Insert newline (same as Enter)      | ✅     | |
| `[[fragment`  | Wikilink autocomplete               | ✅     | ockr-specific: popup with Up/Down/Tab/Enter |

---

## macOS / App-level Bindings

| Key(s)        | Action                              | Status | Notes |
|---------------|-------------------------------------|--------|-------|
| `Cmd-S`       | Save file                           | ✅     | |
| `Cmd-V`       | Paste from OS clipboard             | ✅     | |
| `Cmd-C`       | Copy to OS clipboard                | ✅     | |
| `Cmd-X`       | Cut to OS clipboard                 | ✅     | |
| `Cmd-Z`       | Undo (OS-standard)                  | ✅     | Works in all modes |
| `Cmd-Shift-Z` | Redo (OS-standard)                  | ✅     | Works in all modes |
