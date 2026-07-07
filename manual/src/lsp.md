# Language server (LSP)

ockr integrates with [tinymist](https://github.com/Myriad-Dreamin/tinymist), the
Typst language server, for real language intelligence. It's **optional**: if
tinymist isn't installed, these features are silently disabled and everything
else works normally.

## Setup

Install tinymist and make sure it's on your `PATH`, then restart ockr:

```sh
# e.g. with Homebrew
brew install tinymist
```

When connected, the status bar shows an `lsp` indicator.

## Features

| Feature | Key | Notes |
| --- | --- | --- |
| Diagnostics | — | Errors/warnings underlined and marked in the gutter; count in the status bar |
| Hover | `K` | Documentation popup for the symbol under the cursor |
| Go to definition | `gd` | Jumps to the definition, opening the target file if needed |
| Completions | <kbd>⌃Space</kbd> | Popup; <kbd>↑</kbd>/<kbd>↓</kbd> or <kbd>⌃p</kbd>/<kbd>⌃n</kbd> to navigate, <kbd>Tab</kbd>/<kbd>Enter</kbd> to accept |
| Diagnostic jump | `[d` `]d` | Previous / next diagnostic (compiler + LSP merged) |

If you invoke `K` or `gd` without tinymist installed, ockr shows a hint telling
you how to enable it rather than doing nothing.

## Compiler diagnostics

Independently of tinymist, ockr's built-in Typst compiler reports its own
errors and warnings live as you type. These are merged with the LSP's
diagnostics in the gutter and the status-bar count.
