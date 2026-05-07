# Rex CLI (`rex`)

This crate provides the `rex` command-line interface.

It is a thin wrapper around the core pipeline:

`rex-parser` → `rex-typesystem` → `rex-engine`

## Usage

Run a `.rex` file as a snippet (declarations plus one expression):

```sh
cargo run -p rex-cli --bin rex -- --snippet rex-cli/examples/record_update.rex
```

Run a declaration-only module file:

```sh
cargo run -p rex-cli --bin rex -- path/to/module.rex
```

Run inline code:

```sh
cargo run -p rex-cli --bin rex -- -c 'map ((*) 2) [1, 2, 3]'
```

Inspect compiler output:

- `--emit-ast`: print the parsed AST as JSON and exit
- `--emit-type` (alias: `--type`): print the inferred type as JSON and exit
