# Embedding Rex in Rust

Rex is designed as a small pipeline you can embed at whatever stage you need:

1. `rexlang-lexer`: source → `Tokens`
2. `rexlang-parser`: tokens → `Program { decls, expr }`
3. `rexlang-typesystem`: HM inference + type classes → `TypedExpr` (plus predicates/type)
4. `rexlang-engine`: evaluate a `TypedExpr` → `rexlang_engine::Value`

This document focuses on common embedding patterns.

## Running Untrusted Rex Code (Production Checklist)

This repo provides the *mechanisms* to safely run user-submitted Rex (gas metering, parsing limits,
cancellation). Your production server is responsible for enforcing hard resource limits (process
isolation, wall-clock timeouts, memory limits).

Recommended defaults for untrusted input:

- Always cap parsing nesting depth with `ParserLimits::safe_defaults()` (or stricter).
- Always run with a bounded `GasMeter` for **parse + infer + eval** (and calibrate budgets with real workloads).
- Treat `EvalError::OutOfGas` / `EvalError::Cancelled` and the same variants wrapped inside
  `ExecutionError` as normal user-visible outcomes.
- Run evaluation in an isolation boundary you can hard-kill (separate process/container), with CPU/RSS/time limits.

Evaluation API:

- Evaluation is async and gas-metered via `Engine::eval_with_gas`.

## Compile Then Run

`rexlang-engine` now has an explicit preparation boundary:

- `Engine` builds the host environment.
- `Compiler` prepares user code into a `CompiledProgram`.
- `Evaluator` runs prepared code against a `RuntimeEnv`.

The current implementation still keeps engine-backed state internally, but the public API now
separates compile-time and runtime phases.

```rust
use rexlang::{Engine, GasMeter};

let engine = Engine::with_prelude(())?;
let mut compiler = rexlang::Compiler::new(engine.clone());
let runtime = rexlang::RuntimeEnv::new(engine.clone());
let mut evaluator = rexlang::Evaluator::new(runtime.clone());
let mut gas = GasMeter::default();

let program = compiler.compile_snippet("let x = 1 + 2 in x * 3", &mut gas)?;
runtime.validate(&program)?;
let value = evaluator.run(&program, &mut gas).await?;
assert_eq!(program.result_type().to_string(), "i32");
```

What "compiled" means in the current design:

- parsing, import rewriting, declaration injection, and typechecking have already happened
- `CompiledProgram` carries a typed expression plus the environment snapshot needed to run it
- runtime-linked requirements are still explicit, and `RuntimeEnv::validate` checks them before execution
- internally, `RuntimeEnv` keeps a runtime snapshot for execution and separate engine-backed loader
  state for convenience entry points like library loading and REPL-style session sync
- `CompiledProgram::link_contract()` and `RuntimeEnv::capabilities()` now make the runtime link
  contract explicit, including the current ABI version and the required callable shapes
- `CompiledProgram::storage_boundary()` and `RuntimeEnv::storage_boundary()` mark both values as
  process-local, not serializable artifacts

What is currently captured versus linked:

- Rex declarations that are part of the prepared program are captured into the compiled env snapshot
- host-provided exports registered through `export`, `export_async`, `export_native`,
  `export_native_async`, or `export_value` are runtime-linked and must be available in the
  `RuntimeEnv`
- typeclass method bindings are also runtime-linked through the `RuntimeEnv`

That means `CompiledProgram` is engine-independent at the API level, but it is not a fully
self-contained serialized artifact. It is best thought of as a prepared program plus explicit
runtime link requirements.

Phase-specific errors:

- `Compiler` APIs return `CompileError`
- `Evaluator::run` returns `EvalError`
- convenience helpers like `eval_snippet` return `ExecutionError` because they still do both
  phases

`Evaluator` is a stateful session. Reusing one evaluator preserves the compiler/runtime snapshot it
has accumulated so far, which matters for REPL-style workflows. Constructing a fresh evaluator from
the same engine starts a fresh session.

