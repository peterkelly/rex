# Example: Small “stdlib” helpers

It’s common to build small helpers with `let` and reuse them.

## Goal

Practice building tiny “glue” functions that keep your code readable, especially when composing
many operations.

```rex
let
  compose = \f g x -> f (g x),
  inc = \x -> x + 1,
  double = \x -> x * 2,
  inc_then_double = compose double inc
in
  inc_then_double 10
```

## Exercise

Write `double_then_inc` and confirm it produces a different result.

## A more “pipeline” style

Sometimes it’s clearer to read left-to-right:

```rex
let
  pipe = \x f -> f x,
  pipe2 = \x f g -> g (f x),
  inc = \x -> x + 1,
  double = \x -> x * 2
in
  pipe2 10 inc double
```

This is the same logic as `double (inc 10)`, just easier to extend when you have many steps.

## Exercises

1. Write `pipe3` and use it to apply three transforms.
2. Use `compose` to build a function that squares a number and then adds 1.
3. Combine `map` with `compose` to map a composed function over a list.
