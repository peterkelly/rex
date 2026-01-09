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

Async integration footgun:

- `Engine::eval()` will block on async natives internally. If your server is async, prefer
  `Engine::eval_async*` end-to-end, or run `Engine::eval()` on a dedicated blocking thread. Do not
  call `Engine::eval()` from inside another executor context that cannot be nested.

## Evaluate Rex Code

```rust
use rex_engine::Engine;
use rex_lexer::Token;
use rex_parser::Parser;

let tokens = Token::tokenize("let x = 1 + 2 in x * 3")?;
let mut parser = Parser::new(tokens);
let program = parser.parse_program().map_err(|errs| format!("{errs:?}"))?;

let mut engine = Engine::with_prelude()?;
engine.inject_decls(&program.decls)?;
let value = engine.eval(program.expr.as_ref())?;
println!("{value}");
```

## Typecheck Without Evaluating

```rust
use rex_lexer::Token;
use rex_parser::Parser;
use rex_ts::TypeSystem;

let tokens = Token::tokenize("map (\\x -> x) [1, 2, 3]")?;
let mut parser = Parser::new(tokens);
let program = parser.parse_program().map_err(|errs| format!("{errs:?}"))?;

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
let program = parser.parse_program().map_err(|errs| format!("{errs:?}"))?;

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
use rex_engine::Engine;
use rex_lexer::Token;
use rex_parser::Parser;

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
let program = parser.parse_program().map_err(|errs| format!("{errs:?}"))?;

let mut engine = Engine::with_prelude()?;
engine.inject_decls(&program.decls)?;
let value = engine.eval(program.expr.as_ref())?;
println!("{value}");
```

## Inject Native Values and Functions

`rex-engine` is the boundary where Rust provides implementations for Rex values.

```rust
use rex_engine::Engine;

let mut engine = Engine::with_prelude()?;
engine.inject_value("answer", 42i32)?;
engine.inject_fn1("inc", |x: i32| x + 1)?;
```

### Async Natives

If your host functions are async, inject them with `inject_async_fn*` and evaluate with
`Engine::eval_async` (or call `Engine::eval` to block on async natives).

```rust
use rex_engine::Engine;

let mut engine = Engine::with_prelude()?;
engine.inject_async_fn1("inc", |x: i32| async move { x + 1 })?;

let tokens = rex_lexer::Token::tokenize("inc 1")?;
let mut parser = rex_parser::Parser::new(tokens);
let program = parser
    .parse_program()
    .map_err(|errs| format!("parse error: {errs:?}"))?;
let v = engine.eval_async(program.expr.as_ref()).await?;
println!("{v}");
```

### Cancellation

Async natives can be cancelled. Cancellation is cooperative: you get a `CancellationToken` and
trigger it from another thread/task, and the engine will stop evaluation with `EngineError::Cancelled`.

```rust
use rex_engine::{CancellationToken, Engine, EngineError};
use futures::executor::block_on;

let tokens = rex_lexer::Token::tokenize("stall")?;
let mut parser = rex_parser::Parser::new(tokens);
let expr = parser
    .parse_program()
    .map_err(|errs| format!("parse error: {errs:?}"))?
    .expr;

let mut engine = Engine::with_prelude()?;
engine.inject_async_fn0_cancellable("stall", |token: CancellationToken| async move {
    token.cancelled().await;
    0i32
})?;

let token = engine.cancellation_token();
let handle = std::thread::spawn(move || block_on(engine.eval_async(expr.as_ref())));
token.cancel();
let res = handle
    .join()
    .map_err(|_| EngineError::Module("evaluation thread panicked".into()))?;
assert!(matches!(res, Err(EngineError::Cancelled)));
```

### Gas Metering

To defend against untrusted/large programs, you can run the pipeline with a gas budget:

- `Parser::parse_program_with_gas`
- `TypeSystem::infer_with_gas` / `infer_typed_with_gas`
- `Engine::eval_with_gas` / `eval_async_with_gas`

### Parsing Limits

For untrusted input, you can cap syntactic nesting depth during parsing:

```rust
use rex_parser::{Parser, ParserLimits};

let mut parser = Parser::new(rex_lexer::Token::tokenize("(((1)))")?);
parser.set_limits(ParserLimits::safe_defaults());
let program = parser.parse_program()?;
```

## Bridge Rust Types with `#[derive(Rex)]`

The derive:
- declares an ADT in the Rex type system
- injects runtime constructors (so Rex can *build* values)
- implements `FromValue`/`IntoValue` for converting Rust ↔ Rex

```rust
use rex_engine::{Engine, FromValue};
use rex_proc_macro::Rex;

#[derive(Rex, Debug, PartialEq)]
enum Maybe<T> {
    Just(T),
    Nothing,
}

let mut engine = Engine::with_prelude()?;
Maybe::<i32>::inject_rex(&mut engine)?;

let v = engine.eval(
    rex_parser::Parser::new(rex_lexer::Token::tokenize("Just 1")?)
        .parse_program()
        .map_err(|errs| format!("parse error: {errs:?}"))?
        .expr
        .as_ref(),
)?;
assert_eq!(Maybe::<i32>::from_value(&v, "v")?, Maybe::Just(1));
```

## Stack Size Entry Points

Some workloads (very deep nesting) can overflow the default thread stack. The project exposes
“large stack” entry points:

- `rex_parser::Parser::parse_program_with_stack_size`
- `rex_ts::TypeSystem::infer_with_stack_size`
- `rex_engine::Engine::eval_with_stack_size`
