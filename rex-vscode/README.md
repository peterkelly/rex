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

## Packaging and Installing (VSIX)

The extension is installable as a `.vsix` file.

From `rex-vscode/`:

```bash
# Make sure runtime dependencies are present (needed at VS Code runtime)
npm ci --omit=dev

# Build a VSIX (requires vsce; use global install or npx)
npx @vscode/vsce package
```

Then install the generated `.vsix`:

```bash
code --install-extension rex-lang-0.0.1.vsix
```

Important:

- The VSIX does **not** automatically include `rex-lsp` unless you ship a prebuilt binary inside
  `rex-vscode/server/`. In the default setup, users must either:
  - install `rex-lsp` into their PATH (e.g. `cargo install --path rex-lsp`), or
  - set `rex.serverPath` to point at the `rex-lsp` executable.

If `vsce` warns about a missing `repository` field, it’s safe to ignore or you can pass
`--allow-missing-repository`.

## Publishing

VS Code Marketplace:

1. Create a publisher matching `package.json`’s `"publisher"`.
2. Create a Personal Access Token (PAT) for publishing.
3. Publish:

```bash
npx @vscode/vsce publish -p "$VSCE_PAT"
```

Open VSX (optional, e.g. for VSCodium):

```bash
npx ovsx publish -p "$OVSX_TOKEN"
```
