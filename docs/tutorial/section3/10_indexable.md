# Example: `Indexable`

`Indexable` is a multi-parameter class in the prelude (so you can use `get` without defining the
class yourself).

## Goal

Learn how to pull elements out of common containers with a single API:

- lists
- arrays
- small tuples

Example uses:

```rex
( get 0 [10, 20, 30]
, get 2 (1, 2, 3)
)
```

## How it works

- Lists and arrays have `Indexable (List a, a)` / `Indexable (Array a, a)` instances.
- Some tuples also have `Indexable` instances (see `rex-ts/src/prelude_typeclasses.rex`).

## A more guided example

```rex
let
  xs = [10, 20, 30],
  first = get 0 xs,
  third = get 2 xs,
  tup = (1, 2, 3),
  last = get 2 tup
in
  (first, third, last)
```

## Note on out-of-bounds

`get` is generally expected to error if the index is out of bounds (depending on the host/runtime
implementation). If you need safe indexing (returning `Option`/`Result`), write it with `match` on
your container type (e.g. `[]` vs `x:xs` for lists).

## Exercises

1. Write `head : List a -> Option a` using `match` (`[]` vs `x:xs`).
2. Use `get` to read elements out of a tuple `(100, 200, 300, 400)` (if your prelude provides that
   arity; check `rex-ts/src/prelude_typeclasses.rex`).
