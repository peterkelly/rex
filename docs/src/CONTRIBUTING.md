# Contributing

## Workspace Layout

Rex is a Cargo workspace. The most important crates are:

- `rexlang-lexer`: tokenization + spans
- `rexlang-parser`: parsing into a `Program { decls, expr }`
- `rexlang-typesystem`: Hindley–Milner inference + type classes + ADTs
- `rexlang-engine`: typed evaluation + native injection
- `rexlang-proc-macro`: `#[derive(Rex)]` bridge for Rust types ↔ Rex types/values
- `rex`: CLI binary

Architecture overview: [ARCHITECTURE.md](ARCHITECTURE.md).

## Development

Run the full test suite:

```sh
cargo test
```

There is also a lightweight “fuzz smoke” test that runs a deterministic lex→parse→infer→eval loop.
You can scale iterations with `REX_FUZZ_ITERS`:

```sh
REX_FUZZ_ITERS=2000 cargo test -p rex --test fuzz_smoke
```

## Fuzz Harnesses

For end-to-end fuzzing with external fuzzers (AFL++, honggfuzz, custom mutational drivers), the
workspace includes `rexlang-fuzz`, a set of stdin-driven harness binaries:

```sh
cargo build -p rexlang-fuzz --bins
printf '1 + 2' | cargo run -q -p rexlang-fuzz --bin e2e
printf '(' | cargo run -q -p rexlang-fuzz --bin parse
```

Tuning knobs (environment variables):

- `REX_FUZZ_GAS`: gas budget for the harness run
- `REX_FUZZ_MAX_NESTING`: parser nesting cap (defaults to `ParserLimits::safe_defaults()`)
- `REX_FUZZ_STACK_MB`: stack size (MiB) for the harness thread

If you edit Rust code, also run:

```sh
cargo fmt
cargo clippy
```

## Lockfiles

This repo commits:

- `Cargo.lock` (workspace lockfile)
- `rexlang-vscode/package-lock.json` (VS Code extension)

Other lock-like files (for example under `target/` or `node_modules/`) are build artifacts and
should not be committed.