If you want an explicit preflight before running:

```rust
let runtime = rexlang::RuntimeEnv::new(engine.clone());
runtime.validate(&program)?;

let mut evaluator = rexlang::Evaluator::new(runtime);
let value = evaluator.run(&program, &mut gas).await?;
```

The convenience helpers such as `Evaluator::eval`, `eval_snippet`, `eval_snippet_at`, and
`eval_repl_program` now route through the same prepare/validate/run boundary internally. They are
still sugar, but they no longer use a separate execution path.

## Evaluate Rex Code Directly

```rust
use rexlang::{Engine, GasMeter, Library, Parser, Token};

let tokens = Token::tokenize("let x = 1 + 2 in x * 3")?;
let mut parser = Parser::new(tokens);
let program = parser.parse_program(&mut GasMeter::default()).map_err(|errs| format!("{errs:?}"))?;

let mut engine = Engine::with_prelude(())?;
let mut globals = Library::global();
globals.add_decls(program.decls.clone());
engine.inject_library(globals)?;
let mut gas = GasMeter::default();
let mut compiler = rexlang::Compiler::new(engine.clone());
let program = compiler.compile_expr(program.expr.as_ref())?;
let mut evaluator =
    rexlang::Evaluator::new_with_compiler(rexlang::RuntimeEnv::new(engine.clone()), rexlang::Compiler::new(engine.clone()));
let value = evaluator.run(&program, &mut gas).await?;
println!("{value}");
```

Library sources loaded via resolvers (and library files on disk) must be declaration-only. To run an expression, use snippet/repl entry points.
Qualified alias members used in type/class positions (annotations, `where` constraints, instance
headers, superclass clauses) are validated against library exports during library processing; missing
exports fail early with library errors.

## Engine Initialization and Default Imports

`Engine::with_prelude(state)` is shorthand for `Engine::with_options(state, EngineOptions::default())`.

- Prelude is enabled by default.
- `Prelude` is default-imported.
- Default imports are weak: they fill missing names, but never override local declarations
  or explicit imports.

If you want full control:

```rust
use rexlang::{Engine, EngineOptions, PreludeMode};

let mut engine = Engine::with_options(
    (),
    EngineOptions {
        prelude: PreludeMode::Disabled,
        default_imports: vec![],
    },
)?;
```

## Inject Libraries (Embedder Patterns)

This is fully supported in `rexlang-engine`. You can compose library loading from:

- default resolvers (`std.*`, local filesystem, optional remote feature)
- include roots
- custom resolvers (for DB/object-store/in-memory modules)

### 1) Use Built-In Resolvers

```rust
use rexlang::{Engine, GasMeter};

let mut engine = Engine::with_prelude(())?;
engine.add_default_resolvers();
engine.add_include_resolver("/opt/my-app/rex-modules")?;

let mut gas = GasMeter::default();
let value = engine
    .eval_library_file("workflows/main.rex", &mut gas)
    .await?;
println!("{value}");
```

Notes:

- local imports are resolved relative to the importing library path.
- include roots are searched after local-relative imports.
- type-only workflows can use `infer_library_file` with the same resolver setup.
- compile-only workflows can use `Compiler::compile_library_file` with the same resolver setup.
- import clauses (`(*)` / item lists) import exported names into unqualified scope.
- unqualified imports are context-sensitive: expression positions use values, type positions use
  types, and class/constraint positions use classes.
- library aliases (`import x as M`) provide qualified access to exported values, types, and classes.
- importing a name only brings in the facets that actually exist under that name.

### 2) Inject In-Memory Rex Modules

For host-managed modules, add a resolver that maps `library_name` to source text.

