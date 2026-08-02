# GC and evaluator safety completion plan

## Objective

Make the copying collector safe in the presence of concurrent host work, and
make the relevant invariants visible in Rust's types.

The central problem is not merely that evaluator code can forget to root a
temporary pointer before it allocates. `Heap` is a cloneable shared-state
capability. Its methods acquire a mutex internally, so passing `&Heap` through
the evaluator hides both the critical section and the fact that another Tokio
worker may acquire another `Heap` clone, allocate through a host callback, and
collect between two evaluator operations.

Rooting and refreshing every pointer around evaluator-initiated collection is
therefore insufficient. Once the evaluator releases the heap lock, an async
host function can collect and invalidate any raw pointer copy retained by the
evaluator. A collection-epoch check has the same time-of-check/time-of-use race
if the lock is released after the check.

The target design has four distinct reference categories:

- `InternalPtr` is used only for edges inside GC-managed cells. It is private
  to the heap implementation.
- `RootedPtr<'scope>` is a temporary reference used during one synchronous
  evaluation cycle. Its branded lifetime is tied to a `RootScope` that owns
  exclusive `&mut HeapState` access.
- `PersistentPtr` is an opaque internal root used by evaluator state that must
  survive after the heap lock is released. It does not expose `Heap` or any
  operation that can acquire the mutex.
- `Handle` remains the public, thread-safe reference used by embedders and host
  functions across threads and `await` points.

No raw heap location may cross a mutex-unlock or `await` boundary.

## Current position

The existing transition has established much of the synchronous API:

- `RootScope` and lifetime-branded `RootedPtr` exist.
- The temporary root stack is traced and rewritten by collection.
- Most allocation-capable evaluator and scheduler-native functions accept a
  `RootScope`.
- `RuntimeCore` no longer owns `Heap`, and `InternalCtx` does not expose it.
- Public host callbacks use a `Context` containing `Heap` and exchange
  `Handle` values through the unified immediate/deferred host path.
- Control frames live outside the GC heap.

The evaluator now owns a generational `PersistentRootStore`. Frames, evaluator
environments, scheduler work items, and native tasks use `PersistentPtr` while
the heap is unlocked. Host completions remain `Handle` values until the next
locked evaluator cycle, and host-call arguments are promoted to handles before
that cycle releases the mutex. The obsolete typeclass value cache has been
removed; the cactus evaluator had not populated it since its introduction.

One synchronous evaluator cycle now resolves persistent state under a single
`RootScope`, executes a work item, and rebuilds the persistent arena before
unlocking. The cycle receives no `Heap` capability. Its only access to
`HeapState` is the active `RootScope`; a separate sealed `HandlePromoter` can
only promote rooted values without locking, inspecting heap values, allocating
Rex values, or collecting. The promoter is supplied only by the explicitly
named top-level promotable-scope entry point in the handle-promotion module;
ordinary root scopes never receive it. Host-call arguments and final results
are therefore made boundary-safe without giving `RootScope` or `Heap` a
promotion-specific field or method. The cycle's explicit outcome
distinguishes ready internal work, queued host work, already-started host work,
and completion. The outer async coordinator consumes those outcomes and is the
only layer that starts or polls host futures.

Synchronous frames, work items, environments, native tasks, native-call
requests, and control results now use `RootedPtr<'scope>`. Heap inspection of
closures, partial native functions, overloaded functions, tuples,
dictionaries, ADTs, and lists produces rooted views before evaluator code can
allocate. Pattern bindings, record-update fields, application arguments, and
prelude scheduler intermediates remain rooted for their complete synchronous
lifetime. The old `TempRoots` compatibility API, collection-epoch refreshes,
post-allocation frame rewrites, transient evaluator `Collection`
implementations, and bulk `PersistentRoots` scheduler snapshot have been
removed.

The remaining transition starts at Step 7: raw internal cell edges still use
the crate-visible `Pointer` name and have not yet been confined to the heap
implementation. Handle cleanup and final invariant documentation remain in
Step 8.

## Step 1: Add deterministic concurrency regressions

Add tests that expose the actual cross-thread failure rather than relying only
on collection-on-allocation stress.

Use barriers or channels to coordinate an async host function with the
evaluator. The host function should retain handle arguments, allocate enough
to force collection, and signal exactly when the evaluator is between
operations that previously refreshed and then used raw pointers. Add a
test-only scheduling hook if a deterministic interleaving cannot be expressed
through the public callback API.

Cover at least these cases:

1. A host function collects while other evaluator frames contain live values.
2. A host function collects while a ready work item contains a returned value.
3. Multiple host functions collect on different Tokio workers while runnable
   evaluator work remains queued.
4. A host function returns a composite value after repeated collections.

The tests should prove two separate properties:

- Host allocation cannot run while a synchronous evaluator cycle owns
  `HeapState`.
- Every evaluator value remains rooted and resolvable while the lock is
  intentionally released for host work.

