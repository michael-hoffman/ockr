# ockr Keymap

ockr supports two **keyboard modes**.  Switch between them at any time via the
command palette (`switch-keyboard-mode`):

| Mode | Description |
|------|-------------|
| **Helix** _(default)_ | Modal editing — Normal / Insert / Visual. Select-then-act. |
| **Standard** | Non-modal (VS Code–style). Always "typing" mode; Shift+arrow for selections. |

The active mode is shown in the status bar (`NORMAL` / `INSERT` / `STANDARD`).

---

## Helix Mode

### Normal Mode — Movement

| Key           | Action                              |
|---------------|-------------------------------------|
| `h` `j` `k` `l` | Left · Down · Up · Right         |
| `w`           | Next word start                     |
| `b`           | Previous word start                 |
| `e`           | Next word end                       |
| `W` `B` `E`  | Same, WORD (whitespace-delimited)   |
| `0`           | Line start (column 0)               |
| `^`           | First non-whitespace on line        |
| `$`           | Line end                            |
| `gg`          | Document start                      |
| `G`           | Document end                        |
| `f<c>`        | Find next occurrence of `c` on line |
| `F<c>`        | Find previous occurrence of `c`     |
| `t<c>`        | Move to just before next `c`        |
| `T<c>`        | Move to just after previous `c`     |
| `{`           | Previous paragraph (blank line)     |
| `}`           | Next paragraph (blank line)         |
| `Ctrl-d`      | Scroll half-page down               |
| `Ctrl-u`      | Scroll half-page up                 |
| `Ctrl-f`      | Scroll page down                    |
| `Ctrl-b`      | Scroll page up                      |

### Normal Mode — Selection

| Key           | Action                              |
|---------------|-------------------------------------|
| `v`           | Enter Visual (character) mode       |
| `V`           | Enter Visual Line mode              |
| `Ctrl-v`      | Enter Visual Block mode             |
| `x`           | Select current line                 |
| `X`           | Extend selection to line below      |
| `%`           | Select entire file                  |
| `gv`          | Reselect last visual selection      |
| `;`           | Collapse selection to cursor        |
| `_`           | Trim whitespace from selection ends |

### Normal Mode — Text Objects (select-then-act)

First select with `mi` (inner) or `ma` (around), then `d` / `y` / `c`.

| Key sequence  | Object                              |
|---------------|-------------------------------------|
| `mi w` / `ma w` | Word                              |
| `mi W` / `ma W` | WORD (whitespace-delimited)       |
| `mi p` / `ma p` | Paragraph                         |
| `mi (` / `ma (` | Parentheses `( … )`               |
| `mi {` / `ma {` | Braces `{ … }`                    |
| `mi [` / `ma [` | Brackets `[ … ]`                  |
| `mi <` / `ma <` | Angle brackets `< … >`            |
| `mi "` / `ma "` | Double-quoted string              |
| `mi '` / `ma '` | Single-quoted string              |
| `` mi ` `` / `` ma ` `` | Backtick string          |
| `mi $` / `ma $` | Inline math `$ … $`               |
| `mi t` / `ma t` | Typst content block `[ … ]`       |

### Normal Mode — Operators

| Key           | Action                              |
|---------------|-------------------------------------|
| `d`           | Delete current line                 |
| `D`           | Delete to line end                  |
| `c`           | Change current line (delete + Insert) |
| `C`           | Change to line end                  |
| `y`           | Yank (copy) current line            |
| `p`           | Paste after cursor                  |
| `P`           | Paste before cursor                 |
| `r<c>`        | Replace character under cursor with `c` |
| `R`           | Replace current line with yank register |
| `=`           | Re-indent current line              |
| `~`           | Toggle case of character under cursor |
| `>` / `<`    | Indent / dedent current line        |
| `gc`          | Toggle `// ` comment on current line (or selection in Visual) |
| `.`           | Repeat last change                  |
| `u`           | Undo                                |
| `Ctrl-r`      | Redo                                |
| `Cmd-Z`       | Undo (macOS)                        |
| `Cmd-Shift-Z` | Redo (macOS)                        |

### Normal Mode — Insert Entry

