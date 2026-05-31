# TypeSystem State Is Too Public and Transactional

## Problem

`TypeSystem` exposes much of its internal state as public mutable fields, and higher-level crates manually mutate or roll back pieces of that state during module registration and invalidation.

This makes the type environment feel less like a controlled semantic component and more like a shared mutable database.

## Evidence

`rex-typesystem/src/typesystem.rs` exposes public fields including:

- `env`
- `classes`
- `adts`
- `class_info`
- `class_methods`
- `declared_values`
- `supply`
- `limits`

`rex-engine` reaches into this state directly in several places. For example, stale module invalidation manually removes type-level symbols and declared values from `engine.type_system`.

The type system also has separate paths for registering, preparing, and injecting declarations, with callers responsible for sequencing them correctly.

## Why This Smells

The type system owns important invariants:

- names in the value environment must align with declarations,
- ADT constructors must align with ADT metadata,
- class method schemes must align with class info,
- instance heads and method bodies must line up,
- declared placeholders must be removed when real definitions arrive.

When these structures are public and manually mutated by other crates, those invariants become convention rather than API-enforced behavior.

The cache invalidation path is a good example of the smell. Removing a module's old symbols requires knowing which internal maps to touch and how they relate. That is transactional behavior, but it is implemented as ad hoc field mutation.

## Impact

This raises the risk of stale type state, incomplete rollback, and inconsistent environments after errors or module reloads. It also makes it hard to introduce stronger guarantees around incremental compilation, module unloading, or isolated typechecking sessions.

Even if the current code is careful, the API shape invites future mistakes.