```rust
use rexlang_engine::{LibraryId, ResolveRequest, ResolvedLibrary};
use rexlang::{Engine, GasMeter};
use std::collections::HashMap;
use std::sync::Arc;

let mut engine = Engine::with_prelude(())?;
engine.add_default_resolvers();

let modules = Arc::new(HashMap::from([
    (
        "acme.math".to_string(),
        "pub fn inc : i32 -> i32 = \\x -> x + 1".to_string(),
    ),
    (
        "acme.main".to_string(),
        "import acme.math (inc)\npub fn main : i32 = inc 41".to_string(),
    ),
]));

engine.add_resolver("host-map", {
    let modules = modules.clone();
    move |req: ResolveRequest| {
        let Some(source) = modules.get(&req.library_name) else {
            return Ok(None);
        };
        Ok(Some(ResolvedLibrary {
            id: LibraryId::Virtual(format!("host:{}", req.library_name)),
            content: rexlang::ResolvedLibraryContent::Source(source.clone()),
        }))
    }
});

let mut gas = GasMeter::default();
let value = engine
    .eval_snippet("import acme.main (main)\nmain", &mut gas)
    .await?;
println!("{value}");
```

### 3) Host-Provided Rust Functions, Exposed as Modules

This is the common embedder case.

Use `Library` + `Engine::inject_library(...)`:

1. Create a `Library`.
2. Add exports:
   - typed exports with `export` / `export_async`
   - runtime/native exports with `export_native` / `export_native_async`
   - optional raw Rex declarations with `add_raw_declaration` (for example `pub type ...`)
   - optional structured declarations with `add_rex_adt` / `add_adt_decl`
3. Inject it into the engine.

`Library::add_rex_adt::<T>()` now stages the full acyclic ADT family reachable from `T`.
This is driven by `RexType::collect_rex_family(...)`: ADT types contribute declarations there,
while leaf Rex types inherit a no-op default. For example, if `Label` contains a `Side`, staging
`Label` is enough; you do not need to stage `Side` separately. Cyclic ADT families are still
rejected.

`Library` also exposes its staged `raw_declarations`, `structured_decls`, and `exports` vectors
directly. That is useful if you want to inspect, transform, or assemble a library in multiple
passes before calling `Engine::inject_library`.

`export` handlers are fallible and must return `Result<T, EngineError>`. If a handler returns
`Err(...)`, evaluation fails with that engine error.
`export_async` handlers follow the same rule, but return
`Future<Output = Result<T, EngineError>>`.

```rust
use rexlang_engine::{Engine, Library};
use rex_util::GasMeter;

let mut engine = Engine::with_prelude(())?;
engine.add_default_resolvers();

let mut math = Library::new("acme.math");
math.export("inc", |_state: &(), x: i32| { Ok(x + 1) })?;
math.export_async("double_async", |_state: &(), x: i32| async move { Ok(x * 2) })?;
engine.inject_library(math)?;

let mut gas = GasMeter::default();
let value = engine
    .eval_snippet(
        "import acme.math (inc, double_async as d)\ninc (d 20)",
        &mut gas,
    )
    .await?;
println!("{value}");
```

You can declare ADTs directly inside an injected host library:

```rust
use rexlang_engine::{Engine, Library};

let mut engine = Engine::with_prelude(())?;
engine.add_default_resolvers();

let mut m = Library::new("acme.status");
m.add_raw_declaration("pub type Status = Ready | Failed string")?;
engine.inject_library(m)?;
```

Then Rex code can import and use those names from the library:

```rex
import acme.status (Status, Failed)

let fail: string -> Status = \msg -> Failed msg in
match (fail "boom")
  when Failed msg -> length msg
  when _ -> 0
```

`Status` is used here in type position, while `Failed` is used in expression/pattern positions.
They are imported through the same name-based mechanism.

Internally this generates library declarations and injects host implementations under qualified
library export symbols.

If you need to construct exports separately (for example to build a library from plugin metadata),
you can use:

- `Export::from_handler` / `Export::from_async_handler` (typed handlers)
- `Export::from_native` / `Export::from_native_async` (runtime pointer handlers)

Then add them via `Library::add_export`, or push them into `Library::exports` directly if you are
assembling the library programmatically.

