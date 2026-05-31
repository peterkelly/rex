# Prelude Has Multiple Sources of Truth

## Problem

The prelude is split across Rex source, type-system injection code, and runtime Rust registration code. The typeclass declarations and methods live in `prelude_typeclasses.rex`, while many primitive operations and runtime implementations are registered manually in `rex-engine/src/prelude.rs`.

There is no single manifest that defines the intended builtin surface and links each name to its type, class role, and runtime implementation.

## Evidence

- `rex-typesystem/src/prelude_typeclasses.rex` contains Rex implementations of typeclass methods.
- `rex-typesystem/src/prelude.rs` injects typeclass and primitive type information.
- `rex-engine/src/prelude.rs` injects runtime ADTs, operators, builtins, native functions, and prelude virtual module exports.
- `rex-engine/src/prelude.rs` is over 3,000 lines and repeatedly registers names by string.
- Some runtime registration depends on looking up schemes already present in the type-system environment, such as `unwrap`, `is_some`, and related builtins.

## Why This Smells

The prelude is a language-level contract. Splitting it across several mechanisms makes that contract hard to audit.

The major risk is drift:

- A type-system entry may exist without a runtime implementation.
- A runtime native may be registered under a name whose type moved or changed.
- Typeclass method bodies may assume primitive names whose runtime behavior is defined elsewhere.
- Documentation may describe a builtin that is assembled from several disconnected places.

The split is understandable: some prelude behavior is most naturally written in Rex, while low-level primitives need Rust implementations. The smell is not the existence of the split; it is the lack of a single declarative inventory tying the pieces together.

## Impact

This makes builtin changes review-heavy and error-prone. It also makes it difficult to answer basic questions like "what is in the prelude?", "which builtins are primitive?", and "which typeclass methods are implemented by Rex source versus native Rust?".

As the standard library grows, this file-level and source-level split will become a bigger source of accidental inconsistencies.

