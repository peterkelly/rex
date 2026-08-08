# Rex CLI (`rex_cli`)

This crate provides the `rex_cli` command-line interface.

It is a thin wrapper around the `rex` crate facade for the core pipeline:

`rex-parser` → `rex-typesystem` → `rex-engine`

## Usage

Run a `.rex` file:

```sh
cargo run -p rex-cli --bin rex_cli -- rex-cli/examples/record_update.rex
```

Run a `.rex` file that defines `main` by passing JSON inputs whose top-level
fields match the function parameters:

```sh
cargo run -p rex-cli --bin rex_cli -- path/to/program.rex --inputs path/to/inputs.json
```

```json
{
  "x": 40,
  "y": 2
}
```

Files without a `main` function are evaluated as snippets, conceptually as a
zero-argument entry point. Explicit `main` functions also use this entry point
model; for a zero-argument `main`, pass `{}` as the inputs file.

Inspect the entry point input and result types:

```sh
cargo run -p rex-cli --bin rex_cli -- rex-cli/examples/main_inputs.rex --manifest
```

Run the same example with JSON inputs:

```sh
cargo run -p rex-cli --bin rex_cli -- rex-cli/examples/main_inputs.rex \
  --inputs rex-cli/examples/main_inputs.json
```

Run inline code:

```sh
cargo run -p rex-cli --bin rex_cli -- -c 'map ((*) 2) [1, 2, 3]'
```

Program results are printed as JSON.

For string results, pass `--raw-output` to print the string contents directly:

```sh
cargo run -p rex-cli --bin rex_cli -- --raw-output -c '"hello"'
```

Inspect compiler output:

- `--emit-ast`: print the parsed AST as JSON and exit
- `--emit-type` (alias: `--type`): print the entry point result type as JSON
  and exit
- `--manifest`: print a round-trippable `typeBundle` keyed by input parameter
  names and `result`
- `--inputs <JSON>`: run an explicit `main` with a flat JSON object keyed by
  parameter name
