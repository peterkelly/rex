# Rex Utilities (`rexlang-util`)

Small helpers shared across crates in this workspace.

Currently includes:

- `sha256_hex`: content hashing used by the library system
- `stdlib_source`: embedded Rex stdlib library sources (stored as `.rex` files and included at build time)
- `GasMeter` / `GasCosts`: simple metering used across parse/type/eval
