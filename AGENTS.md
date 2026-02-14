# AGENTS.md

## Summary

Rex is a statically-typed functional programming language similar to OCaml and Haskell. It uses
strict evaluation. All expressions and functions are pure and thus free of side effects, allowing
the interpreter to safely evaluate them in parallel. The interpreter uses async functions and tokio.

Rex is intended to be used as a library, embedded in a Rust program (the "host"), for executing
user-supplied scripts (aka workflows) that call native functions provided ("injected") by the host.
The primary use case is executing scientific workflows, where host functions invoke external
pieces of software. Rex provides a language for coordinating these and performing intermediate
computations and data manipulation.

## Pipeline for executing a rex programs

- lexer (`rex-lexer`)
- parser (`rex-parser`)
- type inference (`rex-ts`)
- evaluator (`rex-engine`).

## Crates in this workspace

- `rex`: Library acting as entry point for embedding in other rust programs, CLI tool for testing.
   Also contains examples and integration tests.
- `rex-ast`: shared AST types (`Expr`, `Pattern`, `Decl`, `Program`, symbols).
- `rex-lexer`: tokenizer + spans.
- `rex-parser`: recursive-descent parser producing `Program { decls, expr }`.
- `rex-ts`: Hindley–Milner inference + ADTs + type classes; prelude typeclasses live here.
- `rex-engine`: typed evaluation and runtime intrinsics.
- `rex-proc-macro`: `#[derive(Rex)]` bridge between Rust types and Rex values.
- `rex-fuzz`: stdin-driven fuzz harness binaries.
- `rex-lsp`: language server (used by the VS Code extension).
- `rex-vscode`: VS Code extension (Node).

## Key Files

- `docs/src/ARCHITECTURE.md`: crate pipeline overview.
- `docs/src/SPEC.md`: locked semantics; keep in sync with regression tests.
- `docs/src/EMBEDDING.md`: embedding patterns and untrusted-code checklist.
- `rex-ts/src/prelude_typeclasses.rex`: Rex implementations of typeclass methods.
- `rex-ts/src/prelude.rs`: typeclass/instance injection + primop types.
- `rex-engine/src/prelude.rs`: runtime implementations of primops and builtins.

## Build, Test, Lint

```sh
cargo test
REX_FUZZ_ITERS=2000 cargo test -p rex --test fuzz_smoke
cargo fmt
cargo cargo clippy --tests
```

## CLI Usage

```sh
cargo run -p rex -- run rex/examples/record_update.rex
cargo run -p rex -- run -c 'map ((*) 2) [1, 2, 3]'
```

## LSP + VS Code Extension

```sh
cargo build -p rex-lsp
```
From `rex-vscode/`:
```sh
npm install
code --extensionDevelopmentPath=$(pwd)
```
If the server binary is not in `target/debug/`, set `rex.serverPath` to the `rex-lsp` executable.

## Docs

Documentation is built with [mdBook](https://rust-lang.github.io/mdBook/):

```sh
cd docs
mdbook build  # Output to docs/book/
mdbook serve  # Serve at http://localhost:3000 with auto-reload
```

Install mdBook: `cargo install mdbook`

## Commit messages

- First line should contain a keyword, followed by a colon, a space, then a message that starts
  with a capital letter.
- Acceptable keywords: keywords: feat, fix, docs, style, refactor, test, chore
- First line should be no more than 50 characters in total
- Rest of the commit message should contain a summary of the changes, beginning with the reasons
  why the changes were made.
- When making a commit, leave untracked files untouched
- **IMPORTANT**: Only create commits when explicitly requested by the user. Do not commit automatically after completing tasks.

## Semantics Changes

- Update `docs/src/SPEC.md` when behavior changes.
- Adjust regression tests:
  - `rex/tests/spec_semantics.rs`
  - `rex/tests/record_update.rs`
  - `rex/tests/typeclasses_system.rs`
  - `rex/tests/negative.rs`.

## Guidelines for embedders running untrusted code

- Always cap parsing nesting depth with `ParserLimits::safe_defaults()` (or stricter).
- Always run with a bounded `GasMeter` for parse + infer + eval.
- Prefer async evaluation with `Engine::eval_with_gas`.

## Lockfiles

- Committed: `Cargo.lock`, `rex-vscode/package-lock.json`.
- Do not commit build artifacts under `target/`, `node_modules/`, or `docs/book/`
