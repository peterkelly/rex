# Plan: Owned host values and single-owner heap execution

## Rationale

Rex currently allows public `Handle` values and a cloneable `Heap` capability to
cross thread and async boundaries. Supporting that model required
`Arc<Mutex<HeapState>>`, registered roots, handle promotion, and conversion of
the complete evaluator state between scoped and persistent root forms whenever
the evaluator enters or leaves a locked cycle.

Profiling shows that `persist_eval_state` and `resolve_eval_state` account for
approximately 60% of execution time in the measured workload. This is not a
localized implementation inefficiency: the evaluator currently performs an
O(live evaluator state) transformation around a cycle that processes only one
ready work item. Optimizing the individual maps would preserve the fundamental
cost and complexity.

The intended embedding use case does not require shared heap access. Host
functions normally perform expensive scientific operations or invoke external
tools. They can receive owned, backend-independent data and return owned data;
they do not need to retain or allocate Rex heap objects. Consequently, Rex can
give one evaluator task exclusive ownership of its heap and copy semantic
values at host-call boundaries.

This change also creates the correct boundary for two planned but out-of-scope
runtime developments:

- synchronous regions may later be JIT-compiled with LLVM after effect
  analysis; and
- the current cell storage may later be replaced by a binary heap with custom
  allocation that generated code accesses directly.

Neither LLVM, effect analysis, nor the binary heap is implemented by this
plan. The present work must nevertheless avoid exposing Rust cell layouts,
heap addresses, or interpreter-only tags through public APIs. It must also
retain a single-threaded GC root and safepoint mechanism that compiled code can
eventually participate in.

## Goals

1. Make `Value` the only public representation of evaluated Rex data.
2. Make every `Value` an owned, tree-structured value containing no heap
   references.
3. Make `Bytes(Vec<u8>)` the canonical external representation of every Rex
   `List U8`, regardless of its internal list representation.
4. Give the builder/compiler/evaluator pipeline exclusive ownership of
   `HeapState`; host work must never access it.
5. Remove `Heap`, public `Handle`, heap locking, handle promotion, persistent
   evaluator roots, and the per-work-item persist/resolve cycle.
6. Separate public host callables from crate-private evaluator intrinsics.
7. Preserve copying-GC correctness, async host-call concurrency, cancellation
   safety, execution bounds, module loading, and typeclass behavior.
8. Leave a clean internal runtime boundary for future LLVM and binary-heap
   implementations.

## Non-goals

- Implement LLVM IR generation, JIT compilation, or effect analysis.
- Define or implement the future binary heap layout or custom allocator.
- Introduce a new Rex I/O model.
- Preserve `std.io` or the existing host-managed callback/action machinery.
- Provide zero-copy host access to heap-backed strings, byte buffers, or other
  collections.
- Allow closures, native functions, overloaded functions, or uninitialized
  cells to cross the host boundary.
- Stabilize an ABI for generated code in this change set.

## Target architecture and invariants

The runtime should have three deliberately separate representations:

```text
host code             evaluator/interpreter            future LLVM code
---------             ---------------------            ----------------
owned Value   <---->  internal heap references  <----> internal tagged values
no heap access        exclusive HeapState owner         same runtime/heap owner
```

The following invariants are required:

- Only the builder, compiler, or evaluator that currently owns `HeapState` may
  read, allocate in, or collect that heap.
- Host call arguments are converted to owned `Value`s before the host callable
  is invoked or its future is dispatched.
- A host callable owns its arguments. The sync dynamic ABI therefore takes
  `Vec<Value>`, not `&[Value]`.
- Host futures contain `Value`s and host state only. They contain no heap,
  internal pointer, root token, or capability that can obtain one.
- Host results are checked and imported into the heap only after the future
  completes on the evaluator side.
- An internal heap reference may survive an allocation only when it is visible
  to the collector through the current runtime root set.
- Evaluator and compiler roots are traversed only when collection actually
  occurs, not around every evaluator work item.
- Temporary roots remain explicit through a single-threaded scope mechanism.
- Internal pointer types and cell layouts remain crate-private and opaque to
  host APIs.
- No public API depends on the Rust layout of `Value`; future LLVM code will
  not use `Value` as its internal calling convention.

## External `Value` model

Replace the current heap-view `Value` with an owned semantic representation:

