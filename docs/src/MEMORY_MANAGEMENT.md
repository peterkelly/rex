# Memory Management

`rex-engine` uses a heap-based runtime: evaluated values live in a central heap, and the internal evaluator passes lightweight pointers to those heap entries.

This gives the engine a clear separation between identity and storage without exposing raw heap pointers to embedders. Public API code works with rooted `Handle` values, while `Pointer` remains an internal representation detail.

## Design goals and rationale

- Support graph-shaped runtime data, including cycles.
- Keep allocation and dereference rules explicit and centralized.
- Make host integration predictable by using stable handles rather than implicit deep copies.
- Preserve strong runtime safety checks for pointer validity and heap ownership.
- Keep diagnostics (type names, debug/display output, equality) correct for heap graphs.

## Core runtime model

### `Pointer` is an internal stable pointer

A `Pointer` identifies a slot in a heap using:

- `heap_id`
- `index`
- `generation`

Conceptually:

- `index` selects a slot.
- `generation` distinguishes different occupants of the same slot over time.
- `heap_id` prevents accidental cross-heap usage.

`Pointer` is intentionally crate-private. The engine validates it on access, so stale pointers and cross-heap usage fail deterministically inside the runtime.

### `Handle` is the public rooted value reference

A `Handle` owns a temporary external heap root for one value. Cloning the handle
clones that root handle, and dropping the final clone unregisters it.

Host code can:

- inspect a value with `Handle::value()`, which returns a `Value`.
- convert to Rust with `Handle::to_rust()` or `FromRex::from_rex(...)`.
- display/debug/compare values through handle methods.

Host code cannot extract or store a raw runtime pointer.

### `Heap` stores all runtime values

`Heap` owns an internal `HeapState`:

- `slots: Vec<HeapSlot>`
- `free_list: Vec<u32>`

Each `HeapSlot` stores:

- `generation: u32`
- `cell: Option<Arc<Cell>>`

Internal runtime reads/writes go through heap methods. Public construction uses
`Heap::make_*` / `Heap::alloc_*`, which return `Handle` values rather than raw
pointers.

### Runtime heap lifecycle

`Engine` constructs the initial `Heap` during preparation (`Engine::new`, `Engine::with_prelude`).
That heap then moves with the prepared runtime state into `Compiler` and `Evaluator`.

- Evaluation returns `Handle`, not `Value`.
- Callers can inspect via the returned handle or allocate more values from native callbacks through
  `EvaluatorRef::heap()`.

This keeps allocation authority clear: the preparation phase creates the heap, and the evaluator's
runtime core is the single store used during execution.

## Read/write semantics

### Public reads return `Value`

`Handle::value()` returns `Value`, a safe public value of the runtime value. Composite values contain child `Handle` values rather than raw pointers.

Why:

- Avoid accidental deep clones in hot paths.
- Keep internal runtime values and heap pointers out of the public API.
- Root child values discovered during inspection.

### Writes are controlled

Public values are created through `Heap::make_*` / `Heap::alloc_*` methods, which return `Handle`.

There is also an internal `overwrite` operation used for recursive initialization patterns (placeholder first, then finalized value).

## Equality, debug, and display are heap-aware

Structural operations are provided as heap-aware helpers:

- `Handle::debug()`
- `Handle::display()` / `Handle::display_with(...)`
- `Handle::value_eq(...)`

These functions dereference through the heap and are cycle-safe (visited-set based), so recursive graphs can be inspected and compared without infinite recursion.

## Handle-first host/native boundary

Runtime conversion traits are handle-centric:

- `IntoRex`
- `FromRex`

Public native injection paths pass handles, including module runtime exports
(`export_native` / `export_native_async`). These callbacks receive
`EvaluatorRef<State>`, so they can allocate public handles through
`engine.heap()` and inspect host state via `engine.state()`. `Value` is used
where direct payload inspection is required.

This keeps ownership/allocation behavior centralized in the heap while making it
impossible for host code to store unrooted raw pointers.

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
