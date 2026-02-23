# Embedding Rex in Rust

Rex is designed as a small pipeline you can embed at whatever stage you need:

1. `rex-lexer`: source → `Tokens`
2. `rex-parser`: tokens → `Program { decls, expr }`
3. `rex-ts`: HM inference + type classes → `TypedExpr` (plus predicates/type)
4. `rex-engine`: evaluate a `TypedExpr` → `rex_engine::Value`

This document focuses on common embedding patterns.

## Running Untrusted Rex Code (Production Checklist)

This repo provides the *mechanisms* to safely run user-submitted Rex (gas metering, parsing limits,
cancellation). Your production server is responsible for enforcing hard resource limits (process
isolation, wall-clock timeouts, memory limits).

Recommended defaults for untrusted input:

- Always cap parsing nesting depth with `ParserLimits::safe_defaults()` (or stricter).
- Always run with a bounded `GasMeter` for **parse + infer + eval** (and calibrate budgets with real workloads).
- Treat `EngineError::OutOfGas` and `EngineError::Cancelled` as normal user-visible outcomes.
- Run evaluation in an isolation boundary you can hard-kill (separate process/container), with CPU/RSS/time limits.

Evaluation API:

- Evaluation is async and gas-metered via `Engine::eval_with_gas`.

## Evaluate Rex Code

```rust
use rex_engine::Engine;
use rex_lexer::Token;
use rex_parser::Parser;
use rex_util::{GasCosts, GasMeter};

let tokens = Token::tokenize("let x = 1 + 2 in x * 3")?;
let mut parser = Parser::new(tokens);
let program = parser.parse_program(&mut GasMeter::default()).map_err(|errs| format!("{errs:?}"))?;

let mut engine = Engine::with_prelude(())?;
engine.inject_decls(&program.decls)?;
let mut gas = GasMeter::default();
let value = engine
    .eval_with_gas(program.expr.as_ref(), &mut gas)
    .await?;
println!("{value}");
```

## Inject Modules (Embedder Patterns)

This is fully supported in `rex-engine`. You can compose module loading from:

- default resolvers (`std.*`, local filesystem, optional remote feature)
- include roots
- custom resolvers (for DB/object-store/in-memory modules)

### 1) Use Built-In Resolvers

```rust
use rex_engine::Engine;
use rex_util::GasMeter;

let mut engine = Engine::with_prelude(())?;
engine.add_default_resolvers();
engine.add_include_resolver("/opt/my-app/rex-modules")?;

let mut gas = GasMeter::default();
let value = engine
    .eval_module_file("workflows/main.rex", &mut gas)
    .await?;
println!("{value}");
```

Notes:

- local imports are resolved relative to the importing module path.
- include roots are searched after local-relative imports.
- type-only workflows can use `infer_module_file` with the same resolver setup.

### 2) Inject In-Memory Rex Modules

For host-managed modules, add a resolver that maps `module_name` to source text.

```rust
use rex_engine::{Engine, ModuleId, ResolveRequest, ResolvedModule};
use rex_util::GasMeter;
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
        "import acme.math (inc)\ninc 41".to_string(),
    ),
]));

engine.add_resolver("host-map", {
    let modules = modules.clone();
    move |req: ResolveRequest| {
        let Some(source) = modules.get(&req.module_name) else {
            return Ok(None);
        };
        Ok(Some(ResolvedModule {
            id: ModuleId::Virtual(format!("host:{}", req.module_name)),
            source: source.clone(),
        }))
    }
});

let mut gas = GasMeter::default();
let value = engine.eval_module_source(&modules["acme.main"], &mut gas).await?;
println!("{value}");
```

### 3) Host-Provided Rust Functions, Exposed as Modules

This is the common embedder case.

Use `Module` + `Engine::inject_module(...)`:

1. Create a `Module`.
2. Add exports:
   - typed exports with `export` / `export_async`
   - runtime/native exports with `export_native` / `export_native_async`
3. Inject it into the engine.

`export` handlers are fallible and must return `Result<T, EngineError>`. If a handler returns
`Err(...)`, evaluation fails with that engine error.
`export_async` handlers follow the same rule, but return
`Future<Output = Result<T, EngineError>>`.

```rust
use rex_engine::{Engine, Module};
use rex_util::GasMeter;

let mut engine = Engine::with_prelude(())?;
engine.add_default_resolvers();

let mut math = Module::new("acme.math");
math.export("inc", |_state: &(), x: i32| { Ok(x + 1) })?;
math.export_async("double_async", |_state: &(), x: i32| async move { Ok(x * 2) })?;
engine.inject_module(math)?;

let mut gas = GasMeter::default();
let value = engine
    .eval_snippet(
        "import acme.math (inc, double_async as d)\ninc (d 20)",
        &mut gas,
    )
    .await?;
println!("{value}");
```

