# Rex CLI (`rex`)

This crate provides the `rex` command-line interface.

It is a thin wrapper around the core pipeline:

`rexlang-lexer` → `rexlang-parser` → `rexlang-typesystem` → `rexlang-engine`

## Usage

Run a `.rex` file:

```sh
cargo run -p rexlang-cli -- run rexlang-cli/examples/record_update.rex
```

Run inline code:

```sh
cargo run -p rexlang-cli -- run -c 'map ((*) 2) [1, 2, 3]'
```

Inspect compiler output:

- `--emit-ast`: print the parsed AST as JSON and exit
- `--emit-type` (alias: `--type`): print the inferred type as JSON and exit

## REPL

```sh
cargo run -p rexlang-cli -- repl
```