This example shows how to use Rust enums and structs as Rex-facing types with ADTs declared inside
the library itself. The host function accepts a Rust `Label` (containing a Rust `Side` enum), and
Rex code calls it through `sample.render_label`.

Example:

```rust
use rexlang::{Engine, EngineError, Library, Rex};
use rex_util::GasMeter;

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

let mut engine = Engine::with_prelude(())?;
engine.add_default_resolvers();

let mut m = Library::new("sample");
m.add_rex_adt::<Label>()?;
m.export("render_label", |_state: &(), label: Label| {
    Ok::<String, EngineError>(render_label(label))
})?;
engine.inject_library(m)?;

let mut gas = GasMeter::default();
let value = engine
    .eval_snippet(
        r#"
        import sample (Label, Left, Right, render_label)
        (
            render_label (Label { text = "left", side = Left }),
            render_label (Label { text = "right", side = Right })
        )
        "#,
        &mut gas,
    )
    .await?;
println!("{value}"); // ("left        ", "       right")
```

In that example:

- `Label` is imported once and then used as both a type name and a constructor value.
- `Left` and `Right` are imported as constructor values.
- `render_label` is imported as a value.

### 3a) Runtime-Defined Signatures (`Pointer` APIs)

If your host determines function signatures/behavior at runtime, use the native library export
APIs and provide an explicit `Scheme` + arity:

- `Library::export_native`
- `Library::export_native_async`

These callbacks receive `EvaluatorRef<'_, State>` (not just `&State`), so they can:

- read state via `engine.state`
- allocate new values via `engine.heap`
- inspect typed call information via the explicit `&Type` / `Type` callback parameter

```rust
use futures::FutureExt;
use rexlang_engine::{Engine, EvaluatorRef, Library, Pointer};
use rexlang::{BuiltinTypeId, Scheme, Type};

let mut engine = Engine::with_prelude(())?;
engine.add_default_resolvers();

let mut m = Library::new("acme.dynamic");
let scheme = Scheme::new(vec![], vec![], Type::fun(Type::builtin(BuiltinTypeId::I32), Type::builtin(BuiltinTypeId::I32)));

m.export_native("id_ptr", scheme.clone(), 1, |_engine: EvaluatorRef<'_, ()>, _typ: &Type, args: &[Pointer]| {
    Ok(args[0].clone())
})?;

m.export_native_async("answer_async", Scheme::new(vec![], vec![], Type::builtin(BuiltinTypeId::I32)), 0, |engine: EvaluatorRef<'_, ()>, _typ: Type, _args: Vec<Pointer>| {
    async move { engine.heap.alloc_i32(42) }.boxed_local()
})?;

engine.inject_library(m)?;
```

`Scheme` and arity must agree. Registration returns an error if the type does not accept the
provided number of arguments.

### 4) Custom Resolver Contract (Advanced)

If you need dynamic/nonstandard library loading behavior, you can still use raw resolvers.

Resolver contract:

- return `Ok(Some(ResolvedLibrary { ... }))` when you can satisfy the library.
- return `Ok(None)` to let the next resolver try.
- return `Err(...)` for hard failures (invalid library payload, policy violations, etc.).

`ResolvedLibrary` can carry either `ResolvedLibraryContent::Source(...)` for real Rex source or
`ResolvedLibraryContent::Program(...)` for preconstructed structured modules.

### 5) Snippets That Import Relative Modules

If you evaluate ad-hoc Rex snippets that contain imports, use `eval_snippet_at` (or
`infer_snippet_at`) to provide an importer path anchor:

```rust
let mut gas = rex_util::GasMeter::default();
let value = engine
    .eval_snippet_at("import foo.bar as Bar\nBar.add 1 2", "/tmp/workflow/_snippet.rex", &mut gas)
    .await?;
```

## Engine State

`Engine` is generic over host state: `Engine<State>`, where `State: Clone + Sync + 'static`.
The state is stored as `engine.state: Arc<State>` and is shared across all injected functions.

