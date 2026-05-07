# Rex Fuzz Harnesses (`rex-fuzz`)

This crate contains small stdin-driven binaries used for fuzzing and regression testing.

## Binaries

- `parse`: parse a single source input (parser-focused coverage)
- `e2e`: parse + typecheck + eval a single source input (end-to-end coverage)

## Running

```sh
# Parse only
cargo run -p rex-fuzz --bin parse < path/to/input

# Full pipeline
cargo run -p rex-fuzz --bin e2e < path/to/input
```

## Environment knobs

- `REX_FUZZ_STACK_MB`: per-input thread stack size
