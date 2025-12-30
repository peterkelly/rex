# Rex Engine (`rex-engine`)

This crate evaluates Rex ASTs and supports host-native injection of functions and values. Evaluation is type-aware: the engine runs the Rex type system first, then evaluates a typed AST so overloads and native implementations are selected by type. The runtime operates on `Value` and supports closures, application, let-in, if-then-else, tuples/lists/dicts, and `match` expressions.

## Quickstart

```rust
use rex_engine::{Engine, Value};
use rex_lexer::Token;
use rex_parser::Parser;

let mut engine = Engine::with_prelude();
engine.inject_fn2("(+)", |x: i32, y: i32| -> i32 { x + y });
engine.inject_value("answer", 42i32);

let mut parser = Parser::new(Token::tokenize("answer + 1").unwrap());
let program = parser.parse_program().unwrap();
let expr = program.expr;
let value = engine.eval(expr.as_ref()).unwrap();

assert!(matches!(value, Value::I32(43)));
```

## Injection API

- `inject_value(name, value)`: inject a constant value (numbers, strings, arrays, etc.).
- `inject_fn0`, `inject_fn1`, `inject_fn2`: inject native functions with 0–2 args.
- `inject_native`: inject an arbitrary native function with explicit arity and access to raw `Value` slices.
- `adt_decl` + `inject_adt`: declare and register ADT constructors (mirrors `type` declarations).
- `inject_class`: register a type class (mirrors `class` declarations).
- `inject_instance`: register a type class instance in the checker (mirrors `instance` declarations).

Operator names can be injected with parentheses (e.g., `"(+)"`); the engine normalizes to `+`.

## Prelude

`Engine::with_prelude()` injects the standard runtime helpers:

- **Constructors**: `Empty`, `Cons`, `Some`, `None`, `Ok`, `Err`
- **Arithmetic**: `+`, `-`, `*`, `/`, `negate`, `zero`, `one`
- **Equality**: `==`, `!=`
- **Ordering**: `<`, `<=`, `>`, `>=`
- **Booleans**: `&&`, `||`
- **Collection combinators** (List/Array/Option/Result): `map`, `fold`, `foldl`, `foldr`, `filter`, `filter_map`, `flat_map`, `sum`, `mean`, `count`, `take`, `skip`, `zip`, `unzip`, `min`, `max`, `and_then`, `or_else`
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
