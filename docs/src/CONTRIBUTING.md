# Contributing

## Workspace Layout

Rex is a Cargo workspace. The most important crates are:

- `rex-parser`: source parsing into a `CompilationUnit { decls, body }`
- `rex-ast`: AST nodes, symbols, and spans
- `rex-typesystem`: Hindley–Milner inference + type classes + ADTs
- `rex-engine`: host environment building, compilation, evaluation, and native injection
- `rex-proc-macro`: derives and registration attributes for documented Rust types, functions, and
  modules exposed to Rex
- `rex`: top-level embedding facade and integration tests
- `rex-cli`: `rex_cli` command-line binary

Architecture overview: [ARCHITECTURE.md](ARCHITECTURE.md).

## Development

Run the full test suite:

```sh
cargo test
```

There is also a lightweight “fuzz smoke” test that runs a deterministic parse→infer→eval loop.
You can scale iterations with `REX_FUZZ_ITERS`:

```sh
REX_FUZZ_ITERS=2000 cargo test -p rex --test fuzz_smoke
```

## Fuzz Harnesses

For end-to-end fuzzing with external fuzzers (AFL++, honggfuzz, custom mutational drivers), the
workspace includes `rex-fuzz`, a set of stdin-driven harness binaries:

```sh
cargo build -p rex-fuzz --bins
printf '1 + 2' | cargo run -q -p rex-fuzz --bin e2e
printf '(' | cargo run -q -p rex-fuzz --bin parse
```

Tuning knobs (environment variables):

- `REX_FUZZ_STACK_MB`: stack size (MiB) for the harness thread

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
