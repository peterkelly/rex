# Rex Engine (`rexlang-engine`)

This crate evaluates Rex ASTs and supports host-native injection of functions and values. The API
now exposes an explicit preparation boundary: `Engine` builds the host environment, `Compiler`
prepares Rex code into `CompiledProgram`, and `Evaluator` runs prepared programs. The current
implementation still keeps engine-backed state internally, but the compile/runtime split is now a
first-class part of the public API. The runtime operates on `Value` and supports closures,
application, let-in, if-then-else, tuples/lists/dicts, and `match` expressions.

## Quickstart

```rust
use rex_engine::{Engine, Library};
use rex_lexer::Token;
use rex_parser::Parser;
use rex_util::{GasCosts, GasMeter};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut engine = Engine::with_prelude(())?;
    let mut globals = Library::global();
    globals.export("(+)", |_state, x: i32, y: i32| { Ok(x + y) })?;
    globals.export_value("answer", 42i32)?;
    engine.inject_library(globals)?;

    let tokens = Token::tokenize("answer + 1")?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program(&mut GasMeter::default()).map_err(|errs| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, format!("parse error: {errs:?}"))
    })?;
    let mut compiler = Compiler::new(engine.clone());
    let mut evaluator =
        Evaluator::new_with_compiler(RuntimeEnv::new(engine.clone()), Compiler::new(engine.clone()));
    let mut gas = GasMeter::default();
    let compiled = compiler.compile_expr(program.expr.as_ref())?;
    let value = evaluator.run(&compiled, &mut gas).await?;

    assert_eq!(engine.heap.pointer_as_i32(&value)?, 43);
    Ok(())
}
```

Phase-specific errors:

- `Compiler` returns `CompileError`
- `Evaluator::run` returns `EvalError`
- convenience helpers like `eval_snippet` return `ExecutionError`

## Injection API

- Build staged host APIs with `Library`.
- Use `Library::global()` for root-scope values/functions.
- Use `Library::new("acme.math")` for importable modules.
- Add typed exports with `export` / `export_async`.
- Add pointer-level exports with `export_native` / `export_native_async`.
- Add constant values with `export_value`.
- Add ADTs with `add_adt_decl` or `add_rex_adt::<T>()`.
- Materialize the staged library with `Engine::inject_library(...)`.

`Library::add_rex_adt::<T>()` collects `T`'s Rex family via `RexType::collect_rex_family` and
stages the reachable acyclic ADT family automatically. Ordinary leaf types inherit the default
no-op implementation, so they participate in Rex type mapping without pretending to be ADTs.

Operator names can be injected with parentheses (e.g., `"(+)"`); the engine normalizes to `+`.

`Engine` is generic over host state (`Engine<State>`, where `State: Clone + Sync + 'static`).
`export` callbacks receive `&State` as the first argument and must return `Result<T, EngineError>`;
returning `Err(...)` fails evaluation.
`export_async` callbacks receive `&State` and return `Future<Output = Result<T, EngineError>>`;
returning `Err(...)` fails evaluation.
Pointer-level APIs (`export_native*`) receive `EvaluatorRef<'_, State>` so they can access heap/runtime internals.
`export_native*` validates `Scheme`/arity compatibility during registration.

## Prelude

`Engine::with_prelude(())?` injects the standard runtime helpers. If you need host state, pass
your state value instead: `Engine::with_prelude(state)?`.

For explicit control, use:

- `Engine::with_options(state, EngineOptions { ... })`
- `PreludeMode::{Enabled, Disabled}`
- `default_imports` (defaults to importing `Prelude` weakly)

- **Constructors**: `Empty`, `Cons`, `Some`, `None`, `Ok`, `Err`
- **Arithmetic**: `+`, `-`, `*`, `/`, `negate`, `zero`, `one`
- **Equality**: `==`, `!=`
- **Ordering**: `<`, `<=`, `>`, `>=`
- **Booleans**: `&&`, `||`
- **Collection combinators** (List/Array/Option/Result): `map`, `fold`, `foldl`, `foldr`, `filter`, `filter_map`, `bind`, `ap`, `sum`, `mean`, `count`, `take`, `skip`, `zip`, `unzip`, `min`, `max`, `or_else`
- **Option/Result helpers**: `is_some`, `is_none`, `is_ok`, `is_err`

List literals are evaluated to the `Empty`/`Cons` ADT constructors (stored as `Value::Adt`). Arrays are host-native `Value::Array` values injected from Rust (e.g., by passing `Vec<T>` to `export_value`) and participate in the same collection combinators via type classes.

## Type Defaults

Some expressions can leave overloaded values ambiguous (for example, `one` or `zero` in a polymorphic branch). During evaluation, the engine applies a small defaulting pass to pick a concrete type when possible:

- Prefer primitive types already observed in the expression.
- Fall back to `f32`, then `i32`, then `string`.

## Tests

Run:

```bash
cargo test -p rexlang-engine
```
