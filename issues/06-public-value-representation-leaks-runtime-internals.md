# Public Value Representation Leaks Runtime Internals

## Problem

`rex-engine/src/value.rs` combines heap implementation, GC roots, public handles, conversion traits, internal runtime cells, and public value views. The public `Value` enum includes variants that represent runtime implementation concepts rather than ordinary Rex values.

## Evidence

`rex-engine/src/value.rs` defines:

- heap state and root slots,
- copying GC support,
- `Heap`,
- `Handle`,
- public `Value`,
- conversion traits such as `IntoRex` and `FromRex`,
- tuple/container conversion implementations,
- tests for rooting and GC behavior.

The public `Value` enum includes variants such as:

- `Uninitialized`
- `Frame`
- `Closure`
- `Native`
- `Overloaded`

These are runtime/internal concepts, not values an embedder should normally treat as Rex data.

## Why This Smells

Public APIs should expose stable semantic concepts. Runtime frames, closures, native callables, and overload sets are implementation details of the evaluator.

When they appear in the public value enum:

- Embedders can observe internal implementation states.
- Internal representation changes become public API changes.
- Display/conversion/type-name code must account for non-data values.
- The boundary between "Rex value" and "runtime cell" becomes unclear.

The file-level structure reinforces the problem. Because heap internals and public value views live together, it is harder to see what is intended as stable embedder API versus evaluator machinery.

## Impact

This increases API coupling between embedders and the evaluator internals. It also makes future runtime refactors more expensive, especially if the evaluator changes how closures, frames, overloaded functions, or native values are represented.

The smell is architectural: it does not mean the current GC/rooting design is wrong. It means the public value view and internal heap cell model need a clearer boundary.

