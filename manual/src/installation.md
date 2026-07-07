# Installation

ockr is macOS-only (it links AppKit and WebKit).

## Download (recommended)

1. Download the latest
   [`ockr-macos.dmg`](https://github.com/michael-hoffman/ockr/releases/latest/download/ockr-macos.dmg).
   All versions are on the
   [releases page](https://github.com/michael-hoffman/ockr/releases).
2. Open the `.dmg` and drag **ockr** to Applications.
3. The app is **ad-hoc signed**, so on first launch macOS warns it's from an
   unidentified developer. Right-click the app → **Open** → **Open**. You only
   need to do this once.

## Build from source

Requires the [Rust toolchain](https://rustup.rs/) (1.96+) and macOS 11 or newer.

```sh
git clone https://github.com/michael-hoffman/ockr.git
cd ockr
cargo run --release
```

To produce a distributable `.app` and `.dmg`:

```sh
scripts/bundle.sh            # → dist/ockr.app + dist/ockr-<version>.dmg
scripts/bundle.sh --app-only # skip the .dmg
```

For a signed, notarizable build, set a Developer ID before bundling:

```sh
export CODESIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)"
scripts/bundle.sh
```

## Optional: the language server

Install [tinymist](https://github.com/Myriad-Dreamin/tinymist) and make sure it
is on your `PATH` to enable diagnostics, hover, go-to-definition, and
completions. ockr runs fine without it — LSP features are simply disabled when
tinymist isn't found. See [Language server](./lsp.md).
