# Memory Management

`rex-engine` stores evaluator data in a private moving heap. The builder creates
`Heap`, then ownership moves through the single-use pipeline:

```text
Builder -> Compiler -> Evaluator
```

There is no public heap capability and no heap mutex. At any moment one task
owns the complete runtime and has exclusive access to allocation and
collection. The owning evaluator future may migrate between executor threads,
but two threads cannot access the same heap concurrently.

## Representation boundaries

The runtime deliberately separates three concerns:

| Representation | Purpose | Visibility |
| --- | --- | --- |
| `Value` | Owned semantic data passed to and from hosts | Public; no heap references |
| `RootedPtr` | Stable token for evaluator/compiler state | Crate-private |
| `InternalPtr` | Moving edge stored inside heap cells | Memory implementation only |

`Value` is a host interchange model, not the evaluator's internal value or a
future JIT ABI. Its collection variants recursively own other `Value`s.
Evaluator-only states such as closures, native and overloaded functions, and
uninitialized cells cannot be exported.

`Value::List` represents ordinary Rex lists. `Value::Bytes` is the canonical
host representation of Rex `List U8`. Type-directed conversion returns
`Bytes` for every physical list layout, including empty lists, cons chains,
data-backed slices, binary-data slices, and mixed cons/slice values.

## Runtime roots

An `InternalPtr` contains a heap identity, slot, and collection generation.
It is valid only until an allocation that may collect unless it is already a
traced cell edge or represented by a runtime root.

A `RootedPtr` is a stable generational token into the runtime root table. It
does not expose a cell address. Collection rewrites the table entry when an
object moves, allowing evaluator frames, environments, scheduler work, module
values, and compiler state to retain the token. `RootScope` provides exclusive
synchronous access for inspecting and allocating values and explicitly roots
temporary results.

Machine-owned roots remain registered while they are present in evaluator or
compiler state. At a collection safepoint the runtime traverses the current
frames, environments, and scheduler state, removes stale root tokens, and then
runs the collector. This traversal occurs only when collection is required;
the evaluator does not persist and reconstruct its complete state around each
ready work item.

## Copying collection

Collection begins from the live runtime-root table. It traces private
`InternalPtr` edges through reachable cells, copies reachable cells, and
rewrites cell edges and runtime-root entries to the new generation. Debug
builds verify that copied slots have the current generation, contain valid
children, and remain reachable.

The object allocation path also protects child edges already placed in a
pending cell. It temporarily registers those edges as runtime roots, collects,
rewrites the pending cell from the relocated roots, releases the temporary
entries on both success and failure, and only then installs the cell.

Extreme GC stress is available through a builder test setting. It collects at
every evaluator safepoint and randomizes destinations so tests exercise root
relocation rather than relying on stable slot numbers.

## Host calls

Public host functions never receive `Heap`, `RootScope`, `RootedPtr`, or a
capability that can obtain them. Public `Context` retains only host state and
type-system metadata, not runtime registries or their root tokens. The call boundary is:

```text
private heap --type-directed copy--> owned Value
owned Value --host work/future-----> owned Value
owned Value --validate and import--> private heap
```

Arguments are converted before a host callback is invoked. A synchronous
dynamic callback owns `Vec<Value>`; an async callback future owns the same
heap-independent data plus host state. When the result completes, the
evaluator validates it against the instantiated Rex result type and imports it
while it again has exclusive runtime access. Cancellation drops the owning
runtime and host futures without acquiring a lock or unregistering roots from
another thread.

The conversion kernel validates scalar and composite shapes, tuple and ADT
arity, record fields, constructor identity, and concrete element types. It
uses an explicit work stack for recursive data, so deeply nested ADTs do not
consume the Rust call stack. List traversal is specialized and iterative.

Host-provided constants are imported once during module installation and
retained as internal rooted constants. Looking one up does not schedule a host
call or reconvert an owned `Value`.

## Internal intrinsics

Prelude operations and generated constructors are not host functions. They
use a crate-private intrinsic ABI that receives an active root scope and may
inspect or allocate internal values directly. Higher-order intrinsics may
return evaluator-managed work that applies Rex closures. This distinction is
important: only genuinely external host work pays for conversion to and from
`Value`.

## Future runtimes

The current heap uses Rust cells, but no public API exposes their layout.
Future LLVM-generated sequential regions can use private tagged values and a
custom binary heap while preserving the same host boundary. Compiled code
that may allocate will need to report live references at safepoints through
stack maps, statepoints, or a compatible shadow-root mechanism. `Value` will
remain the host interchange tree rather than becoming the generated-code
calling convention.

No `unsafe` code is used by the current memory model.