- Use `Engine::with_prelude(())?` if you do not need host state.
- If you do, pass your state struct into `Engine::new(state)` or `Engine::with_prelude(state)`.
- `export` / `export_async` callbacks receive `&State` as their first parameter.
- Pointer-level APIs (`export_native*`) receive
  `EvaluatorRef<'_, State>` so
  they can use heap/runtime internals and read `engine.state`.

```rust
use rexlang_engine::Engine;

#[derive(Clone)]
struct HostState {
    user_id: String,
    roles: Vec<String>,
}

let mut engine: Engine<HostState> = Engine::with_prelude(HostState {
    user_id: "u-123".into(),
    roles: vec!["admin".into(), "editor".into()],
})?;

let mut globals = Library::global();
globals.export("have_role", |state, role: String| {
    Ok(state.roles.iter().any(|r| r == &role))
})?;
engine.inject_library(globals)?;
```

## Array/List Interop at Host Boundaries

Rex keeps both `List a` and `Array a` because they serve different goals:

- `List a` is ergonomic for user-authored functional code and pattern matching.
- `Array a` is the host-facing contiguous representation (for example `Vec<u8>`
  from filesystem reads).

At host function call sites, Rex performs a narrow implicit coercion from
`List a` to `Array a` in argument position. This means users can pass list
literals to host functions that accept `Vec<T>` without writing conversions.

```rex
accept_bytes [1, 2, 3]
```

where `accept_bytes` is exported from Rust with a `Vec<u8>` parameter.

For the opposite direction, Rex exposes explicit helpers:

- `to_list : Array a -> List a`
- `to_array : List a -> Array a`

### Why `to_list` Is Explicit (Not Implicit)

`Array -> List` conversion is intentionally explicit to keep runtime costs
predictable in user code. Converting an array into a list allocates a new
linked structure and changes performance characteristics for downstream
operations.

If this conversion were implicit everywhere, the compiler could silently insert
it in places where users do not expect allocation or complexity changes (for
example inside control-flow joins, nested expressions, or polymorphic code).
That would make performance harder to reason about and make type errors less
transparent.

By requiring `to_list` explicitly, we keep intent and cost visible at the exact
program point where representation changes. This preserves ergonomics while
avoiding hidden work:

```rex
match (to_list bytes) with
    when Cons head _ -> head
    when Empty -> 0
```

## Typecheck Without Evaluating

```rust
use rexlang::{Parser, Token, TypeSystem, infer};

let tokens = Token::tokenize("map (\\x -> x) [1, 2, 3]")?;
let mut parser = Parser::new(tokens);
let program = parser.parse_program(&mut GasMeter::default()).map_err(|errs| format!("{errs:?}"))?;

let mut ts = TypeSystem::new_with_prelude()?;
for decl in &program.decls {
    match decl {
        rex_ast::expr::Decl::Type(d) => ts.register_type_decl(d)?,
        rex_ast::expr::Decl::Class(d) => ts.register_class_decl(d)?,
        rex_ast::expr::Decl::Instance(d) => {
            ts.register_instance_decl(d)?;
        }
        rex_ast::expr::Decl::Fn(d) => ts.register_fn_decls(std::slice::from_ref(d))?,
    }
}

let (preds, ty) = infer(&mut ts, program.expr.as_ref())?;
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

1. Parse Rex source into `Program { decls, expr }`.
2. Inject `Decl::Class` / `Decl::Instance` into the type system (if you’re typechecking without running).
3. Inject all decls into the engine (if you’re running), so instance method bodies are available at runtime.

### Typecheck: Inject Class/Instance Decls into `TypeSystem`

```rust
use rexlang::{Parser, Token, TypeSystem, infer};

let code = r#"
class Size a
    size : a -> i32

instance Size (List t)
    size = \xs ->
        match xs
            when Empty -> 0
            when Cons _ rest -> 1 + size rest

size [1, 2, 3]
"#;

