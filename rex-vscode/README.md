# Rex VS Code Extension

This folder contains a standalone VS Code extension for the Rex language. It provides:

- Syntax highlighting for `.rex` files.
- A Rust language server (LSP) with basic hover help and block-comment diagnostics.

## Development

From the repository root, build the language server:

```bash
cargo build -p rex-lsp
```

Then, from this folder:

```bash
npm install
code --extensionDevelopmentPath=$(pwd)
```

Open a `.rex` file to activate the extension. The language server reports unmatched `{-` and `-}` comment delimiters.

If the server binary is not in `../target/debug/`, set `rex.serverPath` to the full path of the `rex-lsp` executable or install it into your PATH with:

```bash
cargo install --path rex-lsp
```
