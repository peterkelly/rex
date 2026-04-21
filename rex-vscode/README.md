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

## Semantic Commands

The extension also exposes semantic query and insertion commands (Command Palette, context menu, and keybindings in Rex files):

- `Rex: Show Expected Type At Cursor`
- `Rex: Show Functions Producing Expected Type At Cursor`
- `Rex: Show Functions Accepting Inferred Type At Cursor`
- `Rex: Show Adapters From Inferred To Expected Type At Cursor`
- `Rex: Show Functions Compatible With In-Scope Values At Cursor`
- `Rex: Show Holes And Expected Types`
- `Rex: Show Semantic Loop Step At Cursor`
- `Rex: Apply Semantic Loop Quick-Fix At Cursor`
- `Rex: Apply Best Semantic Loop Quick-Fixes At Cursor`
- `Rex: Preview Best Semantic Loop Quick-Fixes At Cursor (Dry Run)`
- `Rex: Insert Function From Candidates At Cursor`
- `Rex: Insert Function From Candidates At Cursor (With Type Comment)`

Rex now supports first-class hole expressions with `?` (for example: `let x : i32 = ? in x`), and these commands can inspect/fill them.

### Keybindings

- Insert from candidates:
  - macOS: `cmd+alt+i`
  - Windows/Linux: `ctrl+alt+i`
- Insert from candidates (always include type comment):
  - macOS: `cmd+alt+shift+i`
  - Windows/Linux: `ctrl+alt+shift+i`

### Query Output

Semantic query results are written to the `Rex Query Results` output panel (View → Output), including file/cursor context.
Bulk semantic quick-fix runs also include stop details and progress-guard metrics (`stoppedReasonDetail`, diagnostics delta, streak, seen states).

### Settings

- `rex.serverPath`: path to `rex-lsp` (existing setting).
- `rex.insertCandidateTypeComment` (default `false`): when enabled, candidate insertion appends a Rex block comment with the selected function signature.
- `rex.semanticLoopBulkMaxSteps` (default `3`, range `1..20`): max steps for `Rex: Apply Best Semantic Loop Quick-Fixes At Cursor`.
- `rex.semanticLoopBulkStrategy` (default `conservative`, options `conservative|aggressive`): ranking strategy used by bulk semantic quick-fix application.

## LLM Workflow Loop

For agent-assisted workflow authoring, the extension supports a stable refinement loop:

1. Place cursor at the expression/hole you want to fill.
2. Run `Rex: Show Holes And Expected Types` to enumerate unresolved placeholders.
3. Run `Rex: Show Semantic Loop Step At Cursor` for one-shot expected/inferred type, in-scope values, local diagnostics, structured quick-fixes (with stable IDs), and candidate fills.
4. Optionally run `Rex: Show Expected Type At Cursor` and `Rex: Show Functions Producing Expected Type At Cursor` for focused views.
5. Run `Rex: Apply Semantic Loop Quick-Fix At Cursor` for deterministic single-step edits when available.
6. Run `Rex: Apply Best Semantic Loop Quick-Fixes At Cursor` to apply a short server-guided sequence with re-analysis between steps.
7. Optionally run `Rex: Preview Best Semantic Loop Quick-Fixes At Cursor (Dry Run)` to inspect projected steps/stop conditions without changing the file.
8. Run `Rex: Insert Function From Candidates At Cursor` (or the type-comment variant).
9. Re-check diagnostics and repeat until clean.

This keeps generation constrained by Rex type information instead of free-form text edits.

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