```rust
pub enum Value {
    Bool(bool),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    String(String),
    Uuid(Uuid),
    DateTime(DateTime<Utc>),
    Tuple(Vec<Value>),
    List(Vec<Value>),
    Bytes(Vec<u8>),
    Dict(BTreeMap<Symbol, Value>),
    Adt(Symbol, Vec<Value>),
}
```

Exact naming and struct-versus-tuple syntax may be adjusted during
implementation, but the semantic content must remain the same.

The following current variants are internal storage details and must not exist
in public `Value`:

- `Empty`
- `Cons`
- `ListSlice`
- `Data`
- `BinaryData`
- `Uninitialized`
- `Closure`
- `Native`
- `Overloaded`

`Value::List` is the one external representation for ordinary Rex lists.
`Value::Bytes` is not a new Rex language type: it is the canonical external
representation of the existing Rex type `List U8`. A scalar `U8` remains
`Value::U8`.

`Value` may implement `Clone`, but host-call plumbing must move arguments and
results rather than cloning them. Explicitly exported constants may require a
clone or a single import during module installation; that cost must not be
silently imposed on every ordinary host call.

## Type-directed conversion rules

Every heap/host boundary conversion must receive the fully instantiated Rex
type as well as the runtime value. Runtime shape inspection alone is
insufficient: an empty list cannot otherwise be distinguished as `List U8`
versus another list type, and nested byte lists must also be canonicalized.

The outbound conversion has behavior equivalent to:

```rust
fn value_from_heap(
    runtime: &RuntimeScope<'_>,
    pointer: InternalPtr,
    expected: &Type,
) -> Result<Value, EngineError>;
```

The inbound conversion has behavior equivalent to:

```rust
fn value_into_heap(
    runtime: &mut RuntimeScope<'_>,
    value: Value,
    expected: &Type,
) -> Result<InternalPtr, EngineError>;
```

These signatures are illustrative. They must be implemented behind an
internal runtime/memory abstraction rather than becoming a permanent API tied
to the current `HeapState` or cell representation.

Conversion requirements:

- Reject unresolved type variables at an external boundary unless the call
  machinery can first resolve them to a concrete instantiation.
- Validate scalar ranges and exact composite shapes against `expected`.
- Validate tuple arity, record fields, ADT constructor identity and arity, and
  list element types.
- Reject function types and any callable or uninitialized cell, including when
  nested inside another value. Return a structured conversion error rather
  than exposing a sentinel `Value` variant.
- Preserve logical collection order while discarding heap representation
  details and internal sharing. The external result is a tree; shared heap
  subgraphs may be duplicated.
- Avoid recursive Rust call stacks for arbitrarily deep recursive ADT data.
  Use an explicit conversion work stack, while retaining optimized iterative
  paths for lists.
- Keep all knowledge of `Cell`, list slices, and future binary layouts inside
  the memory/runtime conversion layer.

### Canonical byte-list conversion

If `expected` is `List U8`, outbound conversion must always return
`Value::Bytes`, including for an empty list. The specialized logical list
walker must support, in order:

- an empty list;
- a chain of cons cells;
- a list slice backed by `Data`;
- a list slice backed by `BinaryData`;
- cons cells followed by either kind of slice;
- full and partial slices; and
- any equivalent internal representation supported by the heap.

The walker should append directly into one `Vec<u8>`. It must not first build
`Vec<Value>` or one `Value::U8` per byte. When a backing `BinaryData` range is
available, copy the contiguous range directly. When the static type says
`List U8` but the heap contains a non-`u8` element, report an internal/type
consistency error rather than falling back to `Value::List`.

For any other list element type, return `Value::List` and recursively convert
each logical element. Thus `List (List U8)` becomes a `Value::List` whose
elements are `Value::Bytes`.

Inbound conversion treats the representation as canonical as well:

- `Value::Bytes` is accepted when `expected` is `List U8` and is imported into
  the most efficient list representation currently available.
- `Value::List` is rejected for `List U8`; host code must use `Bytes`.
- `Value::Bytes` is rejected for non-`u8` lists.

## Implementation phases

Each phase should leave the workspace compiling and its relevant tests passing.
Temporary adapters may exist between phases, but they must be deleted by the
final cleanup phase.

### Phase 0: Characterize behavior and establish measurements

1. Add or identify a reproducible evaluator benchmark/workload that exhibits
   the current `persist_eval_state`/`resolve_eval_state` cost.
2. Record the baseline runtime and profile in the change description; do not
   commit generated profiler output.
