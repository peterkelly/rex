# Memory Management

`rex-engine` stores evaluated values in a shared moving heap. The copying collector may relocate
every live value during allocation, so a heap location is never a stable runtime identity. Safety
comes from using a reference representation whose lifetime and ownership match the boundary it
crosses.

Embedders allocate through `Heap` and retain heap values through rooted `Handle` values;
`Handle::value()` provides a public view when direct inspection is needed. During evaluation,
mutable frame and scheduler state alternates between scope-rooted references while the heap is
locked and opaque persistent roots while it is unlocked. Compiled environments and host work use
handles at their outer boundaries. Raw moving pointers are confined to collector-owned cells and
the heap implementation.

## Design goals

- Support graph-shaped runtime data, including cycles.
- Make allocation, locking, and dereference authority explicit.
- Keep values alive across concurrent host work without exposing moving locations.
- Detect stale, cross-heap, and cross-arena references deterministically.
- Keep equality, display, diagnostics, and Rust conversions correct for moving heap graphs.
- Reclaim unreachable values without requiring evaluator code to rewrite its long-lived state.

## Heap ownership

### `Heap` is the shared public capability

`Heap` is a cloneable, thread-safe owner of `Arc<Mutex<HeapState>>`. It may be retained across
threads and `await` points. Public allocation and inspection methods acquire that mutex, and every
allocation may collect before creating its result.

This makes a `Heap` clone a broad capability: apparently small operations can lock shared state and
move every live object. It belongs in the outer async evaluator coordinator, public host `Context`,
and embedding APIs. Code that already owns the heap lock must not call back through `Heap`.

### `HeapState` is the locked collector state

`HeapState` contains object slots, registered-root slots, the temporary-root stack, root free lists,
and collection policy. It is available only while the `Heap` mutex is held. Exclusive
`&mut HeapState` access is the underlying proof that one synchronous operation has sole access to
the collector.

Evaluator code receives that proof as `RootScope`, not as `HeapState` or `Heap`. A locked evaluator
cycle cannot invoke host code, block, await, or call an operation whose destructor may reacquire the
heap mutex.

## Reference categories

| Reference | Purpose | Collector action | Permitted boundary |
| --- | --- | --- | --- |
| `InternalPtr` | Edge inside a GC-managed cell | Rewrites the pointer in the copied cell | Heap internals only; never across allocation, unlock, thread transfer, or `await` as an unrooted local |
| `RootedPtr<'scope>` | Temporary value during one locked synchronous operation | Rewrites its temporary-root stack entry | Only inside the branded `RootScope`; deliberately neither `Send` nor `Sync`, and never across unlock or `await` |
| `PersistentPtr` | Evaluator-owned value between locked cycles | Rewrites the registered heap root owned by its arena slot | May survive unlock, evaluator suspension, and task migration; resolvable only by its owning arena under a new `RootScope` |
| `Handle` | Public, host, and outer-runtime rooted value | Rewrites its registered root slot | `Send + Sync`; may cross allocations, threads, callbacks, and `await` points |

### `InternalPtr` is a raw moving edge

An `InternalPtr` contains a heap identifier, slot index, and heap-wide collection epoch. The heap
checks all three when dereferencing it. Every collection advances the epoch for all copied cells,
so an old raw pointer fails even when its value happens to retain the same numeric slot index.

`InternalPtr` is private to the heap and its private list-traversal module. GC-managed `Cell` values
may contain it because the collector owns those edges and rewrites them during copying. A local raw
pointer is valid only while the heap remains locked and no allocation can occur. It is not a frame,
scheduler, environment, or host representation.

### `RootedPtr<'scope>` is a synchronous temporary root

A `RootedPtr` is an index into the temporary-root stack, not a heap location. Its invariant lifetime
brand ties it to the higher-ranked `RootScope` closure that created it. Collection updates the stack
entry, so synchronous evaluator code can continue using the token after allocations in that same
scope.

`RootScope` owns exclusive `&mut HeapState` access and restores the temporary-root stack to its entry
depth on drop. Synchronous frames, environments, scheduler work, native tasks, and intermediate
control results use `RootedPtr` while one evaluator cycle is executing. The scope brand makes both
`RootScope` and `RootedPtr` neither `Send` nor `Sync`.

### `PersistentPtr` is evaluator-owned unlocked state

A `PersistentPtr` is a generational token for a slot in one `PersistentRootStore`. It contains no
heap location and no `Heap` capability. Every live arena slot owns an ordinary registered heap root,
which the collector updates when its value moves.

Frames, environments, scheduler work items, and native-task state are converted to `PersistentPtr`
before the evaluator releases the mutex. On the next cycle, the owning store resolves them into a
new scope's `RootedPtr` values. Store identity, slot generation, and heap identity checks reject
stale tokens and tokens from another heap or evaluator arena.

The arena uses explicit insertion, replacement, removal, and teardown. It intentionally performs no
destructor-based heap cleanup: dropping evaluator state while the heap is locked must not attempt to
lock the same mutex again.

### `Handle` is the boundary-safe registered root

A `Handle` owns a generational registered-root identifier. It never exposes the current heap
location. Clones share that root ownership, and the last owner unregisters the root. The registered
slot is rewritten by collection, so the handle remains valid while host code allocates, suspends, or
moves work between threads.

