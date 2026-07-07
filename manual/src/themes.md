# Themes

ockr ships two themes and lets you add your own.

| Theme | Variant | Palette |
| --- | --- | --- |
| **Oxide** (default) | dark | warm near-blacks with an ochre (`#CC7722`) accent |
| **Ochre** | light | warm cream and parchment with a deep-ochre accent |

Switch at runtime from the [settings panel](./configuration.md) (<kbd>⌘,</kbd>)
or `switch-theme` in the palette. Your choice persists.

## Custom themes

A theme is a TOML file. Drop one in `~/.config/ockr/themes/<name>.toml` and set
`theme = "<name>"` in your settings. User themes take precedence over the
built-ins.

The simplest start is to copy a built-in
([`oxide.toml`](https://github.com/michael-hoffman/ockr/blob/main/themes/oxide.toml)
or
[`ochre.toml`](https://github.com/michael-hoffman/ockr/blob/main/themes/ochre.toml))
and edit the colors. The keys:

```toml
name    = "My Theme"
variant = "dark"                 # "dark" | "light"

bg_base    = "#0B0907"           # root background
bg_panel   = "#16120E"           # editor / preview panel
bg_surface = "#1B1712"           # sidebar / chrome
bg_hover   = "#241E17"

border        = "#171310"
border_subtle = "#2C251C"

ochre        = "#CC7722"         # accent
ochre_dim    = "#4A2E12"         # selection highlight
ochre_border = "#3D2508"

text        = "#F5F0EA"
text_muted  = "#A89F93"
text_subtle = "#7A7163"
text_faint  = "#57503F"

mode_insert = "#528BFF"
mode_normal = "#CC7722"
mode_visual = "#A855F7"

cursor_fg = "#0B0907"            # text drawn on top of the block cursor

syntax_heading = "#F5F0EA"
syntax_keyword = "#CC7722"
syntax_math    = "#528BFF"
syntax_link    = "#A855F7"
syntax_code    = "#2DD4BF"
syntax_comment = "#57503F"

bracket_match_bg = "#3A2F1A"     # optional
```
