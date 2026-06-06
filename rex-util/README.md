# Rex Utilities (`rex-util`)

Small helpers shared across crates in this workspace.

Currently includes:

- `sha256_hex`: stable content hashing used by language-tooling snapshots
- `resolve_local_import_path`: CLI-style mapping from module-name segments to `.rex` paths
- `stdlib_source`: embedded pure Rex `std.*` module sources (stored as `.rex` files and included at build time)
