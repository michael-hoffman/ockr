# ockr Keymap

ockr uses **Helix-style modal editing** — select first, then act.
Three modes: **Normal** (default), **Insert**, **Visual**.

---

## Normal Mode

### Movement

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

### Selection

| Key           | Action                              |
|---------------|-------------------------------------|
| `v`           | Enter Visual (character) mode       |
| `V`           | Enter Visual Line mode              |
| `Ctrl-v`      | Enter Visual Block mode             |
| `x`           | Select current line                 |
| `%`           | Select entire file                  |
| `gv`          | Reselect last visual selection      |
| `;`           | Collapse selection to cursor        |
| `_`           | Trim whitespace from selection ends |

### Text Objects (select-then-act)

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

### Operators

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
| `~`           | Toggle case of character under cursor |
| `>` / `<`    | Indent / dedent current line        |
| `.`           | Repeat last change                  |
| `u`           | Undo                                |
| `Ctrl-r`      | Redo                                |

### Insert Entry

| Key           | Action                              |
|---------------|-------------------------------------|
| `i`           | Insert before cursor                |
| `a`           | Insert after cursor                 |
| `I`           | Insert at line start                |
| `A`           | Insert at line end                  |
| `o`           | Open new line below, enter Insert   |
| `O`           | Open new line above, enter Insert   |

### Go-to Prefix (`g`)

| Key           | Action                              |
|---------------|-------------------------------------|
| `gg`          | Go to document start                |
| `G`           | Go to document end                  |
| `gh`          | Go to line start                    |
| `gl`          | Go to line end                      |
| `gs`          | Go to first non-whitespace          |
| `ge`          | Go to next word end                 |
| `gv`          | Reselect last visual selection      |

### Command Palette

| Key           | Action                              |
|---------------|-------------------------------------|
| `:`           | Open command palette                |
| `Cmd-P`       | Open command palette                |

---

## Visual Mode

All Normal-mode **movement** keys extend the selection (anchor stays, cursor moves).

| Key           | Action                              |
|---------------|-------------------------------------|
| `d` or `x`   | Delete selection                    |
| `y`           | Yank selection                      |
| `c`           | Change selection (delete + Insert)  |
| `~`           | Toggle case of selection            |
| `>` / `<`    | Indent / dedent selection           |
| `;`           | Collapse to cursor, return to Normal |
| `_`           | Trim selection whitespace           |
| `v` / `V`    | Switch to Visual Char / Line        |
| `Ctrl-v`      | Switch to Visual Block              |
| `mi` / `ma`  | Select inner / around text object   |
| `Escape`      | Return to Normal                    |

---

## Insert Mode

| Key           | Action                              |
|---------------|-------------------------------------|
| _(any char)_  | Insert character                    |
| `Backspace`   | Delete character before cursor      |
| `Delete`      | Delete character at cursor          |
| `Enter`       | Insert newline                      |
| `←` `→` `↑` `↓` | Move cursor                    |
| `Home` / `End` | Line start / end                   |
| `Ctrl-w`      | Delete previous word                |
| `Escape`      | Return to Normal                    |

---

## App / macOS

| Key           | Action                              |
|---------------|-------------------------------------|
| `Cmd-S`       | Save file                           |
| `Cmd-P`       | Command palette                     |
| `Cmd-V`       | Paste from system clipboard         |
| `Cmd-C`       | Copy selection / line to clipboard  |
| `Cmd-X`       | Cut selection / line to clipboard   |
