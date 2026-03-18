# Rex Proc Macros (`rexlang-proc-macro`)

This crate provides procedural macros for bridging Rust types and Rex values.

## `#[derive(Rex)]`

The derive generates:

- an ADT declaration suitable for injection into an `Engine`
- `IntoValue` / `FromValue` implementations to convert between Rust values and `rexlang_engine::Value`

It’s intended for host applications embedding Rex and exposing domain-specific data types.