Keep these tests after the migration. They are the regression coverage for the
thread-safety invariant.

## Step 2: Add an evaluator-owned persistent root representation

Introduce an opaque `PersistentPtr` and an evaluator-owned persistent root
store. This is the representation for values kept in frames, scheduler state,
native tasks, and environments between synchronous cycles.

`PersistentPtr` should identify a registered heap root without containing or
exposing a `Heap` clone. Resolving it must require an active `RootScope` or an
already-borrowed `HeapState`. Registering, cloning, and unregistering persistent
roots must occur explicitly while the caller holds the heap lock; dropping a
value during a synchronous cycle must never try to acquire the same mutex.

Prefer one evaluator-owned arena with generational slot identifiers over one
independently locking object per value. The arena should make ownership and
cleanup explicit:

- Adding persistent state registers a root before the lock is released.
- Removing state unregisters its roots while the same lock is held.
- Replacing state registers its replacement before releasing its previous
  roots.
- Ending evaluation unregisters the complete arena in one controlled step.
- Stale or cross-heap root identifiers produce an internal error.

Add focused tests for slot reuse, generation checks, cloning or sharing policy,
collection rewriting, explicit teardown, and foreign-heap rejection.

## Step 3: Move long-lived evaluator state to persistent roots

Replace raw `Pointer` fields that survive beyond one locked cycle with
`PersistentPtr`.

Migrate the state in small, compiling groups:

1. `EvalWorkItem` return values and scheduler ready queues.
2. Every value-bearing field in `Frame` and `NativeTask`.
3. Environments stored in frames and evaluator-owned native state.
4. Pending host-call metadata that currently contains raw arguments before it
   is promoted to handles.

Do not move control frames back into the GC heap. `FrameStore` should remain a
Rust-owned `BTreeMap<FrameId, Frame>`; only the representation of Rex values in
those frames changes.

The existing `Environment` type is currently used both inside heap cells and
outside the heap in evaluator frames. Split those roles. Heap closures need an
internal environment containing `InternalPtr`; evaluator state needs a
persistent environment whose values are persistent roots. Converting from a
closure to an active evaluator environment must happen through `RootScope`.

After each group, remove its `Collection` implementation and manual pointer
rewrite code. The collector should update registered root slots; evaluator
state should no longer be rewritten by building a `HashMap<Pointer, Pointer>`.

Acceptance criteria for this step:

- An evaluator may be paused indefinitely while other threads allocate and
  collect.
- No frame, ready item, native task, or environment contains a
  raw copying-collector location while the mutex is unlocked.
- The evaluator no longer needs a bulk trace/refresh pass merely to survive an
  async boundary.

## Step 4: Create one locked synchronous evaluation cycle

Extract a non-async function for one evaluator cycle. The outer async loop
should acquire the heap mutex once, create one `RootScope`, run the complete
synchronous cycle, convert its outcome into boundary-safe state, and then
release the lock.

One cycle should perform all of the following under the same guard:

1. Resolve the selected work item and frame from `PersistentPtr` to
   `RootedPtr`.
2. Run `eval_enter` or `eval_receive` and every synchronous helper it calls.
3. Apply the resulting control action to `FrameStore` and the scheduler.
4. Persist every value that must survive into another cycle.
5. Promote final results or host-call arguments to `Handle` before unlocking.

The function must not accept `&Heap`, call `Heap::with_locked`, invoke a host
callback, or contain an `await`. Its only capability for reading, allocating,
or mutating `HeapState` is `&mut RootScope<'_, 'scope>`. A separate sealed
promotion capability may only convert scope-rooted values to handles without
locking.

Make the cycle outcome explicit. It should distinguish at least:

- More internal work is ready.
- Host work must be started or polled.
- Evaluation is waiting for already-started host work.
- Evaluation completed with a `Handle`.

Any scope-branded value in an intermediate control result must be consumed or
promoted before the `RootScope` closure returns. The type system should reject
an attempt to place `RootedPtr<'scope>` directly into the outer async loop.

This step removes the current lock/refresh/use race. In particular, there must
be no sequence that refreshes frame pointers under one lock acquisition,
releases the lock, and then evaluates the frame under another acquisition.

## Step 5: Make the host boundary handle-only

Keep host future activation and polling completely outside the synchronous
heap critical section.

Construct each `NativeCall` while the evaluation cycle still owns
`RootScope`, promoting every argument to a `Handle` without reacquiring the
mutex. Queue only handles, types, call-site data, and frame identifiers for the
outer async scheduler.

Both immediate and deferred host callables should continue through the same
handle-based ABI. "Immediate" controls scheduling policy; it must not re-open a
raw-pointer callback path. It may run on the evaluator task after the heap lock
has been released. Deferred calls retain their executor, admission, and permit
semantics.

While host futures are running, the outer scheduler may inspect host-future
state and frame identifiers but must not resolve or manipulate evaluator Rex
values. A completed future remains a `Handle` until the next locked evaluator
cycle roots or persists it.

