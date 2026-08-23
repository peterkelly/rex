# Embedding Rex in Rust

Rex is designed as a small pipeline you can embed at whatever stage you need:

1. `rex-parser`: source → `CompilationUnit { decls, body }`
2. `rex-typesystem`: HM inference + type classes → `TypedExpr` (plus predicates/type)
3. `rex-engine`: build host modules, compile typed code into `CompiledProgram`, then run it →
   `rex_engine::Value`

This document focuses on common embedding patterns.

## Running Untrusted Rex Code (Production Checklist)

This repo provides language-level parsing limits and a pure evaluator suitable for embedding. Your
production server is responsible for enforcing hard resource limits (process isolation, wall-clock
timeouts, memory limits).

Recommended defaults for untrusted input:

- Parsing enforces a fixed AST-depth cap.
- Run evaluation in an isolation boundary you can hard-kill (separate process/container), with CPU/RSS/time limits.

Evaluation API:

- Evaluation is async via `Evaluator`.

## Compile Then Run

`rex-engine` now has an explicit preparation boundary:

- `Builder` builds the host environment.
- `Compiler` prepares user code into a `CompiledProgram`.
- `Evaluator` owns the runtime core and runs one prepared program with runtime inputs for `main`.

The whole builder/compiler/evaluator lineage is single-use. `Builder::build_compiler()` consumes
the builder, `Compiler::compile_program` and `Compiler::infer_*` consume the compiler, and
`Evaluator::run` consumes the evaluator, the compiled program, and a `BTreeMap<String, Value>` of
inputs. Programs are compiled with Rex's singular external interface semantics: an explicit
`fn main ...` defines named runtime inputs, while a final expression without `main` is treated as
an implicit zero-input `main`.

```rust,ignore
use rex::{
    engine::{CompileOptions, Builder},
    parser::parse,
};

let builder = Builder::with_prelude(())?;
let compiler = builder.build_compiler();

let parsed = parse("let x = 1 + 2 in x * 3").map_err(|errs| format!("{errs:?}"))?;
let (program, evaluator) = compiler
    .compile_program(&parsed, CompileOptions::for_module("workflow.main")?)
    .await?;
assert_eq!(program.result_type().to_string(), "i32");
let value = evaluator.run(program, Default::default()).await?;
```

What "compiled" means in the current design:

- parsing, import rewriting, declaration injection, and typechecking have already happened
- `CompiledProgram` carries a typed expression plus the environment snapshot needed to run it
- `CompiledProgram::main_signature()` reports input names/types and the external result type
- `Evaluator` owns the runtime core needed for execution
- `Evaluator::run` consumes the evaluator, compiled program, and runtime input map; use a new
  builder/compiler/evaluator lineage for another generated workflow

What is captured:

- Rex declarations that are part of the prepared program are captured into the compiled env snapshot
- host-provided exports registered through `export`, `export_async`, `export_native`,
  `export_native_async`, or `export_value` are carried by the evaluator produced from the same
  compiler
- typeclass method bindings are carried by that same evaluator runtime

That means a `CompiledProgram` is intended to be run by the evaluator created from the same
compiler. Rex does not currently expose a portable compiled artifact or cross-runtime linking
model.

Phase-specific errors:

- `Compiler` APIs return `EngineError`
- `Evaluator::run` returns `EngineError`
- APIs that parse, compile, and run in one call return `ExecutionError` because they cross
  phase boundaries

### Runtime Values and Heap Ownership

Rex uses a moving copying collector, but the heap is entirely private and has one owner. External
`main` inputs and evaluation results are owned `Value` trees containing no heap references.
Composite variants recursively contain `Value`; closures, native functions, overloaded functions,
and uninitialized cells cannot cross this boundary and produce a conversion error.

`Value::List` represents ordinary lists. `Value::Bytes` is the required representation for Rex
`List U8`, including empty lists and lists whose internal representation mixes cons cells and
vector-backed slices. `Bytes` is an embedding optimization, not a distinct Rex language type.

Host functions receive owned values before they start and return owned values that are validated
and imported only after completion. Async host futures therefore contain no heap capability. See
[Memory Management](MEMORY_MANAGEMENT.md) for the complete internal ownership model.

Compile parsed Rex sources with `Compiler::compile_program` and pass the resulting
`CompiledProgram` to `Evaluator::run`.

## Evaluate Rex Code Directly

```rust,ignore
use rex::{
    engine::{Builder, CompileOptions},
    parser::parse,
};

let program = parse("let x = 1 + 2 in x * 3").map_err(|errs| format!("{errs:?}"))?;

let builder = Builder::with_prelude(())?;
let compiler = builder.build_compiler();
let (program, evaluator) = compiler
    .compile_program(&program, CompileOptions::for_module("workflow.main")?)
    .await?;
let value = evaluator.run(program, Default::default()).await?;
println!("{value}");
```

Rex source modules loaded via importers must be declaration-only. To run an expression, use snippet
or program entry points.
Qualified alias members used in type/class positions (annotations, `where` constraints, instance
headers, superclass clauses) are validated against module exports during module processing; missing
exports fail early with module errors.

## Builder Initialization and Default Imports

`Builder::with_prelude(state)` is shorthand for `Builder::with_options(state, EngineOptions::default())`.

- Prelude is enabled by default.
- `std.prelude` is default-imported.
- Default imports are weak: they fill missing names, but never override local declarations
  or explicit imports.

