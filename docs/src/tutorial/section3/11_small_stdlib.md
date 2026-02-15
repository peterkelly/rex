# Example: Small “stdlib” helpers

It’s common to build small helpers with `let` and reuse them.

## Goal

Practice building tiny “glue” functions that keep your code readable, especially when composing
many operations.

```rex,interactive
let
  compose = \f g x -> f (g x),
  inc = \x -> x + 1,
  double = \x -> x * 2,
  inc_then_double = compose double inc
in
  inc_then_double 10
```

## Worked example: `double_then_inc`

Problem: define `double_then_inc` and show it differs from `inc_then_double`.

```rex,interactive
let
  compose = \f g x -> f (g x),
  inc = \x -> x + 1,
  double = \x -> x * 2,
  inc_then_double = compose double inc,
  double_then_inc = compose inc double
in
  (inc_then_double 10, double_then_inc 10)
```

Why this works: function composition order changes the final result (`22` vs `21`).

## A more “pipeline” style

Sometimes it’s clearer to read left-to-right:

```rex,interactive
let
  pipe = \x f -> f x,
  pipe2 = \x f g -> g (f x),
  inc = \x -> x + 1,
  double = \x -> x * 2
in
  pipe2 10 inc double
```

This is the same logic as `double (inc 10)`, just easier to extend when you have many steps.

## Worked examples

### Example: `pipe3`

Problem: apply three transforms in left-to-right style.

```rex,interactive
let
  pipe3 = \x f g h -> h (g (f x)),
  inc = \x -> x + 1,
  double = \x -> x * 2,
  square = \x -> x * x
in
  pipe3 3 inc double square
```

Why this works: each function consumes the previous output, so the pipeline is explicit.

### Example: compose square then add one

Problem: build a reusable function that squares and then increments.

```rex,interactive
let
  compose = \f g x -> f (g x),
  square = \x -> x * x,
  add1 = \x -> x + 1,
  square_then_add1 = compose add1 square
in
  square_then_add1 5
```

Why this works: `compose add1 square` creates `\x -> add1 (square x)`.

### Example: map a composed function

Problem: combine `map` with composition for list transforms.

```rex,interactive
let
  compose = \f g x -> f (g x),
  square = \x -> x * x,
  add1 = \x -> x + 1,
  square_then_add1 = compose add1 square
in
  map square_then_add1 [1, 2, 3, 4]
```

Why this works: one composed function encapsulates the per-element transform applied by `map`.