3. Preserve characterization tests for:
   - copying GC under extreme stress;
   - evaluator cancellation while host work is pending;
   - concurrent async host calls and admission limits;
   - module initialization and cached imports;
   - typeclass method evaluation;
   - list slices and hybrid cons/slice lists; and
   - typed and dynamic host exports.
4. Add compile-time auto-trait assertions for the intended ownership model:
   owned host values and host futures must be `Send`; internal scoped roots
   must not become cross-thread capabilities. `HeapState` may be `Send` so the
   owning evaluator future can migrate between executor threads, but it must
   not be shared concurrently through `Sync` APIs.

Exit criterion: the existing behavior and performance problem are covered by
repeatable tests/measurements before structural changes begin.

### Phase 1: Remove the obsolete CLI host-action I/O path

1. Delete `rex-cli/src/modules/stdio.rs` and stop injecting `std.io` from the
   CLI prelude/module list.
2. Remove or replace CLI tests and examples that exist only to exercise
   `std.io`. Do not invent a replacement I/O system in this work.
3. Remove `HostAction`, `HostActionEffect`, `HostActionFuture`, and
   `run_host_action` from `rex-engine`, together with their re-exports from the
   top-level `rex` crate.
4. Remove `Context::resume_callback_once` and its synthetic evaluator-entry
   path.
5. Remove `Evaluator::run_with_context`. Convert remaining CLI callers to the
   ordinary single-shot `run` path.
6. Keep `std.process` and other data-only CLI host functions, migrating them in
   later phases.

Exit criterion: no host API can retain or resume a Rex closure after evaluation
through the current action mechanism.

### Phase 2: Introduce the owned `Value` and conversion kernel

1. Move the public `Value` definition out of the handle-view implementation if
   necessary, so it has no dependency on `Heap`, `Handle`, `RootId`, or `Cell`.
2. Replace collection children with owned `Value`s and add `List` and `Bytes`
   exactly as specified above.
3. Add structured heap-to-value and value-to-heap conversion errors with enough
   path/type context to diagnose a nested unsupported value.
4. Implement type-directed outbound conversion while the old `Heap` wrapper
   still exists as a temporary internal adapter.
5. Implement canonical byte-list traversal over every current list layout,
   including mixed cons/slice layouts.
6. Implement transactional inbound allocation: every partially built composite
   must remain rooted if a later child allocation triggers GC, and failed
   conversion must release temporary roots without leaking registered roots.
7. Implement iterative conversion for deep recursive structures.
8. Add focused unit tests for every scalar and composite variant, nested ADTs,
   records, ordinary lists, byte lists, malformed shapes, and unsupported
   callable values.

Exit criterion: an internal heap value of a concrete representable type can be
round-tripped through the new owned `Value`, and the old public heap-view
variants have no remaining external consumers.

### Phase 3: Move Rust and JSON conversion APIs to owned values

1. Change `IntoRex` so it creates an owned `Value` without receiving a heap.
2. Change `FromRex` so it consumes an owned `Value`. Typed host arguments should
   move strings, byte buffers, and collections into Rust values rather than
   cloning them from a handle.
3. Remove the `IntoRex`/`FromRex` implementations for `Handle`.
4. Preserve the special mapping:
   - Rust `Vec<u8>` <-> `Value::Bytes`;
   - Rust `Vec<T>` for other `T` <-> `Value::List`.
5. Update scalar, tuple, record, `Option`, `Result`, collection, UUID, datetime,
   and derived ADT conversions.
6. Update `rex-proc-macro` derives to construct and consume the owned tree.
7. Change `RexDefault` to return an owned `Value` and remove heap access from
   its public contract.
8. Refactor `rex::json` to convert directly between `serde_json::Value` and
   Rex `Value`, guided by `Type` and `TypeSystem`. It must no longer allocate in
   or inspect a public heap.
9. Change JSON main-input helpers to return `BTreeMap<String, Value>`.
10. Update embedding tests and documentation examples for the new conversion
    signatures.

Exit criterion: all public Rust/JSON conversion is heap-independent and
collection data is owned end-to-end.

### Phase 4: Split host callables from evaluator intrinsics

1. Replace the handle-based host callable ABI with owned values. The dynamic
   APIs should be equivalent to:

   ```rust
   Fn(Context<State>, Type, Vec<Value>) -> Result<Value, EngineError>
   Fn(Context<State>, Type, Vec<Value>) -> Future<Output = Result<Value, EngineError>>
   ```

   The sync form may borrow the `Type`, but it must own `Vec<Value>`.

