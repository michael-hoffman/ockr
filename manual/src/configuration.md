# Configuration

## Settings panel

Press <kbd>⌘,</kbd> (or **Settings…** in the app menu / `settings` in the
palette) for a GUI over the most-used options — keyboard mode, theme, line
numbers, and preview format. Each row cycles on click, applies immediately, and
is saved.

## Settings files

Settings resolve from two optional TOML files, vault overriding global:

1. **Global** — `~/.config/ockr/settings.toml`
2. **Vault** — `<vault>/.ockr/settings.toml`

Every key is optional; missing keys fall back to defaults.

```toml
# ~/.config/ockr/settings.toml
keyboard_mode    = "helix"      # "helix" | "standard"
theme            = "oxide"      # "oxide" (dark) | "ochre" (light) | custom
font_size        = 14.0
line_number_mode = "relative"   # "relative" | "absolute" | "off"
preview_mode     = "html"       # "html" | "paged"
soft_wrap        = true
tab_size         = 2
auto_save        = false
show_word_count  = true
```

Reload after editing with `reload-settings` in the palette (or reopen the app).

## Sessions

ockr persists your last vault, open tabs, per-file undo history, and recent
files between launches — no configuration needed.
