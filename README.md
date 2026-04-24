# 🦖 Rex

[![MIT licensed][mit-badge]][mit-url]
[![Docs][docs-badge]][docs-url]

[mit-badge]: https://img.shields.io/badge/license-MIT-blue.svg
[mit-url]: https://github.com/peterkelly/rex/blob/master/LICENSE
[docs-badge]: https://img.shields.io/badge/docs-online-blue
[docs-url]: https://peterkelly.github.io/rex/

<p align="center">
  <img src="logo.jpg" width="400">
</p>

Rex (short for *[Rush](https://rush.cloud/) Expressions*) is a strongly-typed,
pure, implicitly parallel functional programming language built to be an
excellent target for LLM-generated programs, with a focus on scientific
workflows and data analysis pipelines. At a high level, you write
transformations over lists, records, ADTs, and other values using familiar
functional building blocks like `map`, `filter`, folds, pattern matching, and
composition. The language is designed to make dataflow clear and predictable,
with types and pure expressions doing most of the heavy lifting.

Rex is designed first and foremost to be embedded inside Rust applications. In
that model, your Rust program acts as the host runtime and injects native
functions into Rex so scripts can orchestrate real work while staying in a
concise, declarative style. This makes Rex a practical scripting layer for
workflow-style systems where you want strong typing and explicit control at the
host boundary.

Because Rex programs are pure and free of side effects in the language itself,
the runtime can safely execute host-provided async functions in parallel when
it is valid to do so. In practice, that means users can write straightforward
functional code and still benefit from concurrency without directly managing
threads, locks, or low-level async orchestration.

If you are using Rex as a code-generation target, read
**[LLM Guidance](docs/src/LLMS.md)** early. It captures syntax pitfalls and
validation workflow that reduce iteration time.

## Example

```rex
let
  values = [3, 12, 7, 20, 15, 4],
  selected = filter (\n -> n >= 10) values,
  adjusted = map (\n -> n - 2) selected,
  total = foldl (\acc n -> acc + n) 0 adjusted
in
  (values, selected, adjusted, total)
```

[Try it yourself in the interactive browser-based playground](https://peterkelly.github.io/rex/)

## Rex as a target for LLMs

<p align="center">
  <img src="agents_rex.jpg" width="600">
</p>

Rex is the world’s first parallel functional language explicitly designed to be
a useful target for LLMs. Its strong static type system gives rapid,
high-signal feedback on generated programs, so both users and models can
quickly identify mismatches and converge on correct code.

That typechecking loop works especially well with Rex’s functional,
expression-oriented style. Because programs are written as pure data
transformations, LLM-generated code tends to be easier to inspect, reason
about, and refine than imperative scripts with hidden state or side effects.

Together, these properties make Rex a strong fit for LLM-generated data
analysis pipelines and scientific workflows. Models can generate high-level
orchestration in Rex, while host-provided Rust functions handle domain-specific
execution, giving a clean split between deterministic workflow logic and host
capabilities.

## Documentation

[https://peterkelly.github.io/rex/](https://peterkelly.github.io/rex/)

## Current status

The `main` branch of this repository contains what will soon become Rex v4.
Currently it is marked as 3.9.x until the API has stabilized, at which point
the version number will be updated.

The most recent stable (and production-ready) version is available at
https://github.com/talo/rex.

## Crates

This repo is a Cargo workspace. The key crates are:

- `rex-lexer`: tokenization (+ spans)
- `rex-parser`: parser producing a `Program { decls, expr }`
- `rex-typesystem`: Hindley–Milner type inference + type classes + ADTs
- `rex-engine`: runtime evaluator + native-function injection, backed by `rex-typesystem`
- `rex-proc-macro`: `#[derive(Rex)]` for bridging Rust types ↔ Rex ADTs/values
- `rex-cli`: CLI crate providing the `rex` binary (`cargo run -p rex-cli -- ...`)
- `rex-fuzz`: stdin-driven fuzz harness binaries
- `rex-util`: small shared helpers (e.g. module hashing, bundled `std.*` module sources)
- `rex-lsp` / `rex-vscode`: language tooling (LSP + VS Code extension)

## CLI

Run a file:

```sh
cargo run -p rex-cli -- run rex-cli/examples/record_update.rex
```

Run the advanced module import example:

```sh
cargo run -p rex-cli -- run rex-cli/examples/modules_advanced/main.rex
```

Run inline code:

```sh
cargo run -p rex-cli -- run -c 'map ((*) 2) [1, 2, 3]'
```

Other useful flags:

- `--emit-ast`: print parsed AST as JSON and exit
- `--emit-type` (alias: `--type`): print inferred type as JSON and exit
- `--stdin`: read a program from stdin
- `--stack-size-mb`: control the runner thread stack size
- `--max-nesting`: cap syntactic nesting depth during parsing
- `--no-max-nesting`: disable the parsing nesting cap
- `--gas`: total gas budget for parse/type/eval
- `--no-gas`: disable gas metering

## Standard Module (Prelude)

Rex ships with a prelude that provides core functions, types, and type classes
used throughout the language (for example mapping, filtering, folds, numeric
operations, equality/ordering, and container abstractions).

The prelude surface definitions and type-class wiring live primarily in:

- `rex-typesystem/src/prelude_typeclasses.rex`
- `rex-typesystem/src/prelude.rs`
- `rex-engine/src/prelude.rs`

For full details and usage patterns, see the [docs](https://peterkelly.github.io/rex/):

- [Prelude tour](https://peterkelly.github.io/rex/tutorial/section1/12_prelude_tour.html)
- [Language reference](https://peterkelly.github.io/rex/LANGUAGE.html)
- [LLM usage guidelines](https://peterkelly.github.io/rex/LLMS.html)

Made with ❤️ by [QDX](https://qdx.co/)