2. Make public `Context` heapless. It may expose host state and immutable type
   information, but no heap, internal runtime, callback-resumption facility, or
   allocator.
3. Decode typed host arguments before constructing an async future, then move
   the resulting Rust values into that future.
4. Change host-call scheduling records, queued work, pending futures, and
   completions to contain `Value` rather than `Handle`.
5. At the evaluator boundary, derive concrete argument and result types from
   the instantiated call type. Convert arguments before dispatch and import the
   result after completion.
6. Reject a host call when any argument or result is not representable as
   `Value`. Add early registration/type validation when a scheme is statically
   known to contain a function at the boundary, while retaining runtime checks
   for polymorphic instantiations.
7. Rename the crate-private scheduler-native category to an explicit internal
   concept such as `Intrinsic` or `EvaluatorNative`. Its ABI may access the
   internal runtime/root scope and may return either an immediate internal
   value or an evaluator-managed `NativeTask`.
8. Migrate all prelude implementation functions to the internal intrinsic ABI,
   including:
   - higher-order list/dict traversals and folds that invoke Rex closures;
   - structural list operations such as length, slicing, zip, and indexing;
   - `Option`/`Result` inspection and unwrapping;
   - JSON/UUID/datetime primitives;
   - scalar arithmetic, comparison, conversion, and display helpers; and
   - automatically generated ADT constructors.
9. Provide typed internal wrappers for simple scalar intrinsics where useful;
   do not route prelude operations through the public `Value` boundary merely
   to reuse public handler macros.
10. Change staged `export_value` constants to store an owned `Value` until
    module installation, import it once into the owning runtime where
    practical, and expose it as an internal constant rather than a public host
    call on every lookup.

Exit criterion: only host-provided callables use the `Value` ABI; every
heap-aware callable is crate-private and classified as an intrinsic.

### Phase 5: Move evaluation inputs and results to `Value`

1. Change the public evaluator entry point to:

   ```rust
   async fn run(
       self,
       program: CompiledProgram,
       inputs: BTreeMap<String, Value>,
   ) -> Result<Value, EngineError>;
   ```

2. Validate and import each input using its `MainSignature` type after checking
   missing and extra names.
3. Export the final internal result using `program.result_type()` before
   returning it to the host.
4. Remove `Evaluator::heap()` and all same-heap/foreign-heap validation from
   the public API.
5. Move the prepared heap-rooted environment out of `CompiledProgram` and into
   the exclusively owned evaluator/runtime where feasible. `CompiledProgram`
   should describe prepared code and its types, not act as a heap capability.
   If any heap-associated identity remains temporarily, make mismatched
   program/evaluator pairs fail internally and remove that identity by the end
   of Phase 6.
6. Update the CLI, examples, integration tests, fuzz harnesses, and top-level
   `rex` re-exports.

Exit criterion: embedders can compile and run Rex without seeing or creating a
heap object or handle.

### Phase 6: Give the runtime exclusive `HeapState` ownership

1. Replace `Heap { Arc<Mutex<HeapState>> }` fields in `Builder`, `Compiler`, and
   `Evaluator` with direct ownership of `HeapState` or a crate-private runtime
   aggregate that directly owns it.
2. Move that ownership exactly once along the existing single-use pipeline:
   `Builder -> Compiler -> Evaluator`.
3. Keep the evaluator loop and its complete `EvalState` alive across awaits in
   the same owning future. Async host futures hold only `Value`, so they do not
   borrow or share the heap.
4. Store internal pointers directly in evaluator frames, scheduler state, and
   environments across host waits. Do not convert them to registered roots at
   every loop boundary.
5. Introduce a crate-private root traversal/relocation interface for all
   non-heap runtime roots. It must cover at least:
   - evaluator frames and scheduler work items;
   - evaluator environments;
   - compiler/builder top-level environments while they can allocate;
   - typeclass implementation environments;
   - module initialization values and import caches;
   - internal constants; and
   - the local temporary root stack.
6. Arrange allocation/collection through a runtime scope that has mutable
   access to both `HeapState` and the complete current root owner. On an actual
   copying collection, relocate those roots in place or rebuild immutable
   shared structures once. Do not map the entire evaluator state between every
   work item.
7. Retain the useful concept behind `RootScope` for temporary allocation roots,
   but make it single-owner and lock-free. Collection must see both these local
   roots and the long-lived machine roots.
