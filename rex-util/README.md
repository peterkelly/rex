# Rex Utilities (`rex-util`)

Small helpers shared across crates in this workspace.

Currently includes:

- `sha256_hex`: content hashing used by the module system
- `stdlib_source`: embedded Rex `std.*` module sources (stored as `.rex` files and included at build time)
- `GasMeter` / `GasCosts`: simple metering used across parse/type/eval
