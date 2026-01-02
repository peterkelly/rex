# Contributing

## Workspace Layout

Rex is a Cargo workspace. The most important crates are:

- `rex-lexer`: tokenization + spans
- `rex-parser`: parsing into a `Program { decls, expr }`
- `rex-ts`: Hindley–Milner inference + type classes + ADTs
- `rex-engine`: typed evaluation + native injection
- `rex-proc-macro`: `#[derive(Rex)]` bridge for Rust types ↔ Rex types/values
- `rex`: CLI binary

Architecture overview: `docs/ARCHITECTURE.md`.

## Development

Run the full test suite:

```sh
cargo test
```

If you edit Rust code, also run:

```sh
cargo fmt
cargo clippy
```

## Lockfiles

This repo commits:

- `Cargo.lock` (workspace lockfile)
- `rex-vscode/package-lock.json` (VS Code extension)

Other lock-like files (for example under `target/` or `node_modules/`) are build artifacts and
should not be committed.