let tokens = Token::tokenize(code)?;
let mut parser = Parser::new(tokens);
let program = parser.parse_program(&mut GasMeter::default()).map_err(|errs| format!("{errs:?}"))?;

let mut ts = TypeSystem::new_with_prelude()?;
for decl in &program.decls {
    match decl {
        rex_ast::expr::Decl::Type(d) => ts.register_type_decl(d)?,
        rex_ast::expr::Decl::Class(d) => ts.register_class_decl(d)?,
        rex_ast::expr::Decl::Instance(d) => {
            ts.register_instance_decl(d)?;
        }
        rex_ast::expr::Decl::Fn(d) => ts.register_fn_decls(std::slice::from_ref(d))?,
    }
}

let (_preds, ty) = infer(&mut ts, program.expr.as_ref())?;
assert_eq!(ty.to_string(), "i32");
```

### Evaluate: Inject Decls into `Engine`

```rust
use rexlang_engine::{Engine, EngineError};
use rexlang::{Parser, Token};
use rex_util::{GasCosts, GasMeter};

let code = r#"
class Size a
    size : a -> i32

instance Size (List t)
    size = \xs ->
        match xs
            when Empty -> 0
            when Cons _ rest -> 1 + size rest

(size [1, 2, 3], size [])
"#;

let tokens = Token::tokenize(code)?;
let mut parser = Parser::new(tokens);
let program = parser.parse_program(&mut GasMeter::default()).map_err(|errs| format!("{errs:?}"))?;

let mut engine = Engine::with_prelude(())?;
let mut globals = Library::global();
globals.add_decls(program.decls.clone());
engine.inject_library(globals)?;
let mut gas = GasMeter::default();
let value = engine
    .eval_with_gas(program.expr.as_ref(), &mut gas)
    .await?;
println!("{value}");
```

## Inject Native Values and Functions

`rexlang-engine` is the boundary where Rust provides implementations for Rex values.

For host-provided *modules*, prefer `Library` + `inject_library` (above). For root-scope values
or functions, use `Library::global()` and inject that staged library into the engine.

```rust
use rexlang_engine::{Engine, Library};

let mut engine = Engine::with_prelude(())?;
let mut globals = Library::global();
globals.export_value("answer", 42i32)?;
globals.export("inc", |_state, x: i32| { Ok(x + 1) })?;
engine.inject_library(globals)?;
```

### Integer Literal Overloading with Host Natives

Integer literals are overloaded (`Integral a`) and can specialize at call sites. This works for
direct calls, `let` bindings, and lambda wrappers:

```rust
use rexlang_engine::{Engine, Library};
use rex_util::GasMeter;

let mut engine = Engine::with_prelude(())?;
let mut globals = Library::global();
globals.export("num_u8", |_state: &(), x: u8| Ok(format!("{x}:u8")))?;
globals.export("num_i64", |_state: &(), x: i64| Ok(format!("{x}:i64")))?;
engine.inject_library(globals)?;

for code in [
    "num_u8 4",
    "let x = 4 in num_u8 x",
    "let f = \\x -> num_i64 x in f 4",
] {
    let tokens = Token::tokenize(code)?;
    let mut parser = Parser::new(tokens);
    let program = parser
        .parse_program(&mut GasMeter::default())
        .map_err(|errs| format!("parse error: {errs:?}"))?;
    let mut gas = GasMeter::default();
    let value = engine.eval_with_gas(program.expr.as_ref(), &mut gas).await?;
    println!("{value}");
}
```

Negative literals specialize only to signed numeric types. For example, `num_i32 (-3)` is valid,
while `num_u32 (-3)` is a type error.

### Async Natives

If your host functions are async, stage them in a library with `export_async` and evaluate with
`Engine::eval_with_gas`.

```rust
use rexlang_engine::{Engine, Library};
use rex_util::{GasCosts, GasMeter};

let mut engine = Engine::with_prelude(())?;
let mut globals = Library::global();
globals.export_async("inc", |_state, x: i32| async move { Ok(x + 1) })?;
engine.inject_library(globals)?;