| Key           | Action                              |
|---------------|-------------------------------------|
| `i`           | Insert before cursor                |
| `a`           | Insert after cursor                 |
| `I`           | Insert at line start                |
| `A`           | Insert at line end                  |
| `o`           | Open new line below, enter Insert   |
| `O`           | Open new line above, enter Insert   |

### Normal Mode — Go-to Prefix (`g`)

| Key           | Action                              |
|---------------|-------------------------------------|
| `gg`          | Go to document start                |
| `G`           | Go to document end                  |
| `gh`          | Go to line start                    |
| `gl`          | Go to line end                      |
| `gs`          | Go to first non-whitespace          |
| `ge`          | Go to next word end                 |
| `gv`          | Reselect last visual selection      |
| `gc`          | Toggle `// ` comment on current line |

### Normal Mode — Marks

`m` is already the text-object prefix (`mi`/`ma`), so setting a mark uses
`Alt-m` instead of bare `m`; jumping still uses the familiar bare
backtick/quote. Marks are buffer-local and cleared when a different file
loads into the pane.

| Key           | Action                                             |
|---------------|-----------------------------------------------------|
| `Alt-m<reg>`  | Set mark `<reg>` (any letter) at the cursor          |
| `` `<reg> ``  | Jump to mark `<reg>` (exact position)                |
| `'<reg>`      | Jump to mark `<reg>`'s line (first non-whitespace)   |

### Normal Mode — Search

| Key           | Action                              |
|---------------|-------------------------------------|
| `/`           | Open forward search bar             |
| `?`           | Open backward search bar            |
| `n`           | Jump to next match                  |
| `N`           | Jump to previous match              |
| `Enter`       | Confirm search, close bar           |
| `Escape`      | Cancel search, restore cursor       |
| —             | `[n/M]` match counter shown in search bar; `↩ wrap` appears when navigation wraps around the document |
| `Cmd-F`       | Open forward search (any mode)      |
| `Cmd-H`       | Open find-and-replace bar           |

**Find-and-replace bar** (opened with `Cmd-H`):

| Key           | Action                              |
|---------------|-------------------------------------|
| `Tab`         | Switch focus: query ↔ replace row   |
| `Enter` _(replace row)_ | Replace current match, advance |
| `Ctrl-A` _(replace row)_ | Replace all matches        |
| `Escape`      | Cancel, restore cursor              |

### Normal Mode — Command Palette

| Key           | Action                              |
|---------------|-------------------------------------|
| `:`           | Open command palette                |
| `Cmd-P`       | Open command palette                |

---

### Visual Mode

All Normal-mode **movement** keys extend the selection (anchor stays, cursor moves).

| Key           | Action                              |
|---------------|-------------------------------------|
| `d` or `x`   | Delete selection                    |
| `y`           | Yank selection                      |
| `c`           | Change selection (delete + Insert)  |
| `R`           | Replace selection with yank register |
| `X`           | Extend to include next line         |
| `=`           | Re-indent selected lines            |
| `~`           | Toggle case of selection            |
| `>` / `<`    | Indent / dedent selection           |
| `gc`          | Toggle `// ` comment on selected lines |
| `;`           | Collapse to cursor, return to Normal |
| `Alt-;`       | Flip selection direction            |
| `_`           | Trim selection whitespace           |
| `v` / `V`    | Switch to Visual Char / Line        |
| `Ctrl-v`      | Switch to Visual Block              |
| `mi` / `ma`  | Select inner / around text object   |
| `Escape`      | Return to Normal                    |

#### Surround selection

With text selected, pressing an opening delimiter **wraps** the selection instead of replacing it.

| Key  | Wraps selection with |
|------|----------------------|
| `(`  | `( … )`              |
| `[`  | `[ … ]`              |
| `{`  | `{ … }`              |
| `"`  | `" … "`              |
| `$`  | `$ … $`              |

---

### Insert Mode (Helix)

| Key           | Action                              |
|---------------|-------------------------------------|
| _(any char)_  | Insert character                    |
| `Backspace`   | Delete character before cursor      |
| `Delete`      | Delete character at cursor          |
| `Enter`       | Insert newline                      |
| `←` `→` `↑` `↓` | Move cursor                    |
| `Home` / `End` | Line start / end                   |
| `Ctrl-w`      | Delete previous word                |
| `Ctrl-u`      | Delete from line start to cursor    |
| `Ctrl-k`      | Delete from cursor to line end      |
| `Ctrl-j`      | Insert newline                      |
| `Escape`      | Return to Normal                    |