8. Refactor the evaluator loop so it can process ready internal work without
   leaving and re-entering an artificial locked cycle. It should return to the
   async coordinator when it must dispatch/poll host work, reaches a
   cooperative execution budget, or completes.
9. Preserve fairness and `ParallelismController` behavior: ready internal work
   must not starve completed host futures, and host concurrency/admission
   limits must remain effective.
10. Import a completed host `Value` transactionally, root the resulting
    internal pointer, and schedule it into the waiting evaluator frame.
11. Ensure cancellation simply drops the owning runtime and host futures; it
    must not require locking the heap or unregistering roots from another
    thread.
12. Keep the implementation within the current `#![forbid(unsafe_code)]`
    policy. Any future unsafe binary-heap implementation requires a separate,
    explicit design review and is not justified by this refactor.

Exit criterion: no evaluator operation locks a heap, and collection traverses
live evaluator state only when collection occurs.

### Phase 7: Delete obsolete root and handle machinery

After all consumers have migrated:

1. Delete `persist_eval_state` and `resolve_eval_state`.
2. Delete `PersistentEvalState`, `PersistentEnvironment`, `PersistentPtr`, and
   `PersistentRootStore`.
3. Delete public `Handle`, `HandleRoot`, handle root registration/drop logic,
   heap identity validation, and handle promotion (`HandlePromoter` and related
   request promotion).
4. Delete the public `Heap` wrapper and its lock helpers.
5. Remove `ValueSeed`, whose only purpose is moving registered child roots out
   of a heap lock before constructing the old public `Value` view.
6. Remove obsolete registered-root tables if no remaining internal root scope
   requires them. Keep only the lock-free local/root metadata used by actual
   collection.
7. Collapse `ScopedEnvironment`, `PersistentEnvironment`, and
   `RootedEnvironment` into the minimum representations required by the
   single-owner compiler/evaluator. Any mapping of shared environments for a
   copying collection happens only on collection.
8. Remove `Heap` and `Handle` re-exports and update all public rustdoc that
   describes same-heap handles or locked host cycles.
9. Use repository-wide searches to verify that the deleted concepts do not
   survive under aliases.

Exit criterion: the runtime has no public or internal concurrency-oriented heap
capability and no per-step persistent-root transformation path.

### Phase 8: Documentation, validation, and performance verification

1. Update `docs/src/ARCHITECTURE.md` and `docs/src/EMBEDDING.md` with the owned
   `Value` boundary, the host/intrinsic distinction, the single-owner runtime,
   and the cost/ownership implications of host conversion.
2. Document that host functions cannot accept or return function values and
   that `List U8` is represented by `Value::Bytes`.
3. Update `docs/src/SPEC.md` only if implementation changes Rex language
   semantics; the embedding representation alone should not do so.
4. Update public examples and migration notes for the breaking removal of
   `Heap`, `Handle`, `Context::heap`, and `run_with_context`.
5. Run formatting, the full test suite, clippy, fuzz smoke tests, and the
   repository build script required by `CONTRIBUTING.md` before any commit.
6. Re-run the Phase 0 benchmark/profile. Confirm that
   `persist_eval_state`/`resolve_eval_state` are absent and that no equivalent
   whole-state per-work-item traversal replaced them.
7. Profile GC-heavy and host-heavy workloads separately so an improvement in
   ordinary evaluation does not conceal pathological boundary conversion or
   collection behavior.

Exit criterion: correctness checks pass, documentation describes the new API,
and profiling demonstrates that whole evaluator-state persistence is no longer
an execution hot path.

## Test matrix

The implementation should include at least the following targeted regression
coverage in addition to the existing workspace suite.

### `Value` and type-directed conversion

- Every scalar round-trips through `Value`.
- Tuples, records/dicts, and ADTs contain only owned child `Value`s.
- Ordinary empty and non-empty lists produce `Value::List`.
- Empty `List U8` produces `Value::Bytes(Vec::new())`.
- A pure cons-cell `List U8` produces `Bytes`.
- A `Data`-backed slice of `u8` produces `Bytes`.
- A `BinaryData`-backed slice produces `Bytes`.
- Cons cells followed by each slice type produce one correctly ordered
  `Bytes` buffer.
- Partial slices preserve their exact range.
- `List (List U8)` produces a list of `Bytes` values.
- `List U16` containing numerically byte-sized values remains `Value::List`.
- Inbound `Value::List` for `List U8` is rejected.
- Inbound `Value::Bytes` for another list type is rejected.
- Top-level and nested closure/native/overloaded/uninitialized values fail with
  a conversion error.
