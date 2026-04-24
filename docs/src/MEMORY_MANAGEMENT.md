# Memory Management

`rex-engine` uses a pointer-based runtime: evaluated values live in a central heap, and the rest of the runtime passes lightweight pointers to those heap entries.

This gives the engine a clear separation between identity (`Pointer`) and storage (`Heap`). It also makes behavior at host/native boundaries explicit: values are allocated once, referenced many times, and inspected through heap APIs.

## Design goals and rationale

- Support graph-shaped runtime data, including cycles.
- Keep allocation and dereference rules explicit and centralized.
- Make host integration predictable by using stable pointers rather than implicit deep copies.
- Preserve strong runtime safety checks for pointer validity and heap ownership.
- Keep diagnostics (type names, debug/display output, equality) correct for heap graphs.

## Core runtime model

### `Pointer` is an opaque stable pointer

A `Pointer` identifies a slot in a heap using:

- `heap_id`
- `index`
- `generation`

Conceptually:

- `index` selects a slot.
- `generation` distinguishes different occupants of the same slot over time.
- `heap_id` prevents accidental cross-heap usage.

`Pointer` is intentionally opaque to callers. The engine validates it on access, so stale pointers and cross-heap usage fail deterministically at runtime.

### `Heap` stores all runtime values

`Heap` owns an internal `HeapState`:

- `slots: Vec<HeapSlot>`
- `free_list: Vec<u32>`

Each `HeapSlot` stores:

- `generation: u32`
- `value: Option<Arc<Value>>`

All runtime reads/writes go through heap methods (`alloc_*`, `get`, `type_name`, etc.). Pointer internals are not exposed in API shape.

### Engine-owned heap lifecycle

`Engine` constructs and owns its own `Heap` (`Engine::new`, `Engine::with_prelude`).

- Evaluation returns `Pointer`, not `Value`.
- Callers can inspect via `engine.heap()` or extract ownership with `engine.into_heap()`.

This keeps allocation authority clear: the engine is responsible for heap creation, and the heap is the single runtime store for values.

## Read/write semantics

### Reads return `ValueRef`

`Heap::get` returns `ValueRef` (an `Arc<Value>` wrapper), not a copied `Value`.

Why:

- Avoid accidental deep clones in hot paths.
- Make cloning explicit and local where it is actually needed.

### Writes are controlled

Values are created through `Heap::alloc_*` methods.

There is also an internal `overwrite` operation used for recursive initialization patterns (placeholder first, then finalized value).

## Equality, debug, and display are heap-aware

Structural operations are provided as heap-aware helpers:

- `value_debug(heap, value)`
- `value_display(heap, value)`
- `value_eq(heap, lhs, rhs)`
- `pointer_eq(heap, lhs, rhs)`
- `closure_debug` / `closure_eq`

These functions dereference through the heap and are cycle-safe (visited-set based), so recursive graphs can be inspected and compared without infinite recursion.

## Pointer-first host/native boundary

Runtime conversion traits are pointer-centric:

- `IntoPointer`
- `FromPointer`

Native injection and prelude paths pass pointers by default, including module runtime exports
(`export_native` / `export_native_async`). These callbacks receive `EvaluatorRef<'_, State>`, so they can allocate through
`engine.heap` and inspect host state via `engine.state`. `Value` is used where direct payload
inspection is required.

This keeps ownership/allocation behavior centralized in the heap and limits implicit copying.

## Safety and invariants

At runtime, the heap enforces:

- Wrong-heap pointer rejection (`heap_id` mismatch).
- Invalid/stale pointer rejection (`index`/`generation` mismatch).
- Type-aware errors via heap-driven `type_name`.

No `unsafe` code is used for this memory model.

## Scope and limitations

- This is a pointer-based heap model, not a full garbage-collected runtime.
- There is no public reclamation/GC API yet.
- The pointer format includes `generation` and the heap tracks a `free_list` in state, but active slot-reuse/reclamation policy is intentionally not exposed as public behavior yet.

In short, memory management is centered on explicit heap ownership, validated pointers, and cycle-safe graph traversal, with reclamation strategy treated as a separate concern.
