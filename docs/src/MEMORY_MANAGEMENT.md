# Memory Management

`rex-engine` uses a heap-based runtime: evaluated values live in a central heap, and the internal
evaluator passes lightweight pointers to those heap entries. The heap now includes a copying
collector, so internal pointers can move during allocation.

This gives the runtime a clear separation between identity and storage without exposing raw heap
pointers to embedders. Public API code works with rooted `Handle` values, while `Pointer` remains
an internal representation detail.

## Design goals and rationale

- Support graph-shaped runtime data, including cycles.
- Keep allocation and dereference rules explicit and centralized.
- Make host integration predictable by using stable handles rather than implicit deep copies.
- Preserve strong runtime safety checks for pointer validity and heap ownership.
- Keep diagnostics (type names, debug/display output, equality) correct for heap graphs.
- Allow unreachable runtime objects to be reclaimed without exposing raw moving pointers to embedders.

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

`Pointer` is intentionally crate-private. The runtime validates it on access, so stale pointers and
cross-heap usage fail deterministically inside the runtime. Because collection is copying, any raw
pointer from before a collection is stale unless it has been rewritten through a traced runtime
structure.

### `Handle` is the public rooted value reference

A `Handle` owns a temporary external heap root for one value. Cloning the handle
clones that root handle, and dropping the final clone unregisters it.

Host code can:

- inspect a value with `Handle::value()`, which returns a `Value`.
- convert to Rust with `Handle::to_rust()` or `FromRex::from_rex(...)`.
- display/debug/compare values through handle methods.

Host code cannot extract or store a raw runtime pointer.

### `Heap` stores all runtime values

`Heap` owns an internal `HeapState` with object slots, external root slots, root-slot reuse state,
and GC scheduling metadata.

- object slots store runtime cells
- root slots store public `Handle` roots and temporary roots
- GC metadata tracks the next collection threshold and collection count

Each `HeapSlot` stores:

- `generation: u64`
- `cell: Option<Cell>`

Internal runtime reads/writes go through heap methods. Public construction uses
`Heap::alloc_*`, which returns `Handle` values rather than raw pointers.

### Runtime heap lifecycle

`Builder` constructs the initial `Heap` during preparation (`Builder::new`, `Builder::with_prelude`).
`Builder::build_compiler()` moves that heap handle into the `Compiler`, and
`Compiler::compile_program()` then moves it into the evaluator runtime core returned with the
compiled program.

- Evaluation returns `Handle`, not `Value`.
- Callers can inspect via the returned handle or allocate more values from native callbacks through
  `Context::heap()`.

This keeps allocation authority clear: the preparation phase creates the heap, and execution uses
the evaluator runtime core's shared heap store.

### Copying collection and roots

Every public `Heap::alloc_*` call may run collection before allocating the requested object.
Collection starts from registered roots, copies reachable objects into a new slot vector, rewrites
roots and traced child pointers, and increments each moved object's generation. Old raw pointers
are intentionally invalid after collection.

Roots come from:

- public `Handle` values returned to host code
- temporary roots used while constructing composite heap cells
- evaluator frames and scheduler tasks that trace and rewrite their internal pointers
- runtime environments, closures, native functions, and overloaded-function records stored in live heap cells

This is why public APIs return handles. A `Handle` owns an external heap root, remains valid across
later allocations and `await` points, and resolves to the current post-GC pointer when the runtime
needs to dereference it.

## Read/write semantics

### Public reads return `Value`

`Handle::value()` returns `Value`, a safe public value of the runtime value. Composite values
contain child `Handle` values rather than raw pointers.

Why:

- Avoid accidental deep clones in hot paths.
- Keep internal runtime values and heap pointers out of the public API.
- Root child values discovered during inspection.

### Writes are controlled

Public values are created through `Heap::alloc_*` methods, which return `Handle`.

There is also an internal `overwrite` operation used for recursive initialization patterns
(placeholder first, then finalized value). Runtime code that holds crate-private pointers across
allocation must protect them with handles, temporary roots, or traced frame/task state.

## Equality, debug, and display are heap-aware

Structural operations are provided as heap-aware helpers:

- `Handle::debug()`
- `Handle::display()` / `Handle::display_with(...)`
- `Handle::value_eq(...)`

These functions dereference through the heap and are cycle-safe (visited-set based), so recursive
graphs can be inspected and compared without infinite recursion. They operate through handles, so
they stay valid even if allocation has moved the underlying objects since the handle was created.

## Handle-first host/native boundary

Runtime conversion traits are handle-centric:

- `IntoRex`
- `FromRex`

Public native injection paths pass handles, including module runtime exports
(`export_native` / `export_native_async`). These callbacks receive
`Context<State>`, so they can allocate public handles through
`ctx.heap()` and inspect host state via `ctx.state()`. `Value` is used
where direct payload inspection is required.

This keeps ownership/allocation behavior centralized in the heap while making it
impossible for host code to store unrooted raw pointers.

## Safety and invariants

At runtime, the heap enforces:

- Wrong-heap pointer rejection (`heap_id` mismatch).
- Invalid/stale pointer rejection (`index`/`generation` mismatch).
- Type-aware errors via heap-driven `type_name`.
- Root validation for public handles and temporary roots.
- Debug-build verification that copied objects are rooted and all traced child pointers are valid
  after collection.

No `unsafe` code is used for this memory model.

## Scope and limitations

- This is a moving, copying heap with automatic collection; embedders should treat object location
  as completely opaque.
- There is no public manual GC or heap-tuning API. Test-only hooks can force collection, but
  production callers only observe stable `Handle` behavior.
- `Pointer`, `Cell`, temporary roots, and trace/rewrite plumbing remain crate-private runtime machinery.

In short, memory management is centered on explicit heap ownership, rooted public handles,
validated moving pointers, and cycle-safe graph traversal.