#### Auto-close pairs

Typing an opening delimiter automatically inserts the closing pair and places the cursor between them.

| Typed         | Result       |
|---------------|--------------|
| `(`           | `(\|)`       |
| `[`           | `[\|]`       |
| `{`           | `{\|}`       |
| `"`           | `"\|"`       |
| `$`           | `$\|$`       |

If the cursor is already directly before the matching closer, the closer is **skipped over** rather than doubled.

**Smart backspace** — when the cursor sits between an empty pair (e.g. `(|)`), `Backspace` removes both delimiters in one keystroke.

---

## Standard Mode

Standard mode is non-modal: the editor is always ready to type.  There are no
Normal / Insert mode transitions.  Use Shift+arrow keys to create and extend
selections, just like VS Code or other "standard" editors.

Switch to Standard mode via the command palette: `switch-keyboard-mode`.

### Movement

| Key                 | Action                          |
|---------------------|---------------------------------|
| `←` `→` `↑` `↓`   | Move cursor                     |
| `Home` / `End`      | Line start / end                |
| `Option-←`          | Move backward one word          |
| `Option-→`          | Move forward one word           |

### Selection (Shift+Arrow)

| Key                    | Action                                           |
|------------------------|--------------------------------------------------|
| `Shift-←` / `Shift-→` | Start or extend character selection left / right |
| `Shift-↑` / `Shift-↓` | Extend selection up / down one line              |
| `Shift-Home` / `Shift-End` | Extend selection to line start / end        |
| `Option-Shift-←`       | Extend selection backward one word              |
| `Option-Shift-→`       | Extend selection forward one word               |
| `Cmd-A`                | Select entire file                              |

**Behaviour matches VS Code:**
- First `Shift-→` immediately creates a 1-character selection and moves the cursor.
- Plain `←` while a selection is active collapses it to the **left** end of the selection.
- Plain `→` while a selection is active collapses it to the **right** end.
- `Escape` collapses any active selection without moving.

### Editing

| Key              | Action                                      |
|------------------|---------------------------------------------|
| _(any char)_     | Insert character                            |
| _(type while selecting)_ | Replace selection with typed character |
| `Backspace`      | Delete character before cursor              |
| `Delete`         | Delete character at cursor                  |
| `Enter`          | Insert newline                              |
| `Tab`            | Insert two spaces                           |
| `Option-Backspace` | Delete previous word                      |
| `Cmd-/`          | Toggle `// ` comment on current line        |
| `Cmd-Z`          | Undo                                        |
| `Cmd-Shift-Z`    | Redo                                        |
| `Cmd-C`          | Copy selection / current line               |
| `Cmd-X`          | Cut selection / current line                |
| `Cmd-V`          | Paste from system clipboard                 |

### Auto-close pairs (Standard Mode)

