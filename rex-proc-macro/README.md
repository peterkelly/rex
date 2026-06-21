# Rex Proc Macros (`rex-proc-macro`)

This crate provides procedural macros for bridging Rust types and Rex values.

## `#[derive(Rex)]`

The derive generates:

- `rex::typesystem::RexType`
- `rex::typesystem::RexAdt`
- `rex::engine::IntoRex`
- `rex::engine::FromRex`
- inherent helper methods such as `inject_rex`, `rex_adt_decl`, and `rex_adt_family`
- an ADT declaration suitable for injection through a `Builder`
- ADT-family discovery so `inject_rex` registers all reachable acyclic derived dependencies

Derived fields of type `Vec<T>` are represented as `List T` and convert to/from Rex lists.

The derive does not implement `rex::engine::RexDefault`; `inject_rex_with_default` is available
only when the type already provides that trait.

In practice this means injecting the top-level derived Rust type is enough for acyclic families of
derived ADTs; manual dependency ordering is no longer required. Cyclic ADT families are still
rejected at registration time.

Leaf types that implement `RexType` / `IntoRex` / `FromRex` but are not `RexAdt`s now work
without any field annotation. The derive uses `RexType::collect_rex_family`, whose default
implementation is a no-op for non-ADT leaves.
