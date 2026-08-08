# Rex Engine (`rex-engine`)

This crate prepares and evaluates Rex programs and supports host-native injection of functions and
values. The API exposes an explicit preparation boundary: `Builder` builds the host environment,
`Compiler` prepares Rex code into `CompiledProgram`, and a single-shot `Evaluator` runs one
prepared program with a map of runtime inputs for `main`. Builder/compiler/evaluator lineages are
single-use; create a new lineage for each program run. The runtime stores values in a private heap
and copies results into owned, heap-independent `Value` trees. It
supports closures, application, let-in, if-then-else, tuples/lists/dicts, and `match` expressions.

## Quickstart

```rust
use rex_engine::{Builder, CompileOptions, Module};
use rex_parser::parse;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut builder = Builder::with_prelude(())?;
    let mut globals = Module::global();
    globals.export("inc", |_state: &(), x: i32| { Ok(x + 1) })?;
    globals.export_value("answer", 42i32)?;
    builder.inject_module(globals)?;

    let program = parse("inc answer").map_err(|errs| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, format!("parse error: {errs:?}"))
    })?;
    let compiler = builder.build_compiler();
    let (compiled, evaluator) = compiler
        .compile_program(&program, CompileOptions::for_module("workflow.main")?)
        .await?;
    let value = evaluator.run(compiled, Default::default()).await?;

    assert_eq!(value.as_i32()?, 43);
    Ok(())
}
```

Phase-specific errors:

- `Compiler` returns `EngineError`
- `Evaluator::run` returns `EngineError`
- APIs that parse, compile, and run in one call return `ExecutionError`

## Runtime Values and GC

The runtime uses a moving copying collector owned exclusively by the builder/compiler/evaluator
lineage. `Evaluator::run` accepts and returns owned `Value`s. Host-call arguments are copied out of
the heap before dispatch, and results are validated and copied back after completion; host code and
host futures never receive heap access. See the
[memory-management guide](../docs/src/MEMORY_MANAGEMENT.md) for the internal rooting model.

## Internal Layout

The engine implementation is split by phase:

- `builder/`: builder-facing host/module registration and registry reporting.
- `compiler/`: program preparation, import rewriting, typechecking, module loading state, and
  `CompiledProgram` construction.
- `evaluator/`: scheduler-driven execution, native dispatch, runtime context, and runtime core
  state.

Shared runtime pieces such as heap values, engine options, module identities, and native handlers
live beside those phase directories.

## Injection API

- Build staged host APIs with `Module`.
- Use `Module::global()` for root-scope values/functions.
- Use `Module::new("acme.math", docs)` for importable modules, where `docs` is an
  `Option<String>` containing module-level Markdown documentation.
- Add typed exports with `export` / `export_async`.
- Add value-based exports with runtime-defined signatures using `export_native` /
  `export_native_async`.
- Add constant values with `export_value`; they are imported once when the module is installed.
- Add ADTs with `add_adt_decl` or `add_rex_adt::<T>()`.
- Materialize the staged module with `Builder::inject_module(...)`.
- For many available Rust modules where most programs import only a few, an `Importer<State>` can
  return `ResolvedModuleContent::module(module)` to install a named `Module<State>` lazily.

`Module::add_rex_adt::<T>()` collects `T`'s Rex family via `RexType::collect_rex_family` and
stages the reachable acyclic ADT family automatically. Ordinary leaf types inherit the default
no-op implementation, so they participate in Rex type mapping without pretending to be ADTs.

When embedding through the top-level `rex` crate, `#[rex::module]` and `#[rex::export]` capture
Rust doc comments and Rust function parameter names automatically. Manual modules pass their docs
to `Module::new`; manually constructed ADTs pass variant docs to `AdtDecl::add_variant` together
with a `Vec<AdtArgument>`. `Module::global()` still takes no arguments and creates an undocumented
root module. Rust parameters cannot carry individual doc comments, so their descriptions belong in
the function-level docs.

Operator names can be injected with parentheses (e.g., `"(+)"`); the engine normalizes to `+`.

`Builder` is generic over host state (`Builder<State>`, where `State: Clone + Send + Sync + 'static`).
`export` callbacks receive `&State` as the first argument and must return `Result<T, EngineError>`;
returning `Err(...)` fails evaluation.
`export_async` callbacks receive `&State` and return `Future<Output = Result<T, EngineError>>`;
returning `Err(...)` fails evaluation.
Value-based APIs (`export_native*`) receive `Context<State>`, the instantiated call type, and an
owned `Vec<Value>`. The context exposes host state and type information but no heap capability.
It does not retain runtime registries or internal root tokens.
`export_native*` validates `Scheme`/arity compatibility during registration.

## Prelude

`Builder::with_prelude(())?` injects the standard runtime helpers. If you need host state, pass
your state value instead: `Builder::with_prelude(state)?`.

The standard prelude source lives in `src/prelude/typeclasses.rex`. `src/prelude/type_system.rs`
builds the prelude-enabled `TypeSystem`, while `src/prelude/mod.rs` parses the source, exposes
`standard_type_system()`, and wires the corresponding runtime/native helpers.

For explicit control, use:

- `Builder::with_options(state, EngineOptions { ... })`
- `PreludeMode::{Enabled, Disabled}`
- `default_imports` (defaults to importing `std.prelude` weakly)

- **Constructors**: `Empty`, `Cons`, `Some`, `None`, `Ok`, `Err`
- **Arithmetic**: `+`, `-`, `*`, `/`, `negate`, `zero`, `one`
- **Equality**: `==`, `!=`
- **Ordering**: `<`, `<=`, `>`, `>=`
- **Booleans**: `&&`, `||`
- **Collection combinators** (List/Option/Result): `map`, `fold`, `foldl`, `foldr`, `filter`, `filter_map`, `bind`, `ap`, `sum`, `mean`, `length`, `first`, `last`, `slice`, `take`, `skip`, `zip`, `unzip`, `min`, `max`, `or_else`
- **Option/Result helpers**: `is_some`, `is_none`, `is_ok`, `is_err`

Rust `Vec<T>` values convert to `Value::List` and Rex `List T`. `Vec<u8>` and every outbound Rex
`List U8` use `Value::Bytes`, regardless of the list's private heap representation.

## Type Defaults

Some expressions can leave overloaded values ambiguous (for example, `one` or `zero` in a polymorphic branch). During evaluation, the engine applies a small defaulting pass to pick a concrete type when possible:

- Prefer primitive types already observed in the expression.
- Fall back to `f32`, then `i32`, then `String`.

## Tests

Run:

```bash
cargo test -p rex-engine
```
