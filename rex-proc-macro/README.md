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

Rust doc comments on the derived type, type parameters, enum variants, tuple fields, and named
fields are preserved in the generated semantic ADT declaration. For a struct, the type docs also
document its single constructor variant. This metadata is available to embedders and survives
named-module installation and conversion to the `TypeBundle` wire format. Rustdoc itself does not
render generic-parameter documentation, so put `#[allow(unused_doc_comments)]` on a documented
generic parameter when warnings are denied.

The derive does not implement `rex::engine::RexDefault`; `inject_rex_with_default` is available
only when the type already provides that trait.

In practice this means injecting the top-level derived Rust type is enough for acyclic families of
derived ADTs; manual dependency ordering is no longer required. Cyclic ADT families are still
rejected at registration time.

Leaf types that implement `RexType` / `IntoRex` / `FromRex` but are not `RexAdt`s now work
without any field annotation. The derive uses `RexType::collect_rex_family`, whose default
implementation is a no-op for non-ADT leaves.

## `#[rex::export]`

`#[rex::export]` registers a synchronous or asynchronous free Rust function as a Rex export. The
generated `<function>_rex_export()` helper preserves the function's Rust doc comments and its
Rex-visible Rust parameter names. The first owned `State` parameter is host context and is not
exposed to Rex.

Rust does not permit `#[doc]` attributes or doc comments on individual function parameters. Put
parameter descriptions in the function-level Rust documentation; the generated parameter metadata
contains names only.

```rust,ignore
/// Look up a sample by identifier.
///
/// `sample_id` is the stable identifier assigned by the host.
#[rex::export(name = "lookup")]
pub async fn lookup_sample(
    state: HostState,
    sample_id: String,
) -> Result<Sample, rex::engine::EngineError> {
    state.lookup(sample_id).await
}
```

## `#[rex::module]`

`#[rex::module(name = "...")]` generates `rex_module()`, returning a documented
`rex::engine::Module<State>`. It collects functions marked `#[rex::export]` and non-generic
derived ADTs marked `#[rex(export)]`. Rust doc comments on the inline module become module-level
Rex documentation.

Use `defaults(Type, ...)` to register qualified Rex `Default` instances for concrete Rust types.
Each listed type must implement `RexType`, `IntoRex`, and `RexDefault<State>`; the blanket
`RexDefault` implementation covers ordinary Rust `Default` types. The generated native default
producer remains private to the module. In Rex, a registered single-variant options type can be
constructed as `LookupOptions {}` or with selected overrides such as
`LookupOptions { include_archived = true }`; omitted fields come from the registered default.

```rust,ignore
/// Host sample-management APIs.
#[rex::module(name = "host.samples", defaults(LookupOptions))]
mod samples {
    use rex::engine::EngineError;

    #[derive(Default, rex::Rex)]
    pub struct LookupOptions {
        pub include_archived: bool,
    }

    /// Return whether a sample exists.
    #[rex::export]
    pub fn exists(
        state: HostState,
        sample_id: String,
        options: LookupOptions,
    ) -> Result<bool, EngineError> {
        let _ = options;
        Ok(state.contains(&sample_id))
    }
}

let module = samples::rex_module()?;
builder.inject_module(module)?;
```
