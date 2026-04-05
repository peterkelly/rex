# Rex Proc Macros (`rexlang-proc-macro`)

This crate provides procedural macros for bridging Rust types and Rex values.

## `#[derive(Rex)]`

The derive generates:

- an ADT declaration suitable for injection into an `Engine`
- ADT-family discovery so `inject_rex` registers all reachable acyclic derived dependencies
- `IntoPointer` / `FromPointer` implementations to convert between Rust values and Rex runtime values

In practice this means injecting the top-level derived Rust type is enough for acyclic families of
derived ADTs; manual dependency ordering is no longer required. Cyclic ADT families are still
rejected at registration time.

If a field uses a Rust type that should contribute only its `RexType` mapping and conversion logic,
not ADT-family registration, mark that field with `#[rex(type_only)]`. This is useful for manual
leaf types that implement `RexType` / `IntoPointer` / `FromPointer` but are not `RexAdt`s.

The generated code now targets the public `rexlang` crate path, so embedders only need `rexlang`
instead of `rexlang-core`.