If you want full control:

```rust,ignore
use rex::engine::{Builder, EngineOptions, PreludeMode};

let mut builder = Builder::with_options(
    (),
    EngineOptions {
        prelude: PreludeMode::Disabled,
        default_imports: vec![],
    },
)?;
```

## Inject Modules (Embedder Patterns)

This is fully supported in `rex-engine`. You can compose module loading from:

- the bundled `std.prelude` virtual module
- modules injected with `Builder::inject_module`
- Rust modules returned lazily by importers
- custom async importers (for DB/object-store/in-memory modules)

### 1) Use an Explicit Importer

`rex-engine` does not read module files from disk by default. File-backed loading is a host
policy decision; the CLI installs its own filesystem importer, while embedded applications should
provide an importer that matches their trust boundary.
Use `DenyImporter` when you need an explicit importer implementation that rejects every module
request.

Notes:

- importers receive an `ImportRequest` with the requested `ModuleId` and optional importing
  module id.
- a `ModuleId` is a qualified namespace name, not a filesystem path; the CLI's filesystem mapping
  is one importer policy, not a core engine rule.
- snippets and parsed programs load Rex source modules through the compiler's import rewriting
  path; source-backed modules remain declaration-only.
- importer results are cached for one compile, so the same request is not sent through the importer
  chain repeatedly.
- import clauses (`(*)` / item lists) import exported names into unqualified scope.
- unqualified imports are context-sensitive: expression positions use values, type positions use
  types, and class/constraint positions use classes.
- module aliases (`import x as M`) provide qualified access to exported values, types, and classes.
- importing a name only brings in the facets that actually exist under that name.

### 2) Inject In-Memory Rex Modules

For host-managed modules, either call `Builder::inject_module` or add an importer that maps
module IDs to source text or prebuilt compilation units.

```rust,ignore
use futures::future::BoxFuture;
use rex::{
    engine::{
        CompileOptions, Builder, ImportRequest, Importer, ResolvedModule, ResolvedModuleContent,
    },
    parser::parse,
};
use std::collections::HashMap;
use std::sync::Arc;

let mut builder = Builder::with_prelude(())?;

let modules = Arc::new(HashMap::from([
    (
        "acme.math".to_string(),
        "pub fn inc : i32 -> i32 = \\x -> x + 1;".to_string(),
    ),
    (
        "acme.main".to_string(),
        "import acme.math (inc);\npub fn main : i32 = inc 41;".to_string(),
    ),
]));

struct MapImporter {
    modules: Arc<HashMap<String, String>>,
}

impl Importer for MapImporter {
    fn import<'a>(
        &'a self,
        req: ImportRequest,
    ) -> BoxFuture<'a, Result<Option<ResolvedModule>, rex::engine::EngineError>> {
        Box::pin(async move {
            let module_name = req.module_id.to_string();
            let Some(source) = self.modules.get(&module_name) else {
                return Ok(None);
            };
            Ok(Some(ResolvedModule {
                id: req.module_id,
                content: ResolvedModuleContent::Source(source.clone()),
            }))
        })
    }
}

builder.add_importer(Arc::new(MapImporter { modules }));
let compiler = builder.build_compiler();
let parsed = parse("import acme.main (main);\nmain").map_err(|errs| format!("{errs:?}"))?;
let (program, evaluator) = compiler
    .compile_program(&parsed, CompileOptions::for_module("workflow.main")?)
    .await?;
let value = evaluator.run(program, Default::default()).await?;
println!("{value}");
```

### 3) Host-Provided Rust Functions, Exposed as Modules

This is the common embedder case.

Use `Module` + `Builder::inject_module(...)`:

1. Create a `Module`.
2. Add exports:
   - typed exports with `export` / `export_async`
   - runtime/native exports with `export_native` / `export_native_async`
   - optional structured declarations with `add_rex_adt` / `add_adt_decl`
   - optional typeclass instances for existing classes with `add_instance`
3. Inject it into the builder.

`Module::add_rex_adt::<T>()` now stages the full acyclic ADT family reachable from `T`.
This is driven by `RexType::collect_rex_family(...)`: ADT types contribute declarations there,
while leaf Rex types inherit a no-op default. For example, if `Label` contains a `Side`, staging
`Label` is enough; you do not need to stage `Side` separately. Cyclic ADT families are still
rejected.

`Module` is intentionally narrower than a general Rex declaration package. Embedders can stage
host-provided ADTs, host exports, and instances of existing typeclasses. Arbitrary Rex declarations
belong in `CompilationPackage`, not `Module`. Type declarations come from the staged ADTs; call
`Module::declarations()` when you need the derived `Declarations` package view used by the
compiler.

`export` handlers are fallible and must return `Result<T, EngineError>`. If a handler returns
`Err(...)`, evaluation fails with that engine error.
`export_async` handlers follow the same rule, but return
`Future<Output = Result<T, EngineError>>`.

Both forms receive owned arguments copied from the evaluator heap. The returned owned value is
validated and imported in a later evaluator cycle. Synchronous handlers resume through an
immediately-ready native completion; they do not consume async-native permits or pass through
`AsyncCallPolicy`. They run on the evaluator task, so blocking or long-running work belongs in an
asynchronous export.

