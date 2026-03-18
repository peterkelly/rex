# Rexlang Core (`rexlang-core`)

This crate provides the core Rust API for embedding Rex.

The standalone CLI now lives in `rexlang-cli`.

`rexlang-core` re-exports the main embedding surface over the core pipeline:

`rex-lexer` → `rex-parser` → `rex-ts` → `rex-engine`

## Usage

Create an engine with the prelude:

```sh
cargo test -p rexlang-core
```
