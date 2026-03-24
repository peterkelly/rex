# Rex Fuzz Harnesses (`rexlang-fuzz`)

This crate contains small stdin-driven binaries used for fuzzing and regression testing.

## Binaries

- `parse`: tokenize + parse a single input (parser-focused coverage)
- `e2e`: tokenize + parse + typecheck + eval a single input (end-to-end coverage)

## Running

```sh
# Parse only
cargo run -p rexlang-fuzz --bin parse < path/to/input

# Full pipeline
cargo run -p rexlang-fuzz --bin e2e < path/to/input
```

## Environment knobs

- `REX_FUZZ_STACK_MB`: per-input thread stack size
- `REX_FUZZ_MAX_NESTING`: parser nesting cap override
- `REX_FUZZ_GAS`: total gas budget (parse/type/eval depending on binary)

