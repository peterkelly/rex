# Prelude Has Multiple Sources of Truth

## Problem

The standard prelude is now owned by `rex-engine`, but its contract is still
assembled from several engine-local sources:

- Rex source for classes, instances, and method bodies.
- Rust code that builds the prelude-enabled `TypeSystem`.
- Rust code that registers native runtime implementations.
- Documentation generation code and hand-written descriptions.

There is no single manifest that defines the intended builtin surface and links
each name to its type, class role, source-level implementation, documentation,
and runtime implementation.

## Evidence

- `rex-engine/src/prelude/typeclasses.rex` contains Rex implementations of
  standard typeclass methods.
- `rex-engine/src/prelude/type_system.rs` builds the standard type environment,
  including ADTs, helper functions, and `prim_*` schemes.
- `rex-engine/src/prelude/mod.rs` parses the Rex source, injects runtime ADTs,
  registers native functions, and builds the `std.prelude` module.
- Primitive names and scheme shapes are still manually mirrored between
  type-system construction and runtime registration.
- Some runtime registration depends on looking up schemes already present in the type-system environment, such as `unwrap`, `is_some`, and related builtins.

## Why This Smells

The prelude is a language-level contract. Keeping it in one crate is a major
improvement, but the contract is still split across several mechanisms and is
therefore harder to audit than it should be.

The major risk is drift:

- A type-system entry may exist without a runtime implementation.
- A runtime native may be registered under a name whose type moved or changed.
- Typeclass method bodies may assume primitive names whose runtime behavior is defined elsewhere.
- Documentation may describe a builtin that is assembled from several disconnected places.

The split is understandable: some prelude behavior is most naturally written in Rex, while low-level primitives need Rust implementations. The smell is not the existence of the split; it is the lack of a single declarative inventory tying the pieces together.

## Current Safeguards

`rex-engine` now has consistency tests that derive primitive names from the
prelude source, the standard type system, and the runtime native registry. These
tests catch missing `prim_*` schemes, missing runtime implementations, and
incompatible scheme registrations without hard-coding the actual primitive
inventory.

Those tests reduce the drift risk, but they are still a guardrail rather than a
source of truth.

## Impact

This makes builtin changes review-heavy and error-prone. It also makes it difficult to answer basic questions like "what is in the prelude?", "which builtins are primitive?", and "which typeclass methods are implemented by Rex source versus native Rust?".

As the standard library grows, this file-level and source-level split will become a bigger source of accidental inconsistencies.

## Possible Next Step

Centralize primitive names, primitive type-family inventories, and primitive
scheme constructors in an engine-local module such as
`rex-engine/src/prelude/primitive_specs.rs`.

Runtime implementation blocks can remain in Rust code, but both
`type_system.rs` and `mod.rs` should call the same scheme builders and shared
name constants. That would make common changes, such as adding a supported type
to an existing primitive family, require one edit to the inventory rather than
parallel edits in type and runtime registration code.