Internally this generates module declarations and injects host implementations under qualified
module export symbols.

If you need to construct exports separately (for example to build a module from plugin metadata),
you can use:

- `Export::from_handler` / `Export::from_async_handler` (typed handlers)
- `Export::from_native` / `Export::from_native_async` (runtime pointer handlers)

Then add them via `Module::add_export`.

### 3a) Runtime-Defined Signatures (`Pointer` APIs)

If your host determines function signatures/behavior at runtime, use the native module export
APIs and provide an explicit `Scheme` + arity:

- `Module::export_native`
- `Module::export_native_async`

These callbacks receive `&Engine<State>` (not just `&State`), so they can:

- read state via `engine.state`
- allocate new values via `engine.heap()`
- inspect typed call information via the explicit `&Type` / `Type` callback parameter

```rust
use futures::FutureExt;
use rex_engine::{Engine, Module, Pointer};
use rex_ts::{Scheme, Type};

let mut engine = Engine::with_prelude(())?;
engine.add_default_resolvers();

let mut m = Module::new("acme.dynamic");
let scheme = Scheme::new(vec![], vec![], Type::fun(Type::con("i32", 0), Type::con("i32", 0)));

m.export_native("id_ptr", scheme.clone(), 1, |_engine: &Engine<()>, _typ: &Type, args: &[Pointer]| {
    Ok(args[0].clone())
})?;

m.export_native_async("answer_async", Scheme::new(vec![], vec![], Type::con("i32", 0)), 0, |engine: &Engine<()>, _typ: Type, _args: Vec<Pointer>| {
    async move { engine.heap().alloc_i32(42) }.boxed_local()
})?;

engine.inject_module(m)?;
```

`Scheme` and arity must agree. Registration returns an error if the type does not accept the
provided number of arguments.

### 4) Custom Resolver Contract (Advanced)

If you need dynamic/nonstandard module loading behavior, you can still use raw resolvers.

Resolver contract:

- return `Ok(Some(ResolvedModule { ... }))` when you can satisfy the module.
- return `Ok(None)` to let the next resolver try.
- return `Err(...)` for hard failures (invalid module payload, policy violations, etc.).

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
  `&Engine<State>` so
  they can use heap/runtime internals and read `engine.state`.

```rust
use rex_engine::Engine;

#[derive(Clone)]
struct HostState {
    user_id: String,
    roles: Vec<String>,
}

let mut engine: Engine<HostState> = Engine::with_prelude(HostState {
    user_id: "u-123".into(),
    roles: vec!["admin".into(), "editor".into()],
})?;

engine.export("have_role", |state, role: String| {
    Ok(state.roles.iter().any(|r| r == &role))
})?;
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
use rex_lexer::Token;
use rex_parser::Parser;
use rex_ts::TypeSystem;

let tokens = Token::tokenize("map (\\x -> x) [1, 2, 3]")?;
let mut parser = Parser::new(tokens);
let program = parser.parse_program(&mut GasMeter::default()).map_err(|errs| format!("{errs:?}"))?;

let mut ts = TypeSystem::with_prelude()?;
for decl in &program.decls {
    match decl {
        rex_ast::expr::Decl::Type(d) => ts.inject_type_decl(d)?,
        rex_ast::expr::Decl::Class(d) => ts.inject_class_decl(d)?,
        rex_ast::expr::Decl::Instance(d) => {
            ts.inject_instance_decl(d)?;
        }
        rex_ast::expr::Decl::Fn(d) => ts.inject_fn_decl(d)?,
    }
}

let (preds, ty) = ts.infer(program.expr.as_ref())?;
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
use rex_lexer::Token;
use rex_parser::Parser;
use rex_ts::TypeSystem;

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

let mut ts = TypeSystem::with_prelude()?;
for decl in &program.decls {
    match decl {
        rex_ast::expr::Decl::Type(d) => ts.inject_type_decl(d)?,
        rex_ast::expr::Decl::Class(d) => ts.inject_class_decl(d)?,
        rex_ast::expr::Decl::Instance(d) => {
            ts.inject_instance_decl(d)?;
        }
        rex_ast::expr::Decl::Fn(d) => ts.inject_fn_decl(d)?,
    }
}

let (_preds, ty) = ts.infer(program.expr.as_ref())?;
assert_eq!(ty.to_string(), "i32");
```

### Evaluate: Inject Decls into `Engine`

```rust
use rex_engine::{Engine, EngineError};
use rex_lexer::Token;
use rex_parser::Parser;
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
engine.inject_decls(&program.decls)?;
let mut gas = GasMeter::default();
let value = engine
    .eval_with_gas(program.expr.as_ref(), &mut gas)
    .await?;
println!("{value}");
```

