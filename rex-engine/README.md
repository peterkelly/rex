# Rex Engine (`rex-engine`)

This crate prepares and evaluates Rex programs and supports host-native injection of functions and
values. The API exposes an explicit preparation boundary: `Engine` builds the host environment,
`Compiler` prepares Rex code into `CompiledProgram`, and a single-shot `Evaluator` runs one
prepared program with a map of runtime inputs for `main`. The runtime stores values in the heap and
returns rooted `Handle`s; `Handle::value()` exposes safe public `Value` views for inspection. It
supports closures, application, let-in, if-then-else, tuples/lists/dicts, and `match` expressions.

## Quickstart

```rust
use rex_engine::{Engine, Module};
use rex_parser::parse;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut engine = Engine::with_prelude(())?;
    let mut globals = Module::global();
    globals.export("inc", |_state: &(), x: i32| { Ok(x + 1) })?;
    globals.export_value("answer", 42i32)?;
    engine.inject_module(globals)?;

    let program = parse("inc answer").map_err(|errs| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, format!("parse error: {errs:?}"))
    })?;
    let mut compiler = engine.into_compiler();
    let compiled = compiler.compile_program(&program, Default::default()).await?;
    let evaluator = compiler.into_evaluator();
    let value = evaluator.run(compiled, Default::default()).await?;

    assert_eq!(value.as_i32()?, 43);
    Ok(())
}
```

Phase-specific errors:

- `Compiler` returns `EngineError`
- `Evaluator::run` returns `EngineError`
- APIs that parse, compile, and run in one call return `ExecutionError`

## Internal Layout

The engine implementation is split by phase:

- `builder/`: host environment construction, module injection, import rewriting, export
  registration, and registry reporting.
- `compiler/`: typechecking plus `CompiledProgram` construction.
- `evaluator/`: scheduler-driven execution, native dispatch, runtime context, and runtime core
  state.

Shared runtime pieces such as heap values, engine options, module identities, and native handlers
live beside those phase directories.

## Injection API

- Build staged host APIs with `Module`.
- Use `Module::global()` for root-scope values/functions.
- Use `Module::new("acme.math")` for importable modules.
- Add typed exports with `export` / `export_async`.
- Add pointer-level exports with `export_native` / `export_native_async`.
- Add constant values with `export_value`.
- Add ADTs with `add_adt_decl` or `add_rex_adt::<T>()`.
- Materialize the staged module with `Engine::inject_module(...)`.

`Module::add_rex_adt::<T>()` collects `T`'s Rex family via `RexType::collect_rex_family` and
stages the reachable acyclic ADT family automatically. Ordinary leaf types inherit the default
no-op implementation, so they participate in Rex type mapping without pretending to be ADTs.

Operator names can be injected with parentheses (e.g., `"(+)"`); the engine normalizes to `+`.

`Engine` is generic over host state (`Engine<State>`, where `State: Clone + Send + Sync + 'static`).
`export` callbacks receive `&State` as the first argument and must return `Result<T, EngineError>`;
returning `Err(...)` fails evaluation.
`export_async` callbacks receive `&State` and return `Future<Output = Result<T, EngineError>>`;
returning `Err(...)` fails evaluation.
Pointer-level APIs (`export_native*`) receive `Context<State>` so they can access heap/runtime internals.
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

List literals are evaluated to the `Empty`/`Cons` ADT constructors and inspect as `Value::Adt`.
Arrays are host-native array values that inspect as `Value::Array` when injected from Rust (for
example by passing `Vec<T>` to `export_value`) and participate in the same collection combinators
via type classes.

## Type Defaults

Some expressions can leave overloaded values ambiguous (for example, `one` or `zero` in a polymorphic branch). During evaluation, the engine applies a small defaulting pass to pick a concrete type when possible:

- Prefer primitive types already observed in the expression.
- Fall back to `f32`, then `i32`, then `string`.

## Tests

Run:

```bash
cargo test -p rex-engine
```
