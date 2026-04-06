# Rex Type System (`rexlang-typesystem`)

This crate implements a Hindley-Milner style type system with parametric polymorphism, type classes, and simple algebraic data types (ADTs). It is intended to power Rex's checker and can also be reused by host code to register additional builtins or domain-specific types.

## Features

- **Types**: primitives (`u8`, `u16`, `u32`, `u64`, `i8`, `i16`, `i32`, `i64`, `f32`, `f64`, `bool`, `string`, `uuid`, `datetime`), tuples, functions, dictionaries (`Dict a`), and user constructors (`List`, `Result`, `Option`).
- **Arrays**: host-native `Array a` values intended for performance and supported by the same collection combinators via type classes.
- **Schemes**: quantified types with class constraints.
- **Type classes**: class hierarchy for additive/multiplicative monoids, semiring, additive group, ring, and field, plus instance resolution with superclass propagation.
- **Prelude**: ADT constructors (`Empty`, `Cons`, `Ok`, `Err`, `Some`, `None`) and constrained function declarations for numeric, equality, list, option, and result operations.
- **Utilities**: substitution, unification (with occurs check), instantiation, and generalization helpers.

## Quickstart

```rust
use rex_ast::expr::intern;
use rex_ts::{BuiltinTypeId, Predicate, Type, TypeSystem};

fn main() -> Result<(), rex_ts::TypeError> {
    let mut ts = TypeSystem::new_with_prelude()?;

    // Register an additional function: id :: a -> a
    let a = Type::var(ts.supply.fresh(Some(intern("a"))));
    let scheme = rex_ts::generalize(&ts.env, vec![], Type::fun(a.clone(), a));
    ts.add_value("id", scheme);

    // Ask whether a class constraint is satisfiable in the prelude
    let field_f32 = Predicate::new("Field", Type::builtin(BuiltinTypeId::F32));
    assert!(rex_ts::entails(&ts.classes, &[], &field_f32)?);
    Ok(())
}
```

## Inference

The inference entry points are `infer(&mut ts, expr)` (predicates + type) and `infer_typed(&mut ts, expr)` (typed AST + predicates + type), both of which work on `rex_ast::expr::Expr`. Literal behavior follows the current language requirement:

The inference engine and its tests live in `src/inference.rs`; `src/typesystem.rs` contains the shared type-system data structures, unification machinery, and declaration/typeclass registration code that inference builds on.

- Integer literals are `Integral`-constrained and can specialize from context; ambiguous cases default later to `i32`
- Float literals default to `f32`

Example (from tests):

```rex
let
  id = \x -> x
in
  id (id 420, id 6.9, id "str")
```

This infers to the tuple type `(i32, f32, string)`.

The current inference implementation covers variables, application, lambdas, let-in, if-then-else, tuples, lists, dicts, named constructors, literals, and `match`.

## Prelude Summary

### Classes

- `AdditiveMonoid`, `MultiplicativeMonoid`, `Semiring`, `AdditiveGroup`, `Ring`, `Field`
- `Eq` (equality), `Ord` (ordering)
- `Functor`, `Applicative`, `Monad`, `Foldable`, `Filterable`, `Sequence`, `Alternative`

### Operators

- Arithmetic: `+`, `-`, `*`, `/`, `negate`
- Equality: `==`, `!=` (`Eq a` constraints)
- Ordering: `<`, `<=`, `>`, `>=` (`Ord a` constraints)
- Booleans: `&&`, `||` (bool only)
- Monoid identities: `zero`, `one`

### Collection Combinators

- `map`, `fold`, `foldl`, `foldr`, `filter`, `filter_map`, `bind`, `ap`, `or_else`
- `sum`, `mean`, `count`, `take`, `skip`, `zip`, `unzip`, `min`, `max`
These are type-class constrained and work for `List`, `Array`, `Option`, and `Result` where applicable.

### Option/Result Helpers

- Option: `is_some`, `is_none`
- Result: `is_ok`, `is_err`

Run the library tests with:

```bash
cargo test -p rexlang-typesystem
```

## Extending

You can register new ADTs with `AdtDecl` + `TypeSystem::register_adt`. The `TypeSystem` also exposes `register_instance` for low-level instance-head injection and a `supply` for generating fresh type variables when building new schemes.