## Inject Native Values and Functions

`rex-engine` is the boundary where Rust provides implementations for Rex values.

For host-provided *modules*, prefer `Module` + `inject_module` (above). The direct injection APIs
below are still useful for global values/functions that are not grouped under a module namespace.

```rust
use rex_engine::Engine;

let mut engine = Engine::with_prelude(())?;
engine.export_value("answer", 42i32)?;
engine.export("inc", |_state, x: i32| { Ok(x + 1) })?;
```

### Integer Literal Overloading with Host Natives

Integer literals are overloaded (`Integral a`) and can specialize at call sites. This works for
direct calls, `let` bindings, and lambda wrappers:

```rust
use rex_engine::Engine;
use rex_util::GasMeter;

let mut engine = Engine::with_prelude(())?;
engine.export("num_u8", |_state: &(), x: u8| Ok(format!("{x}:u8")))?;
engine.export("num_i64", |_state: &(), x: i64| Ok(format!("{x}:i64")))?;

for code in [
    "num_u8 4",
    "let x = 4 in num_u8 x",
    "let f = \\x -> num_i64 x in f 4",
] {
    let tokens = rex_lexer::Token::tokenize(code)?;
    let mut parser = rex_parser::Parser::new(tokens);
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

If your host functions are async, inject them with `export_async` and evaluate with
`Engine::eval_with_gas`.

```rust
use rex_engine::Engine;
use rex_util::{GasCosts, GasMeter};

let mut engine = Engine::with_prelude(())?;
engine.export_async("inc", |_state, x: i32| async move { Ok(x + 1) })?;

let tokens = rex_lexer::Token::tokenize("inc 1")?;
let mut parser = rex_parser::Parser::new(tokens);
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
use rex_engine::{CancellationToken, Engine, EngineError};
use rex_ts::{Scheme, Type};
use rex_util::{GasCosts, GasMeter};

let tokens = rex_lexer::Token::tokenize("stall")?;
let mut parser = rex_parser::Parser::new(tokens);
let expr = parser
    .parse_program(&mut GasMeter::default())
    .map_err(|errs| format!("parse error: {errs:?}"))?
    .expr;

let mut engine = Engine::with_prelude(())?;
let scheme = Scheme::new(vec![], vec![], Type::con("i32", 0));
engine.export_native_async_cancellable(
    "stall",
    scheme,
    0,
    |engine, token: CancellationToken, _, _args| {
        async move {
            token.cancelled().await;
            engine.heap().alloc_i32(0)
        }
        .boxed_local()
    },
)?;

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
- `TypeSystem::infer_with_gas` / `infer_typed_with_gas`
- `Engine::eval_with_gas`

### Parsing Limits

For untrusted input, you can cap syntactic nesting depth during parsing:

```rust
use rex_parser::{Parser, ParserLimits};

let mut parser = Parser::new(rex_lexer::Token::tokenize("(((1)))")?);
parser.set_limits(ParserLimits::safe_defaults());
let program = parser.parse_program(&mut GasMeter::default())?;
```

## Bridge Rust Types with `#[derive(Rex)]`

The derive:
- declares an ADT in the Rex type system
- injects runtime constructors (so Rex can *build* values)
- implements `FromPointer`/`IntoPointer` for converting Rust ↔ Rex

```rust
use rex::{Engine, FromPointer, GasMeter, Parser, Token};
use rex_proc_macro::Rex;

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
- Register with `Engine::inject_adt(...)`.

```rust
use rex::{Engine, RexType, Type, sym};

let mut engine = Engine::with_prelude(())?;

let mut adt = engine.adt_decl_from_type(&Type::con("PrimitiveEither", 0))?;
adt.add_variant(sym("Flag"), vec![bool::rex_type()]);
adt.add_variant(sym("Count"), vec![i32::rex_type()]);
engine.inject_adt(adt)?;
```

If you have a Rust type with manual `RexType`/`IntoPointer`/`FromPointer` impls, implement
`RexAdt` and provide `rex_adt_decl(...)`. Then `RexAdt::inject_rex(...)` gives the same
registration workflow as derived types.

```rust
use rex::{AdtDecl, Engine, EngineError, RexAdt, RexType, Type, sym};

struct PrimitiveEither;

impl RexType for PrimitiveEither {
    fn rex_type() -> Type {
        Type::con("PrimitiveEither", 0)
    }
}

impl RexAdt for PrimitiveEither {
    fn rex_adt_decl<State: Clone + Send + Sync + 'static>(
        engine: &mut Engine<State>,
    ) -> Result<AdtDecl, EngineError> {
        let mut adt = engine.adt_decl_from_type(&Self::rex_type())?;
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

- `rex_parser::ParserLimits::safe_defaults`
- `rex_ts::TypeSystemLimits::safe_defaults`
