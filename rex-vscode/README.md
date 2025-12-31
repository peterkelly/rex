# Rex VS Code Extension

This folder contains a standalone VS Code extension for the Rex language. It provides:

- Syntax highlighting for `.rex` files.
- A Rust language server (LSP) with hover, completion, go-to-definition, and block-comment diagnostics.

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

Go to definition works on:
- Local bindings (`let`, lambda params, match pattern vars).
- Top-level function declarations (`fn`).
- User-defined ADT names and constructors from `type` declarations.

### Troubleshooting (when nothing shows up)

1. Make sure VS Code thinks the file is **Rex** (bottom-right language mode should say `Rex`).
2. Build the server: `cargo build -p rex-lsp` (from the repo root).
3. Reload VS Code (Command Palette → “Developer: Reload Window”).
4. Check logs: View → Output → select **Rex Language Server**.
5. If the server binary isn’t found, set `rex.serverPath` to the full path of `target/debug/rex-lsp`.

If the server binary is not in `../target/debug/`, set `rex.serverPath` to the full path of the `rex-lsp` executable or install it into your PATH with:

```bash
cargo install --path rex-lsp
```
