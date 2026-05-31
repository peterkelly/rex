# `Engine` Is Carrying Too Many Responsibilities

## Problem

`rex_engine::Engine` acts as the central owner for host state, type-system state, native registries, typeclass registries, module caches, importers, runtime heap, execution policy, default imports, and compiler preparation state.

That makes it a god object across multiple phases: configuration, module registration, type registration, compilation, runtime preparation, and heap ownership.

## Evidence

`rex-engine/src/builder/engine.rs` defines `Engine` with fields for:

- host state,
- runtime environment,
- native function registry,
- typeclass registry,
- `TypeSystem`,
- typeclass method cache,
- module system,
- module export and interface caches,
- module sources and fingerprints,
- cycle-interface state,
- default imports,
- virtual modules,
- module-local type names,
- async policy,
- execution bounds,
- parallelism controller,
- heap.

`Compiler` owns an `Engine` and compilation mutates that engine by rewriting imports and injecting declarations before creating a `CompiledProgram`.

## Why This Smells

The architecture documentation describes a preparation boundary: `Engine` builds host state, `Compiler` prepares code, and `Evaluator` runs code. In practice, `Engine` remains the mutable bucket of almost everything needed by both compile-time and runtime.

This makes phase reasoning harder:

- It is not obvious which fields are compile-only, runtime-only, or both.
- Cache invalidation and module loading mutate the same object that owns runtime heap and host function registration.
- Compiling a program is not just a pure preparation step; it changes the embedded type/runtime environment.
- The object has too many invariants for one type to communicate clearly.

The design also creates pressure for unrelated code to gain access to `Engine` because "everything useful is in there".

## Impact

This makes future changes more fragile, especially around incremental compilation, reusable compiled artifacts, multi-program preparation, module cache invalidation, and lifecycle guarantees.

The current design can work, but it requires contributors to understand many hidden ordering constraints. That is a long-term maintenance cost.