Same behaviour as Helix Insert mode — see [Auto-close pairs](#auto-close-pairs) above.

---

## App / macOS (all modes)

| Key           | Action                              |
|---------------|-------------------------------------|
| `Cmd-S`       | Save file                           |
| `Cmd-F`       | In-buffer search                    |
| `Cmd-H`       | Find and replace                    |
| `Cmd-P` / `Cmd-Shift-P` | Command palette           |
| `Cmd-V`       | Paste from system clipboard         |
| `Cmd-C`       | Copy selection / line to clipboard  |
| `Cmd-X`       | Cut selection / line to clipboard   |
| `Cmd-O`       | Open vault (folder picker)          |
| `Cmd-N`       | New note                            |
| `Cmd-K`       | Quick switch (fuzzy-open note)      |
| `Cmd-Shift-R` | Recent files                        |
| `Cmd-Shift-K` | Backlinks panel                     |
| `Cmd-Shift-O` | Document outline                    |
| `Cmd-Shift-F` | Vault full-text search              |
| `Cmd-Enter`   | Follow `[[wikilink]]` under cursor  |
| `Cmd-T`       | Open / create today's daily note    |
| `Cmd-B`       | Toggle sidebar                      |
| `Ctrl-Cmd-Z`  | Toggle Zen Mode (distraction-free)  |
| `Cmd-\`       | Split pane vertically               |
| `Cmd-Shift-\` | Split pane horizontally             |
| `Cmd-W`       | Close current tab (or pane when empty) |
| `Cmd-Shift-}` | Next tab                            |
| `Cmd-Shift-{` | Previous tab                        |
| `Ctrl-H/L/K/J` | Focus pane left/right/up/down      |
| `Cmd-Alt-H`   | Toggle HTML ↔ paged preview         |
| `Cmd-Shift-E` | Export PDF                          |
| `Cmd-Shift-G` | Graph view                          |
| `Cmd-Q`       | Quit                                |

---

## Command Palette Commands

Open with `:` (Helix Normal mode) or `Cmd-P` (any mode).
A `:<hint>` shows the Helix ex-command equivalent.

### Editor & View

| ID | Description | Shortcut / Hint |
|----|-------------|-----------------|
| `switch-keyboard-mode` | Toggle between Helix and Standard keyboard mode | — |
| `switch-theme` | Toggle between Oxide (dark) and Ochre (light) theme | — |
| `line-numbers-relative` | Relative line numbers | `:set nu rel` |
| `line-numbers-absolute` | Absolute line numbers | `:set nu abs` |
| `line-numbers-off` | No line numbers | `:set nonu` |
| `reload` | Reload current file from disk | `:e` |
| `reload-settings` | Re-read `settings.toml` without restarting | — |
| `toggle-sidebar` | Show / hide sidebar | `Cmd-B` |
| `toggle-zen-mode` | Zen Mode — hide sidebar + preview, centre the writing column | `Ctrl-Cmd-Z` |
| `toggle-comment` | Toggle `// ` comment on current line / selection | `gc` · `Cmd-/` |

### File & Buffer

| ID | Description | Shortcut / Hint |
|----|-------------|-----------------|
| `new-note` | Create new note | `Cmd-N` |
| `open-vault` | Open vault folder | `Cmd-O` |
| `save-file` | Save | `Cmd-S` · `:w` |
| `save-file-and-quit` | Save and quit | `:wq` |
| `reload` | Reload current file from disk | `:e` |
| `buffer-next` | Switch to next open tab | `Cmd-Shift-}` |
| `buffer-previous` | Switch to previous open tab | `Cmd-Shift-{` |
| `buffer-close` | Close current tab | `Cmd-W` |
| `export-pdf` | Export current document as PDF | `Cmd-Shift-E` |

### Navigation

| ID | Description | Shortcut / Hint |
|----|-------------|-----------------|
| `open-quick-switch` | Quick-switch note | `Cmd-K` |
| `open-recent-files` | Recent files (last 20 opened, most-recent first) | `Cmd-Shift-R` |
| `open-backlinks` | Backlinks panel | `Cmd-Shift-K` |
| `open-outline` | Document outline (headings navigator) | `Cmd-Shift-O` |
| `open-vault-search` | Full-text vault search | `Cmd-Shift-F` |
| `open-daily-note` | Today's daily note | `Cmd-T` |
| `follow-link` | Follow wikilink under cursor | `Cmd-Enter` |
| `open-search` | Open forward search bar | `Cmd-F` |
| `open-replace` | Open find-and-replace bar | `Cmd-H` |

### Views & Panels

| ID | Description | Shortcut / Hint |
|----|-------------|-----------------|
| `open-graph-view` | Graph view | `Cmd-Shift-G` |
| `toggle-preview-mode` | HTML ↔ paged preview | `Cmd-Alt-H` |
| `toggle-zen-mode` | Zen Mode — hides sidebar + preview, centres the writing column at ≤ 800 px | `Ctrl-Cmd-Z` |
| `open-plugin-manager` | Plugin manager (installed plugins + status) | — |
| `split-pane-vertical` | Split editor pane vertically | `Cmd-\` |
| `split-pane-horizontal` | Split editor pane horizontally | `Cmd-Shift-\` |
| `close-pane` | Close the active pane | — |

### App

| ID | Description | Shortcut / Hint |
|----|-------------|-----------------|
| `quit` | Quit | `:q` |
| `force-quit` | Quit without saving | `:q!` |