Remove the current repeated scheduler-root refreshes around future polling.
There should be no `Pointer` snapshot whose freshness depends on checking the
collection epoch between polls.

## Step 6: Complete the synchronous `RootedPtr` migration

Remove remaining raw-pointer parameters and results from allocation-capable
synchronous code.

In particular:

- Make `EvalControl` scope-aware so returned values are `RootedPtr<'scope>`
  until they are persisted or promoted.
- Change native and overloaded application to accept rooted arguments rather
  than accepting `Pointer` and immediately calling `scope.root`.
- Return rooted views of closures, partial native functions, overloaded
  functions, tuples, dictionaries, ADTs, and lists from heap inspection APIs.
- Keep pattern bindings, record-update values, list elements, and prelude
  intermediate values rooted for their complete synchronous lifetime.
- Convert host-call arguments to handles before a control result can escape
  the cycle.

As each path becomes rooted, remove its local `with_temp_roots`, collection
epoch check, and post-allocation rewrite logic. Finish by deleting `TempRoots`,
`Heap::with_temp_roots`, `RootScope::with_temp_roots`, and the evaluator refresh
helpers once no caller remains.

The former bulk-snapshot `PersistentRoots` type has already been removed now
that long-lived evaluator state uses the persistent root arena and queued host
work is handle-only.

## Step 7: Confine `InternalPtr` to the heap implementation

Rename the raw cell-edge representation to `InternalPtr` and make it private
to the heap implementation. This is the final compile-time audit boundary.

Heap cells may contain `InternalPtr` because the collector owns and rewrites
their complete object graph. No evaluator, scheduler, builder, prelude helper,
conversion trait, environment, or stack module may import or construct it.

This requires replacing direct `Cell` cloning and matching outside the heap
with safe accessors or rooted view types. A rooted view may expose copied scalar
data and `RootedPtr<'scope>` children, but never an internal edge. Keep list
backing traversal within the heap boundary as well; reorganize list helpers if
necessary so `InternalPtr` does not need broader visibility.

Reduce `Collection` to the heap's private tracing interface for GC-managed
cells. Evaluator frames and scheduler state should no longer implement it,
because their references are persistent roots rather than internal edges.

Use a repository-wide search as an acceptance test: `InternalPtr` should occur
only in the heap implementation and its private submodules, while `Pointer`
should no longer exist as a general-purpose runtime type.

## Step 8: Remove hidden heap re-entry paths

Audit every function reachable from the synchronous cycle.

No such function may:

- Accept or clone `Heap`.
- Call a public `Handle` method that locks the heap.
- Drop a value whose destructor locks the heap.
- Invoke host code.
- Block or await.

Provide lock-aware operations on `RootScope` for resolving persistent roots,
promoting a rooted value to a handle, and performing any internal conversion
that currently re-locks through `Heap` or `Handle`.

Keep `Heap` available only to the outer evaluator coordinator, public host
`Context`, public allocation/conversion APIs, and other code that is explicitly
outside an existing heap critical section.

## Step 9: Stress and validate the completed boundary

After the structural migration compiles, enable collection on every allocation
and randomized copy destinations.

Run the full suite and specifically verify:

- The deterministic concurrent host-allocation tests from Step 1.
- `binary_list_equality_uses_visible_elements_across_runtime_shapes`.
- Nested binary-list matching and materialization.
- Captured closure environments and recursive declarations.
- Repeated typeclass method resolution during collection.
- Immediate and deferred host callbacks returning scalar and composite values.
- Foreign-heap handle rejection.
- `cargo run --bin rex -- rex-cli/examples/adt.rex`.

Add a repeated multithreaded stress test that runs concurrent host allocations
on a Tokio multithread runtime. It should use deterministic synchronization for
its assertions; repetition is additional pressure, not a substitute for a
controlled regression.

Restore `GC_EXTREME_STRESS` to `false` after validation so normal execution
retains its production collection policy. Keep targeted tests able to request
collection on every allocation without changing the global default.

Run `./build.sh` after every independently meaningful step and before each
commit.

## Step 10: Document and enforce the invariants

Document the ownership model next to `Heap`, `HeapState`, `RootScope`,
`RootedPtr`, `PersistentPtr`, `InternalPtr`, and `Handle`.

The documentation should state:

- Which types may cross threads and await points.
- Which type proves exclusive heap access.
- Which references are collector-rewritten automatically.
- Which operations may collect.
- Where conversion between temporary, persistent, internal, and public
  references is allowed.

Delete stale comments describing manual pointer refresh as the safety model.
Keep debug assertions for wrong heaps, stale generations, invalid persistent
root identifiers, shadow-stack imbalance, and internal pointers that were not
rewritten during collection.

The migration is complete only when correctness no longer depends on a caller
remembering that a harmless-looking `&Heap` or `Handle` operation may acquire
shared state and collect.