```rust,ignore
use rex::{
    engine::{CompileOptions, Builder, Module},
    parser::parse,
};

let mut builder = Builder::with_prelude(())?;

let mut math = Module::new(
    "acme.math",
    Some("Arithmetic operations provided by the host.".to_owned()),
);
math.export("inc", |_state: (), x: i32| { Ok(x + 1) })?;
math.export_async("double_async", |_state: (), x: i32| async move { Ok(x * 2) })?;
builder.inject_module(math)?;
let compiler = builder.build_compiler();
let parsed = parse("import acme.math (inc, double_async as d);\ninc (d 20)")
    .map_err(|errs| format!("{errs:?}"))?;
let (program, evaluator) = compiler
    .compile_program(&parsed, CompileOptions::for_module("workflow.main")?)
    .await?;
let value = evaluator.run(program, Default::default()).await?;
println!("{value}");
```

For API surfaces primarily consumed by agents, use the registration attributes so Rust doc
comments become Rex metadata automatically:

```rust,ignore
/// Arithmetic tools exposed by this host.
#[rex::module(name = "acme.math", defaults(Input))]
mod math {
    use rex::engine::EngineError;

    /// A value supplied to arithmetic operations.
    #[derive(Clone, Default, rex::Rex)]
    #[rex(export)]
    pub struct Input {
        /// The integer to operate on.
        pub value: i32,
    }

    /// Increment an input by one.
    #[rex::export(name = "inc")]
    pub fn increment(_state: (), input: Input) -> Result<Input, EngineError> {
        Ok(Input { value: input.value + 1 })
    }
}

let mut builder = rex::engine::Builder::with_prelude(())?;
builder.inject_module(math::rex_module()?)?;
```

`#[rex::module]` generates `rex_module()`. It copies the module's Rust doc comments, collects
functions marked `#[rex::export]`, and stages non-generic derived ADTs marked `#[rex(export)]`.
`#[rex::export]` supports synchronous and asynchronous functions, preserves the function's Rust
doc comments and Rex-visible parameter names, and generates a `<function>_rex_export()` helper.
Every derived ADT reachable through an exported function's argument or result types is staged
automatically, including ADTs nested inside standard containers.

The optional `defaults(Type, ...)` module argument stages a qualified Rex `Default` instance for
each listed concrete Rust type. A listed type must implement `RexDefault<State>` and `IntoRex`;
ordinary Rust `Default` types receive `RexDefault` through its blanket implementation. The native
value producer is private, while the instance becomes available when the named module is imported.
This permits explicitly typed option construction such as `tool Options {}` or
`tool Options { retries = 3 }`. Omitted fields come from the registered default.

Rust does not allow `#[doc]` attributes or doc comments on individual function parameters.
Parameter descriptions therefore belong in the function-level doc comment; Rex metadata stores
only each parameter's Rust identifier. The host-state parameter is not part of the Rex signature
or its parameter-name metadata.

The lower-level APIs remain available for dynamic registration. Pass module documentation as the
second argument to `Module::new`; use `Export::with_docs` and `Export::with_param_names` for an
export; set `AdtDecl::docs`; and pass variant documentation to `AdtDecl::add_variant`. `AdtParam`,
`AdtArgument`, and `AdtField` carry documentation for the corresponding parts of an ADT. Repeated
ADT registrations merge missing documentation and reject contradictory documentation for the
same declaration. Named modules preserve generic-parameter documentation through their
intermediate `TypeDecl` using documented `TypeParam` entries. Rustdoc itself does not render
generic-parameter documentation, so put `#[allow(unused_doc_comments)]` on a documented generic
parameter when warnings are denied.
`Module::global()` keeps its no-argument signature and creates the root module without module-level
documentation.