Handles are the public representation, but they are not restricted to embedder code. Immutable
compiled/runtime environments and queued or completed host work also use handles when their values
must remain valid outside a locked evaluator cycle. Mutable evaluator frames use `PersistentPtr`
instead so their root lifecycle can be managed explicitly as one arena.

Public handle inspection and conversion methods lock the heap. Dropping the last clone also locks in
order to unregister its root. For that reason, an active locked evaluator cycle uses `RootedPtr`
instead of calling handle methods or owning values whose final `Handle` destructor could run there.

## Allowed conversions

Reference conversion is deliberately concentrated at boundary code:

- `Handle` to `RootedPtr`: `RootScope::root_handle` resolves the registered root while the heap is
  already locked, then pushes the current pointer onto the temporary-root stack.
- `PersistentPtr` to `RootedPtr`: `PersistentRootStore::resolve` validates the token and pushes the
  resolved registered root into the active scope.
- `RootedPtr` to `PersistentPtr`: `PersistentRootStore::insert` registers the current value before
  the synchronous cycle releases the mutex.
- `RootedPtr` to `Handle`: the sealed `HandlePromoter` registers the root without relocking. It is
  available only from the explicitly named promotable-scope entry point used for final results and
  host-call arguments.
- `RootedPtr` to `InternalPtr`: private heap allocation and overwrite operations read current
  pointers only while the scope protects every input. The resulting raw pointers become traced
  `Cell` edges before the operation returns.
- `InternalPtr` to `RootedPtr`: private heap inspection code roots a discovered child before any
  subsequent allocation can occur.

There is no conversion that places `InternalPtr` in unlocked evaluator state or exposes it to an
embedder. Scope branding also prevents a `RootedPtr` from being returned to the outer async loop.

## Copying collection

The single object-allocation path decides whether collection is needed. If a new cell already
contains internal child edges, it registers those children before collecting and rewrites them to
their new locations before installing the cell.

Collection starts from two root sets:

- registered roots owned by `Handle` values and evaluator `PersistentRootStore` slots;
- the temporary-root stack owned by active `RootScope` values.

It traces `InternalPtr` edges in reachable cells, copies every reachable cell, builds forwarding
locations, and rewrites registered roots, temporary roots, and cell edges. Old raw pointers then
fail their collection-epoch checks by design.

Evaluator frames and scheduler state do not participate in the collector's tracing interface. They
contain either `RootedPtr` indices during a locked cycle or `PersistentPtr` tokens between cycles.
The collector is solely responsible for rewriting raw cell edges and root slots.

## Evaluator and host boundary

One non-async evaluator cycle executes under a single `RootScope`. It resolves persistent state,
runs one work item and its synchronous helpers, applies the control result, persists surviving
state, and promotes any final result or host-call arguments before returning.

The outer async coordinator releases the heap lock before it activates or polls host work. Queued
and completed host calls contain public `Handle` values, types, and Rust-owned metadata only. A
completed host result remains a handle until the next locked cycle roots it.

Immediate and deferred callbacks use the same handle-only ABI. "Immediate" changes scheduling
policy; it does not create a raw-pointer callback path.

Internal scheduler-native helpers are not part of that host ABI. They run synchronously inside the
locked cycle and receive only `RootScope`, type information, and `RootedPtr` arguments; they cannot
call through public `Heap`/`Handle` operations. If a helper returns an evaluator-native task, its
rooted values are converted to persistent roots before the cycle unlocks.

## Public reads and construction

`Handle::value()` returns a public `Value`. Composite `Value` variants contain child `Handle`
values. The heap registers all child roots while locked, releases the guard, and only then constructs
their public handle owners, avoiding both moving-pointer exposure and destructor re-entry.

Public values are created through `Heap::alloc_*` methods. Composite inputs are resolved and rooted
under one guard before allocation, and the result is registered before that guard is released.
`IntoRex` and `FromRex` follow the same handle-centric boundary.

Heap-aware structural operations include:

- `Handle::debug()`;
- `Handle::display()` and `Handle::display_with(...)`;
- `Handle::value_eq(...)`.

They resolve through registered roots and use cycle-safe graph traversal, so they remain correct
after previous collections.

## Runtime checks

The heap enforces or verifies:

- heap identity on raw pointers, handles, root identifiers, stores, and persistent tokens;
- the heap-wide collection epoch for raw pointers;
- slot generations for stale registered-root and persistent-arena identifiers;
- explicit persistent-root ownership and teardown;
- temporary-root stack integrity when a `RootScope` exits;
- complete forwarding of registered roots and temporary roots during collection;
- valid rewritten child pointers in every copied cell;
- absence of empty, wrong-epoch, or unreachable slots after debug-build collection.

No `unsafe` code is used for this memory model.

## Lifecycle and limitations

`Builder` creates the heap, `Compiler` carries it through program preparation, and `Evaluator` uses
it for execution. Evaluation returns a `Handle`, not an unrooted payload. Handle-based native
callbacks receive the same heap through `Context` and can safely allocate additional handles;
typed exports use that heap internally when converting Rust arguments and results.

There is no public operation that exposes a heap location or manually runs collection. The public
collection-on-every-allocation setting exists for stress validation; production execution uses the
heap-growth policy. Embedders must treat object location and collection epochs as entirely opaque.
