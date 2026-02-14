# Rex Engine (`rex-engine`)

This crate evaluates Rex ASTs and supports host-native injection of functions and values. Evaluation is type-aware: the engine runs the Rex type system first, then evaluates a typed AST so overloads and native implementations are selected by type. The runtime operates on `Value` and supports closures, application, let-in, if-then-else, tuples/lists/dicts, and `match` expressions.

## Quickstart

```rust
use rex_engine::Engine;
use rex_lexer::Token;
use rex_parser::Parser;
use rex_util::{GasCosts, GasMeter};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut engine = Engine::with_prelude(())?;
    engine.inject_fn2("(+)", |_state, x: i32, y: i32| -> i32 { x + y })?;
    engine.inject_value("answer", 42i32)?;

    let tokens = Token::tokenize("answer + 1")?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program(&mut GasMeter::default()).map_err(|errs| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, format!("parse error: {errs:?}"))
    })?;
    let mut gas = GasMeter::default();
    let value = engine
        .eval_with_gas(program.expr.as_ref(), &mut gas)
        .await?;

    assert_eq!(engine.heap().pointer_as_i32(&value)?, 43);
    Ok(())
}
```

## Injection API

- `inject_value(name, value)`: inject a constant value (numbers, strings, arrays, etc.).
- `inject_fn0`, `inject_fn1`, `inject_fn2`: inject native functions with 0–2 args.
- `inject_native`: inject an arbitrary native function with explicit arity and access to raw `Value` slices.
- `adt_decl` + `inject_adt`: declare and register ADT constructors (mirrors `type` declarations).
- `inject_class`: register a type class (mirrors `class` declarations).
- `inject_instance`: register a type class instance in the checker (mirrors `instance` declarations).

Operator names can be injected with parentheses (e.g., `"(+)"`); the engine normalizes to `+`.

`Engine` is generic over host state (`Engine<State>`, where `State: Clone + Sync + 'static`).
`inject_fn*` / `inject_async_fn*` callbacks receive `&State` as the first argument.
Pointer-level APIs (`inject_native*`) receive `&Engine<State>` so they can access heap/runtime internals.

## Prelude

`Engine::with_prelude(())?` injects the standard runtime helpers. If you need host state, pass
your state value instead: `Engine::with_prelude(state)?`.

- **Constructors**: `Empty`, `Cons`, `Some`, `None`, `Ok`, `Err`
- **Arithmetic**: `+`, `-`, `*`, `/`, `negate`, `zero`, `one`
- **Equality**: `==`, `!=`
- **Ordering**: `<`, `<=`, `>`, `>=`
- **Booleans**: `&&`, `||`
- **Collection combinators** (List/Array/Option/Result): `map`, `fold`, `foldl`, `foldr`, `filter`, `filter_map`, `bind`, `ap`, `sum`, `mean`, `count`, `take`, `skip`, `zip`, `unzip`, `min`, `max`, `or_else`
- **Option/Result helpers**: `is_some`, `is_none`, `is_ok`, `is_err`

List literals are evaluated to the `Empty`/`Cons` ADT constructors (stored as `Value::Adt`). Arrays are host-native `Value::Array` values injected from Rust (e.g., by passing `Vec<T>` to `inject_value`) and participate in the same collection combinators via type classes.

## Type Defaults

Some expressions can leave overloaded values ambiguous (for example, `one` or `zero` in a polymorphic branch). During evaluation, the engine applies a small defaulting pass to pick a concrete type when possible:

- Prefer primitive types already observed in the expression.
- Fall back to `f32`, then `i32`, then `string`.

## Tests

Run:

```bash
cargo test -p rex-engine
```
