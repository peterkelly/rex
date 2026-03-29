# Rex Proc Macros (`rexlang-proc-macro`)

This crate provides procedural macros for bridging Rust types and Rex values.

## `#[derive(Rex)]`

The derive generates:

- an ADT declaration suitable for injection into an `Engine`
- `IntoPointer` / `FromPointer` implementations to convert between Rust values and Rex runtime values

The generated code now targets the public `rexlang` crate path, so embedders only need `rexlang`
instead of `rexlang-core`.
