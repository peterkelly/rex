# Constraints and `where`

Sometimes a function is only valid when certain type-class constraints hold. In Rex you’ll see
those constraints in type signatures (for `fn`) and in `where` clauses (commonly for lambdas).

## Constrained lambdas

This example is from `rexlang-cli/examples/type_classes.rex`:

```rex,interactive
let
  use_classes = \ (x: t) (y: f a) (z: a) where Indexable (t, a), Foldable f ->
    let
      first = get 0 x,
      total = foldl (\acc _ -> acc) z y
    in
      (first, total, z)
in
  use_classes [10, 20, 30] [1, 2, 3] 0
```

Notes:

- `where ... ->` attaches constraints to the lambda.
- Multi-parameter classes like `Indexable t a` are written as `Indexable (t, a)` in the `where`
  list (internally they’re represented as tupled predicates).
- Constraints can use library-qualified class names when imported via alias (for example
  `where M.ClassName t -> ...`).

## Constrained top-level functions

Top-level functions can also have a `where` clause:

```rex,interactive
fn sum_list : List i32 -> i32 where Foldable List = \xs -> foldl (+) 0 xs
```

Constraints appear after the type signature and before `=`.

### Multiple constraints

Constraints are comma-separated:

```rex,interactive
fn demo : List i32 -> i32 where Foldable List, AdditiveMonoid i32 = \xs -> foldl (+) 0 xs
```

> **Note:** In many cases you don’t need to write constraints for concrete prelude types (`List`, `Option`,
> etc.) because the argument type already forces instance selection. `where` becomes more important
> when you want *polymorphic* constraints (e.g. “for any `f` with `Foldable f`”).
