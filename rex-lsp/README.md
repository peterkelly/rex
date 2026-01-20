# Rex Language Server (`rex-lsp`)

This crate builds the `rex-lsp` binary: a Language Server Protocol (LSP) implementation for Rex,
built on `tower-lsp`.

It’s designed to run over stdio (the usual editor integration path), and it powers the VS Code
extension in `rex-vscode/`.

## Build

```sh
cargo build -p rex-lsp
```

## Run (stdio)

Most users don’t run this directly; editors spawn it. For debugging, you can run it under an LSP
client that speaks stdio.

