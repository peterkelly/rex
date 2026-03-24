# Rex Language Server (`rexlang-lsp`)

This crate builds the `rexlang-lsp` binary: a Language Server Protocol (LSP) implementation for Rex,
built on `tower-lsp`.

It’s designed to run over stdio (the usual editor integration path), and it powers the VS Code
extension in `rexlang-vscode/`.

## Build

```sh
cargo build -p rexlang-lsp
```

## Run (stdio)

Most users don’t run this directly; editors spawn it. For debugging, you can run it under an LSP
client that speaks stdio.