Before injection, metadata can be inspected through `Module::docs`, `Module::exports` (using each
export's `docs()` and `params()` methods), and `Module::adts`. After registration, the same function
and ADT metadata lives on `RegisteredValue` and `AdtDecl` entries in the builder or compiler's
`TypeSystem`.

Documentation is also preserved in the JSON-facing `TypeBundle` wire format. A bundle can carry
explicit bundle-level docs, documented overloads and parameter names, and documented ADTs down to
type parameters, variants, constructor arguments, and record fields.
`TypeBundle::from_registered_values` preserves the value docs and parameter names in its
`RegisteredValue` inputs, but leaves the bundle-level `docs` field unset; call
`TypeBundle::with_docs` when the bundle itself needs docs.
`TypeBundle::from_schemes` has no value docs to preserve and generates names such as `arg0` for
function parameters. The manifest builder currently uses this latter path. Wire parameter metadata
is a list of strings because individual parameters have no documentation of their own. The wire
format intentionally has no schema-version constant or `schemaVersion` field. When persisting a
virtual module as a bundle, its module-level docs can be stored in this top-level bundle field.
`TypeBundle::into_parts` returns a `DecodedTypeBundle` with named `docs`, `adts`, and `values`
fields. `TypeBundle::register_into` installs those ADTs and returns a `RegisteredTypeBundle` with
named `docs` and `values` fields.

You can declare ADTs directly inside an injected host module:

```rust,ignore
use rex_ast::Symbol;
use rex_engine::{Builder, Module};
use rex_typesystem::types::{AdtArgument, BuiltinTypeId, Type};

let mut builder = Builder::with_prelude(())?;

let mut m = Module::new(
    "acme.status",
    Some("Status values returned by host operations.".to_owned()),
);
let mut status = builder.adt_decl("Status", &[]);
status.docs = Some("The state of a host operation.".to_owned());
status.add_variant(
    Symbol::intern("Ready"),
    vec![],
    Some("The operation completed successfully.".to_owned()),
);
status.add_variant(
    Symbol::intern("Failed"),
    vec![AdtArgument::Positional {
        typ: Type::builtin(BuiltinTypeId::String),
        docs: Some("A human-readable failure message.".to_owned()),
    }],
    Some("The operation failed.".to_owned()),
);
m.add_adt_decl(status)?;
builder.inject_module(m)?;
```

Then Rex code can import and use those names from the module:

```rex
import acme.status (Status, Failed);

let fail: String -> Status = \msg -> Failed msg in
match (fail "boom") with {
  case Failed msg -> length msg;
  case _ -> 0;
}
```

`Status` is used here in type position, while `Failed` is used in expression/pattern positions.
They are imported through the same name-based mechanism.

Internally this generates module declarations and injects host implementations under qualified
module export symbols.

If you need to construct exports separately (for example to build a module from plugin metadata),
you can use:

- `Export::from_handler` / `Export::from_async_handler` (typed handlers)
- `Export::from_native` / `Export::from_native_async` (value-based native handlers)

These constructors initially use generated parameter names such as `arg0` and have no docs. Chain
`Export::with_param_names` and `Export::with_docs` when supplying API metadata, then add the export
with `Module::add_export`. Adding it also stages any derived ADT family required by the export's
Rust signature.

This example shows how to use Rust enums and structs as Rex-facing types with ADTs declared inside
the module itself. The host function accepts a Rust `Label` (containing a Rust `Side` enum), and
Rex code calls it through `sample.render_label`.

Example:

```rust,ignore
use rex::{
    Rex,
    engine::{CompileOptions, Builder, EngineError, Module},
    parser::parse,
};

#[derive(Clone, Debug, PartialEq, Rex)]
enum Side {
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq, Rex)]
struct Label {
    text: String,
    side: Side,
}

fn render_label(label: Label) -> String {
    match label.side {
        Side::Left => format!("{:<12}", label.text),
        Side::Right => format!("{:>12}", label.text),
    }
}

let mut builder = Builder::with_prelude(())?;

let mut m = Module::new("sample", None);
m.add_rex_adt::<Label>()?;
m.export("render_label", |_state: (), label: Label| {
    Ok::<String, EngineError>(render_label(label))
})?;
builder.inject_module(m)?;
let compiler = builder.build_compiler();
let parsed = parse(
    r#"
    import sample (Label, Left, Right, render_label);
    (
        render_label (Label { text = "left", side = Left }),
        render_label (Label { text = "right", side = Right })
    )
    "#,
)
.map_err(|errs| format!("{errs:?}"))?;
let (program, evaluator) = compiler
    .compile_program(&parsed, CompileOptions::for_module("workflow.main")?)
    .await?;
let value = evaluator.run(program, Default::default()).await?;
println!("{value}"); // ("left        ", "       right")
```

In that example:

- `Label` is imported once and then used as both a type name and a constructor value.
- `Left` and `Right` are imported as constructor values.
- `render_label` is imported as a value.

### 3a) Runtime-Defined Signatures (`Value` APIs)

If your host determines function signatures/behavior at runtime, use the native module export
APIs and provide an explicit `Scheme` + arity:

- `Module::export_native`
- `Module::export_native_async`

These callbacks receive `Context<State>` (not just `&State`), so they can:

- read state via `ctx.state()`
- inspect typed call information via the explicit `&Type` / `Type` callback parameter

Async native callbacks receive owned argument vectors and return `Send + 'static` futures. The
host scheduler owns and polls those futures while the evaluator retains exclusive heap ownership.
`Context` retains only shared host state and type-system metadata; it does not retain evaluator
registries, runtime roots, or another indirect heap capability.
Synchronous native callbacks use the same `Context`/`Value` boundary and completion machinery,
but produce an immediately-ready result. They are not subject to async admission or executor
policy and should remain short and nonblocking.

A callback owns its `Vec<Value>` arguments and may move an argument directly into its result. It
may also construct a new owned `Value`; it cannot inspect or allocate in the evaluator heap.

```rust,ignore
use futures::FutureExt;
use rex_engine::{Builder, Context, Module, Value};
use rex::typesystem::{BuiltinTypeId, Scheme, Type};

let mut builder = Builder::with_prelude(())?;

let mut m = Module::new("acme.dynamic", None);
let scheme = Scheme::new(vec![], vec![], Type::fun(Type::builtin(BuiltinTypeId::I32), Type::builtin(BuiltinTypeId::I32)));

m.export_native("id_value", scheme.clone(), 1, |_ctx: Context<()>, _typ: &Type, mut args: Vec<Value>| {
    Ok(args.remove(0))
})?;

m.export_native_async("answer_async", Scheme::new(vec![], vec![], Type::builtin(BuiltinTypeId::I32)), 0, |_ctx: Context<()>, _typ: Type, _args: Vec<Value>| {
    async move { Ok(Value::I32(42)) }.boxed()
})?;

builder.inject_module(m)?;
```

`Scheme` and arity must agree. Registration returns an error if the type does not accept the
provided number of arguments.

### 3b) Lazy Rust Modules From Importers

If many Rust modules are available but most programs import only a few, an importer can build and
return a `Module<State>` on demand. This keeps `Builder::inject_module` as the eager path, while
letting embedders defer expensive module construction until Rex code actually imports that module.

```rust,ignore
use futures::future::BoxFuture;
use rex::{
    engine::{
        Builder, CompileOptions, EngineError, ImportRequest, Importer, Module, ModuleId,
        ResolvedModule, ResolvedModuleContent,
    },
    parser::parse,
};
use std::sync::Arc;

#[derive(Clone)]
struct ToolImporter {
    tools_id: ModuleId,
}

impl Importer for ToolImporter {
    fn import<'a>(
        &'a self,
        req: ImportRequest,
    ) -> BoxFuture<'a, Result<Option<ResolvedModule>, EngineError>> {
        Box::pin(async move {
            if req.module_id != self.tools_id {
                return Ok(None);
            }

            let mut tools = Module::new(self.tools_id.to_string(), None);
            tools.export("inc", |_state: (), x: i32| Ok(x + 1))?;

            Ok(Some(ResolvedModule {
                id: self.tools_id.clone(),
                content: ResolvedModuleContent::module(tools),
            }))
        })
    }
}

let mut builder = Builder::with_prelude(())?;
let tools_id = ModuleId::parse("workflow.tools")?;
builder.add_importer(Arc::new(ToolImporter { tools_id }));

let compiler = builder.build_compiler();
let parsed = parse("import workflow.tools (inc);\ninc 41")
    .map_err(|errs| format!("{errs:?}"))?;
let (program, evaluator) = compiler
    .compile_program(&parsed, CompileOptions::for_module("workflow.main")?)
    .await?;
let value = evaluator.run(program, Default::default()).await?;
println!("{value}");
```

Lazy Rust-module rules:

- the returned `Module<State>` must use the same `State` type as the builder/compiler.
- the module must be a named module, not `Module::global()`.
- the module's qualified name must match the returned `ResolvedModule.id`.
- lazy Rust modules are installed through the same internal path as eager named
  `Builder::inject_module`, so exports, module-local ADTs, type declarations, caches, and native
  runtime registrations behave the same.
- a lazy Rust module is self-contained host code; the engine does not run the Rex source SCC loader
  over imports inside it.

### 4) Custom Importer Contract (Advanced)

If you need dynamic/nonstandard module loading behavior, implement `Importer<State>`.

Importer contract:

- return `Ok(Some(ResolvedModule { ... }))` when you can satisfy the module.
- return `Ok(None)` to let the next importer try.
- return `Err(...)` for hard failures (invalid module payload, policy violations, etc.).

`ResolvedModule<State>` can carry:

- `ResolvedModuleContent::Source(...)` for Rex source text.
- `ResolvedModuleContent::CompilationPackage(...)` for preconstructed structured Rex modules.
- `ResolvedModuleContent::module(...)` for a Rust-backed `Module<State>` installed lazily.

### 5) Snippets That Import Relative Modules

If you evaluate ad-hoc Rex snippets that contain imports, give the snippet an
explicit module name in `CompileOptions`. Importers receive that name as
`ImportRequest::importer` and decide how requested module IDs map to files, databases, in-memory
source, Rust modules, or any other backing store. The core engine treats module IDs as names in an
abstract namespace; filesystem-relative behavior is an importer policy.

```rust,ignore
use rex::{
    engine::{CompileOptions, Builder},
    parser::parse,
};

let builder = Builder::with_prelude(())?;
let compiler = builder.build_compiler();
let parsed = parse("import foo.bar as Bar;\nBar.add 1 2")
    .map_err(|errs| format!("{errs:?}"))?;
let (program, evaluator) = compiler
    .compile_program(&parsed, CompileOptions::for_module("workflow.snippet")?)
    .await?;
let value = evaluator.run(program, Default::default()).await?;
```

## Builder State

`Builder` is generic over host state: `Builder<State>`, where
`State: Clone + Send + Sync + 'static`.
The state is owned by the builder, moved into the compiler/runtime lineage, and shared across all
injected functions.

- Use `Builder::with_prelude(())?` if you do not need host state.
- If you do, pass your state struct into `Builder::new(state)` or `Builder::with_prelude(state)`.
- `export` / `export_async` callbacks receive an owned clone of `State` as their first parameter.
- Value-based native APIs (`export_native*`) receive `Context<State>` so they can read
  `ctx.state()`; the context deliberately exposes no heap access.

```rust,ignore
use rex_engine::{Builder, Module};

#[derive(Clone)]
struct HostState {
    user_id: String,
    roles: Vec<String>,
}

let mut builder: Builder<HostState> = Builder::with_prelude(HostState {
    user_id: "u-123".into(),
    roles: vec!["admin".into(), "editor".into()],
})?;

let mut globals = Module::global();
globals.export("have_role", |state, role: String| {
    Ok(state.roles.iter().any(|r| r == &role))
})?;
builder.inject_module(globals)?;
```

## List Interop at Host Boundaries

Rex exposes one collection type to user code: `List a`. Rust `Vec<T>` values
convert to and from `List T`, so host functions can accept list literals and
return list values without explicit representation conversions.

```rex
accept_bytes [1, 2, 3]
```

where `accept_bytes` is exported from Rust with a `Vec<u8>` parameter.

Internally, lists may be represented either as linked `Cons`/`Empty` cells or
as a slice over contiguous heap data. That choice is not exposed to Rex code:
list constructors, list literals, pattern matching, and prelude collection
functions all operate on the same `List a` abstraction.

For `Vec<u8>`, Rex uses a binary data backing so host byte buffers do not need
one heap allocation per byte. The Rex type is still `List u8`, and host
functions accepting `Vec<u8>` can read lists backed by binary data, ordinary
list data, or cons cells followed by either backing.

```rex
match bytes with {
    case Cons head _ -> head;
    case Empty -> 0;
}
```

## Typecheck Without Evaluating

```rust,ignore
use rex::{
    engine::standard_type_system,
    parser::parse,
    typesystem::infer,
};

let program = parse("map (\\x -> x) [1, 2, 3]").map_err(|errs| format!("{errs:?}"))?;

let mut ts = standard_type_system()?;
for decl in &program.decls {
    match decl {
        rex_ast::Decl::Type(d) => ts.register_type_decl(d)?,
        rex_ast::Decl::Class(d) => ts.register_class_decl(d)?,
        rex_ast::Decl::Instance(d) => {
            ts.register_instance_decl(d)?;
        }
        rex_ast::Decl::Fn(d) => ts.register_fn_decls(std::slice::from_ref(d))?,
    }
}

let body = program
    .body
    .as_ref()
    .expect("snippet must contain a final expression");
let (preds, ty) = infer(&mut ts, body.as_ref())?;
println!("type: {ty}");
if !preds.is_empty() {
    println!(
        "constraints: {}",
        preds.iter()
            .map(|p| format!("{} {}", p.class, p.typ))
            .collect::<Vec<_>>()
            .join(", ")
    );
}
```

## Type Classes and Instances

Users can declare new type classes and instances directly in Rex source. As the host, you:

1. Parse Rex source into `CompilationUnit { decls, body }`.
2. Inject `Decl::Class` / `Decl::Instance` into the type system (if you’re typechecking without running).
3. Compile the full program through `Compiler` (if you’re running), so instance method bodies are
   available at runtime.

### Typecheck: Inject Class/Instance Decls into `TypeSystem`

```rust,ignore
use rex::{
    engine::standard_type_system,
    parser::parse,
    typesystem::infer,
};

let code = r#"
class Size a where {
    size : a -> i32;
}
instance<t> Size (List t) where {
    size = \xs ->
        match xs {
            case Empty -> 0;
            case Cons _ rest -> 1 + size rest;
        };
}
size [1, 2, 3]
"#;

let program = parse(code).map_err(|errs| format!("{errs:?}"))?;

let mut ts = standard_type_system()?;
for decl in &program.decls {
    match decl {
        rex_ast::Decl::Type(d) => ts.register_type_decl(d)?,
        rex_ast::Decl::Class(d) => ts.register_class_decl(d)?,
        rex_ast::Decl::Instance(d) => {
            ts.register_instance_decl(d)?;
        }
        rex_ast::Decl::Fn(d) => ts.register_fn_decls(std::slice::from_ref(d))?,
    }
}

let body = program
    .body
    .as_ref()
    .expect("snippet must contain a final expression");
let (_preds, ty) = infer(&mut ts, body.as_ref())?;
assert_eq!(ty.to_string(), "i32");
```

### Evaluate: Inject Decls into `Builder`

```rust,ignore
use rex_engine::{Builder, CompileOptions};
use rex::parser::parse;

let code = r#"
class Size a where {
    size : a -> i32;
}
instance<t> Size (List t) where {
    size = \xs ->
        match xs {
            case Empty -> 0;
            case Cons _ rest -> 1 + size rest;
        };
}
(size [1, 2, 3], size [])
"#;

let program = parse(code).map_err(|errs| format!("{errs:?}"))?;

let builder = Builder::with_prelude(())?;
let compiler = builder.build_compiler();
let (compiled, evaluator) = compiler
    .compile_program(&program, CompileOptions::for_module("workflow.main")?)
    .await?;
let _ty = compiled.result_type().clone();
let value = evaluator.run(compiled, Default::default()).await?;
println!("{value}");
```

## Inject Native Values and Functions

`rex-engine` is the boundary where Rust provides implementations for Rex values.

For host-provided *modules*, prefer `Module` + `inject_module` (above). For root-scope values
or functions, use `Module::global()` and inject that staged module into the builder.

```rust,ignore
use rex_engine::{Builder, Module};

let mut builder = Builder::with_prelude(())?;
let mut globals = Module::global();
globals.export_value("answer", 42i32)?;
globals.export("inc", |_state, x: i32| { Ok(x + 1) })?;
builder.inject_module(globals)?;
```

Owned constants are converted and imported once when their module is installed. Reading a constant
does not invoke a host callback or repeat the `Value` boundary conversion.

### Integer Literal Overloading with Host Natives

Integer literals are overloaded (`Integral a`) and can specialize at call sites. This works for
direct calls, `let` bindings, and lambda wrappers:

```rust,ignore
use rex::parser::parse;
use rex_engine::{Builder, CompileOptions, Module};

for code in [
    "num_u8 4",
    "let x = 4 in num_u8 x",
    "let f = \\x -> num_i64 x in f 4",
] {
    let mut builder = Builder::with_prelude(())?;
    let mut globals = Module::global();
    globals.export("num_u8", |_state: (), x: u8| Ok(format!("{x}:u8")))?;
    globals.export("num_i64", |_state: (), x: i64| Ok(format!("{x}:i64")))?;
    builder.inject_module(globals)?;

    let program = parse(code).map_err(|errs| format!("parse error: {errs:?}"))?;
    let compiler = builder.build_compiler();
    let (compiled, evaluator) = compiler
        .compile_program(&program, CompileOptions::for_module("workflow.main")?)
        .await?;
    let _ty = compiled.result_type().clone();
    let value = evaluator.run(compiled, Default::default()).await?;
    println!("{value}");
}
```

Negative literals specialize only to signed numeric types. For example, `num_i32 (-3)` is valid,
while `num_u32 (-3)` is a type error.

Float literals are similarly context-sensitive for primitive float widths. A literal such as `3.0`
defaults to `f32` when unconstrained, but specializes to `f64` when passed to a native or Rex
function whose argument type is `f64`.

### Async Natives

If your host functions are async, stage them in a module with `export_async` and run the compiled
program with `Evaluator::run`.

```rust,ignore
use rex::parser::parse;
use rex_engine::{Builder, CompileOptions, Module};

let mut builder = Builder::with_prelude(())?;
let mut globals = Module::global();
globals.export_async("inc", |_state, x: i32| async move { Ok(x + 1) })?;
builder.inject_module(globals)?;

let program = parse("inc 1").map_err(|errs| format!("parse error: {errs:?}"))?;
let compiler = builder.build_compiler();
let (compiled, evaluator) = compiler
    .compile_program(&program, CompileOptions::for_module("workflow.main")?)
    .await?;
let _ty = compiled.result_type().clone();
let v = evaluator.run(compiled, Default::default()).await?;
println!("{v}");
```

By default, admitted async host futures are polled inline by the evaluator. This keeps the runtime
portable and avoids assuming a particular runtime, which is important for wasm embedders. Inline
polling is fine for futures that are naturally non-blocking, but CPU-heavy or blocking work should
be moved onto an executor supplied by the embedding application.

Admission, callback invocation, and future polling occur without lending out the evaluator heap.
Arguments and completed results are owned `Value`s throughout suspension, so an async callback may
retain or move them without depending on a heap location.

Use `set_parallelism_controller` to decide when async host callbacks may be invoked. A
`ParallelismController` grants a `NativeAsyncPermit` for each admitted
async native call; the permit is held until that call completes. Controllers can therefore enforce
process-local limits, shared limits across several evaluators, or externally coordinated limits
backed by a cluster scheduler.

`ExecutionBounds` remains available as a fixed controller. Its `max_ready_work` value is only an
internal evaluator queue-pressure guard: it limits how many already-created Rex frames sit in the
active ready queue, but it does not reserve external compute capacity. Native async permits are the
backpressure mechanism for host jobs.

Use `set_async_call_policy` to wrap futures after they have been admitted. The policy decides where
an admitted future runs; the parallelism controller decides whether the host callback is allowed to
start yet.

```rust,ignore
use futures::FutureExt;
use rex_engine::{AsyncCallPolicy, Builder, EngineError, Module};

let mut builder = Builder::with_prelude(())?;
builder.set_async_call_policy(AsyncCallPolicy::executor_fn(|future| {
    async move {
        tokio::spawn(future)
            .await
            .map_err(|err| EngineError::Internal(format!("async host task failed: {err}")))?
    }
    .boxed()
}));

let mut globals = Module::global();
globals.export_async("inc", |_state, x: i32| async move { Ok(x + 1) })?;
builder.inject_module(globals)?;
```

The executor hook is intentionally generic rather than Tokio-specific. Native applications can use
Tokio or any other Rust executor; wasm applications can keep the inline policy or adapt to browser
task primitives in the host crate.

### Parsing Limits

Parsing enforces a fixed AST-depth cap:

```rust,ignore
use rex::parser::parse;

let program = parse("(((1)))")
    .map_err(|errs| format!("parse error: {errs:?}"))?;
```

## Bridge Rust Types with `#[derive(Rex)]`

The derive:
- implements `RexType`
- implements `RexAdt`
- implements `Rex`
- implements `IntoRex`
- implements `FromRex`
- provides injection helpers through `Rex`
- provides ADT declaration helpers through `RexAdt`
- declares an ADT in the Rex type system
- injects runtime constructors (so Rex can *build* values)
- discovers and registers the full acyclic ADT family needed by the root type

The derive does not implement `RexDefault`; `inject_rex_with_default` is available only when the
type already provides that trait.

Rust doc comments on the derived type, type parameters, enum variants, tuple fields, and named
fields are copied into the generated `AdtDecl`. For a struct, the type's docs also document its
single generated constructor variant.

Fields of type `Vec<T>` are exposed as `List T` and convert to/from Rex lists.
When constructing or updating derived records from Rex code, use list literals
directly for these fields.

Rust `char` is a built-in bridge type corresponding to Rex `Char`; it can be used directly in
injected function signatures and fields of derived types.

That means `MyType::inject_rex(&mut builder)?` is enough for acyclic graphs of derived ADTs. You do
not need to manually register dependencies in topological order. Cyclic ADT families are still not
supported by this registration path.

If a field uses a Rust type that participates in Rex value conversion but is not itself a Rex ADT
(for example a leaf type with manual `RexType` / `IntoRex` / `FromRex` impls), no extra
field annotation is required. Such leaf types inherit the default no-op family collection from
`RexType`, so derived ADTs can contain them without trying to register them as ADTs.

```rust,ignore
use rex::{
    Rex,
    engine::{Builder, EngineError, FromRex, IntoRex, Value},
    typesystem::{RexType, Type},
};

#[derive(Debug, PartialEq)]
struct AtomRef(i32);

impl RexType for AtomRef {
    fn rex_type() -> Type {
        i32::rex_type()
    }
}

impl IntoRex for AtomRef {
    fn into_rex(self) -> Result<Value, EngineError> {
        self.0.into_rex()
    }
}

impl FromRex for AtomRef {
    fn from_rex(value: Value) -> Result<Self, EngineError> {
        Ok(Self(i32::from_rex(value)?))
    }
}

#[derive(Rex, Debug, PartialEq)]
struct Fragment(Vec<AtomRef>);

let mut builder = Builder::with_prelude(())?;
Fragment::inject_rex(&mut builder)?;
```

```rust,ignore
use rex::{
    Rex,
    engine::{Builder, CompileOptions, FromRex},
    parser::parse,
};

#[derive(Rex, Debug, PartialEq)]
enum Maybe<T> {
    Just(T),
    Nothing,
}

let mut builder = Builder::with_prelude(())?;
Maybe::<i32>::inject_rex(&mut builder)?;

let program = parse("Just 1").map_err(|errs| format!("parse error: {errs:?}"))?;
let compiler = builder.build_compiler();
let (compiled, evaluator) = compiler
    .compile_program(&program, CompileOptions::for_module("workflow.main")?)
    .await?;
let _ty = compiled.result_type().clone();
let v = evaluator.run(compiled, Default::default()).await?;
assert_eq!(Maybe::<i32>::from_rex(&v)?, Maybe::Just(1));
```

## Register ADTs Without Derive

If your type metadata is data-driven (for example loaded from JSON), you can build ADTs
without `#[derive(Rex)]`.

- Use `Builder::adt_decl_from_type(...)` to seed an ADT declaration from a Rex type head.
- Add variants with `AdtDecl::add_variant(name, args, docs)`, where `args` is a
  `Vec<AdtArgument>` and `docs` is an `Option<String>`.
- Stage it with `Module::add_adt_decl(...)`, then inject that module with `Builder::inject_module(...)`.

`Module::add_adt_decl(...)` is the low-level single-ADT staging primitive. If you are building
several ADTs manually, prefer batching them in one module with `add_adt_family(...)`.

```rust,ignore
use rex::{
    ast::Symbol,
    engine::{Builder, Module},
    typesystem::{AdtArgument, RexType, Type},
};

let mut builder = Builder::with_prelude(())?;
let mut globals = Module::global();

let mut adt = builder.adt_decl_from_type(&Type::con("PrimitiveEither", 0))?;
adt.add_variant(
    Symbol::intern("Flag"),
    vec![AdtArgument::positional(bool::rex_type())],
    None,
);
adt.add_variant(
    Symbol::intern("Count"),
    vec![AdtArgument::positional(i32::rex_type())],
    None,
);
globals.add_adt_decl(adt)?;
builder.inject_module(globals)?;
```

If you have a Rust type with manual `RexType`/`IntoRex`/`FromRex` impls, implement
`RexAdt` and provide `rex_adt_decl()`. Then `Builder::inject_rex_adt::<T>()` gives manual
types the same registration workflow that `#[derive(Rex)]` exposes as `T::inject_rex(...)`.

If the manual Rust type is itself an ADT, override `RexType::collect_rex_family(...)` and add its
`AdtDecl` there. Leaf types can inherit the default no-op implementation.

```rust,ignore
use rex::{
    ast::Symbol,
    engine::Builder,
    typesystem::{AdtArgument, AdtDecl, RexAdt, RexType, Type, TypeError, TypeVarSupply},
};

struct PrimitiveEither;

impl RexType for PrimitiveEither {
    fn rex_type() -> Type {
        Type::con("PrimitiveEither", 0)
    }

    fn collect_rex_family(out: &mut Vec<AdtDecl>) -> Result<(), TypeError> {
        out.push(<Self as RexAdt>::rex_adt_decl()?);
        Ok(())
    }
}

impl RexAdt for PrimitiveEither {
    fn rex_adt_decl() -> Result<AdtDecl, TypeError> {
        let mut supply = TypeVarSupply::new();
        let mut adt = AdtDecl::new(&Symbol::intern("PrimitiveEither"), &[], &mut supply);
        adt.add_variant(
            Symbol::intern("Flag"),
            vec![AdtArgument::positional(bool::rex_type())],
            None,
        );
        adt.add_variant(
            Symbol::intern("Count"),
            vec![AdtArgument::positional(i32::rex_type())],
            None,
        );
        Ok(adt)
    }
}

let mut builder = Builder::with_prelude(())?;
builder.inject_rex_adt::<PrimitiveEither>()?;
```

## Depth Limits

Some workloads (very deep nesting) can exhaust parser/typechecker recursion depth. Prefer bounded
limits for untrusted code:

- parser AST depth
- `rex_typesystem::TypeSystemLimits::safe_defaults`

## Embedding workflow tool execution

The core `rex` crate does not execute operating-system tools. Hosts using
`rex-workflow` must configure OCI images and either the supplied Docker backend
or an implementation of `OciJobExecutor`. There is no host-process executor.

Provider implementations receive logical CAS inputs and output declarations,
not host paths or Docker arguments. They must enforce the requested platform,
isolation, resource limits, cancellation, result validation, and provenance
contract described in [OCI Executor Protocol](OCI_EXECUTORS.md).