- Deep recursive data converts without overflowing the Rust stack.

### Host boundaries

- Typed sync and async handlers receive and return scalars, ADTs, ordinary
  lists, and byte lists.
- Dynamic sync and async handlers own `Vec<Value>` arguments.
- An async handler can remain pending while unrelated evaluator work and GC
  proceed, without retaining heap access.
- A malformed or wrong-typed host result fails before entering evaluator
  state.
- A callable argument/result fails at the boundary.
- Async admission permits are released on success, error, and cancellation.
- Host futures are `Send` and contain no heap capability.

### Single-owner GC and evaluation

- Extreme-stress collection relocates values held in every frame shape and
  scheduler work item.
- Roots in top-level environments, typeclass implementations, module caches,
  internal constants, and temporary scopes survive collection.
- Multiple ready evaluator branches and multiple pending host calls remain
  correct under randomized relocation.
- Cancellation at each host-call lifecycle stage drops cleanly.
- No mutex poisoning or registered-root cleanup paths remain.
- The evaluator future can run on a multi-thread Tokio runtime while heap
  access remains exclusive.

### Public API and CLI

- Main inputs and results use `Value` only.
- JSON main inputs and results do not require a heap.
- Derived Rust types round-trip through the owned representation.
- `std.process` continues to work through data-only host values.
- `std.io`, `HostAction`, `Heap`, `Handle`, `run_with_context`, and
  `Context::heap` are absent from the supported API.

## Future LLVM and binary-heap constraints

The following are design constraints for this work, not implementation tasks:

1. `Value` is a host interchange model, not the JIT value representation. LLVM
   code must eventually operate on internal tagged values and the binary heap.
2. Internal intrinsics must be distinguishable from host functions. A future
   backend may lower an intrinsic directly or call an internal runtime helper;
   it must never bypass the `Value` boundary for a host function.
3. The internal runtime abstraction should be able to provide a heap base,
   allocator state, and GC metadata later without changing public handlers.
   Current code must not make the Rust `Cell` enum layout an ABI.
4. Internal pointers should remain opaque outside the memory/runtime layer.
   Do not bake current vector indices, Rust references, or addresses into
   prelude or host interfaces.
5. Definitely synchronous code may still allocate, fail, and trigger GC.
   Future effect analysis must distinguish at least suspension, allocation/GC,
   host calls, and failure; this plan must not encode “synchronous” as
   “allocation-free.”
6. Future compiled allocation slow paths will need all live compiled and
   interpreted references visible at safepoints. The lock-free root traversal
   introduced here must therefore be extensible with JIT stack maps,
   statepoints, or a shadow-root stack.
7. A future moving or growable binary heap may use heap-relative offsets,
   tagged words, reserved address space, or another reviewed pointer scheme.
   This plan does not choose one. It only ensures public `Value` and host APIs
   do not constrain that decision.
8. Canonical `List U8`/`Bytes` conversion must remain based on Rex type and
   logical sequence semantics, not on the current or future physical list
   layout.
9. Zero-copy buffers, pinned heap objects, and shared external allocations are
   intentionally deferred. They would add lifetime and collector obligations
   that conflict with the purpose of the present simplification.

## Completion checklist

The work is complete when all of the following are true:

- [ ] Public collections in `Value` contain only `Value` children.
- [ ] `Value` has canonical `List` and `Bytes` variants and no internal runtime
      variants.
- [ ] Every outbound `List U8`, including empty and hybrid lists, becomes
      `Value::Bytes`.
- [ ] Public host functions and evaluator inputs/results use `Value`, never
      `Handle`.
- [ ] Public `Context` exposes no heap access.
- [ ] Prelude functions and ADT constructors use a crate-private intrinsic ABI.
- [ ] `std.io` and host-action callback resumption are removed.
- [ ] `HeapState` has one exclusive owner and no `Arc<Mutex<_>>` wrapper.
- [ ] `persist_eval_state` and `resolve_eval_state` are deleted.
- [ ] Persistent root stores and handle promotion are deleted.
- [ ] GC relocates evaluator/compiler roots only when collection occurs.
- [ ] Async host futures contain only owned host data and host state.
- [ ] Full tests, clippy, fuzz smoke tests, and build checks pass.
- [ ] Post-change profiling confirms that whole-state persistence is gone and
      has not reappeared under a different name.
- [ ] Documentation and embedding examples describe the new ownership model.