let tokens = Token::tokenize("inc 1")?;
let mut parser = Parser::new(tokens);
let program = parser
    .parse_program(&mut GasMeter::default())
    .map_err(|errs| format!("parse error: {errs:?}"))?;
let mut gas = GasMeter::default();
let v = engine.eval_with_gas(program.expr.as_ref(), &mut gas).await?;
println!("{v}");
```

### Cancellation

Async natives can be cancelled. Cancellation is cooperative: you get a `CancellationToken` and
trigger it from another thread/task, and the engine will stop evaluation with `EngineError::Cancelled`.

```rust
use futures::FutureExt;
use rexlang_engine::{CancellationToken, Engine, EngineError, Library};
use rexlang::{BuiltinTypeId, Scheme, Type};
use rex_util::{GasCosts, GasMeter};

let tokens = Token::tokenize("stall")?;
let mut parser = Parser::new(tokens);
let expr = parser
    .parse_program(&mut GasMeter::default())
    .map_err(|errs| format!("parse error: {errs:?}"))?
    .expr;

let mut engine = Engine::with_prelude(())?;
let scheme = Scheme::new(vec![], vec![], Type::builtin(BuiltinTypeId::I32));
let mut globals = Library::global();
globals.export_native_async_cancellable("stall", scheme, 0, |engine, token: CancellationToken, _, _args| {
        async move {
            token.cancelled().await;
            engine.heap.alloc_i32(0)
        }
        .boxed_local()
    })?;
engine.inject_library(globals)?;

let token = engine.cancellation_token();
std::thread::spawn(move || {
    std::thread::sleep(std::time::Duration::from_millis(10));
    token.cancel();
});
let mut gas = GasMeter::default();
let res = engine.eval_with_gas(expr.as_ref(), &mut gas).await;
assert!(matches!(res, Err(EngineError::Cancelled)));
```

### Gas Metering

To defend against untrusted/large programs, you can run the pipeline with a gas budget:

- `Parser::parse_program`
- `infer_with_gas(&mut ts, ...)` / `infer_typed_with_gas(&mut ts, ...)`
- `Engine::eval_with_gas`

### Parsing Limits

For untrusted input, you can cap syntactic nesting depth during parsing:

```rust
use rexlang::{Parser, ParserLimits, Token};

let mut parser = Parser::new(Token::tokenize("(((1)))")?);
parser.set_limits(ParserLimits::safe_defaults());
let program = parser.parse_program(&mut GasMeter::default())?;
```

## Bridge Rust Types with `#[derive(Rex)]`

The derive:
- declares an ADT in the Rex type system
- injects runtime constructors (so Rex can *build* values)
- discovers and registers the full acyclic ADT family needed by the root type
- implements `FromPointer`/`IntoPointer` for converting Rust ↔ Rex

That means `MyType::inject_rex(&mut engine)?` is enough for acyclic graphs of derived ADTs. You do
not need to manually register dependencies in topological order. Cyclic ADT families are still not
supported by this registration path.

If a field uses a Rust type that participates in Rex value conversion but is not itself a Rex ADT
(for example a leaf type with manual `RexType` / `IntoPointer` / `FromPointer` impls), no extra
field annotation is required. Such leaf types inherit the default no-op family collection from
`RexType`, so derived ADTs can contain them without trying to register them as ADTs.

```rust
use rexlang::{Engine, FromPointer, IntoPointer, Pointer, Rex, RexType, Type};

#[derive(Debug, PartialEq)]
struct AtomRef(i32);

impl RexType for AtomRef {
    fn rex_type() -> Type {
        i32::rex_type()
    }
}

impl IntoPointer for AtomRef {
    fn into_pointer(self, heap: &rexlang::Heap) -> Result<Pointer, rexlang::EngineError> {
        self.0.into_pointer(heap)
    }
}

impl FromPointer for AtomRef {
    fn from_pointer(heap: &rexlang::Heap, pointer: &Pointer) -> Result<Self, rexlang::EngineError> {
        Ok(Self(i32::from_pointer(heap, pointer)?))
    }
}

#[derive(Rex, Debug, PartialEq)]
struct Fragment(Vec<AtomRef>);

let mut engine = Engine::with_prelude(())?;
Fragment::inject_rex(&mut engine)?;
```

```rust
use rexlang::{Engine, FromPointer, GasMeter, Parser, Token, Rex};

#[derive(Rex, Debug, PartialEq)]
enum Maybe<T> {
    Just(T),
    Nothing,
}

let mut engine = Engine::with_prelude(())?;
Maybe::<i32>::inject_rex(&mut engine)?;

let expr = Parser::new(Token::tokenize("Just 1")?)
    .parse_program(&mut GasMeter::default())
    .map_err(|errs| format!("parse error: {errs:?}"))?
    .expr;
let mut gas = GasMeter::default();
let (v, _ty) = engine.eval_with_gas(expr.as_ref(), &mut gas).await?;
assert_eq!(Maybe::<i32>::from_pointer(&engine.heap, &v)?, Maybe::Just(1));
```

## Register ADTs Without Derive

If your type metadata is data-driven (for example loaded from JSON), you can build ADTs
without `#[derive(Rex)]`.

- Use `Engine::adt_decl_from_type(...)` to seed an ADT declaration from a Rex type head.
- Add variants with `AdtDecl::add_variant(...)`.
- Stage it with `Library::add_adt_decl(...)`, then inject that library with `Engine::inject_library(...)`.

`Library::add_adt_decl(...)` is the low-level single-ADT staging primitive. If you are building
several ADTs manually, prefer batching them in one library with `add_adt_family(...)`.

```rust
use rexlang::{Engine, Library, RexType, Type, sym};

let mut engine = Engine::with_prelude(())?;
let mut globals = Library::global();

let mut adt = engine.adt_decl_from_type(&Type::con("PrimitiveEither", 0))?;
adt.add_variant(sym("Flag"), vec![bool::rex_type()]);
adt.add_variant(sym("Count"), vec![i32::rex_type()]);
globals.add_adt_decl(adt)?;
engine.inject_library(globals)?;
```

If you have a Rust type with manual `RexType`/`IntoPointer`/`FromPointer` impls, implement
`RexAdt` and provide `rex_adt_decl()`. Then `RexAdt::inject_rex(...)` gives the same
registration workflow as derived types.

If the manual Rust type is itself an ADT, override `RexType::collect_rex_family(...)` and add its
`AdtDecl` there. Leaf types can inherit the default no-op implementation.

```rust
use rexlang::{AdtDecl, Engine, EngineError, RexAdt, RexType, Type, TypeVarSupply, sym};

struct PrimitiveEither;

impl RexType for PrimitiveEither {
    fn rex_type() -> Type {
        Type::con("PrimitiveEither", 0)
    }

    fn collect_rex_family(out: &mut Vec<AdtDecl>) -> Result<(), EngineError> {
        out.push(<Self as RexAdt>::rex_adt_decl()?);
        Ok(())
    }
}

impl RexAdt for PrimitiveEither {
    fn rex_adt_decl() -> Result<AdtDecl, EngineError> {
        let mut supply = TypeVarSupply::new();
        let mut adt = AdtDecl::new(&sym("PrimitiveEither"), &[], &mut supply);
        adt.add_variant(sym("Flag"), vec![bool::rex_type()]);
        adt.add_variant(sym("Count"), vec![i32::rex_type()]);
        Ok(adt)
    }
}

let mut engine = Engine::with_prelude(())?;
PrimitiveEither::inject_rex(&mut engine)?;
```

## Depth Limits

Some workloads (very deep nesting) can exhaust parser/typechecker recursion depth. Prefer bounded
limits for untrusted code:

- `rexlang::ParserLimits::safe_defaults`
- `rexlang_typesystem::TypeSystemLimits::safe_defaults`
