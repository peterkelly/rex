# Example: `Indexable`

`Indexable` is a multi-parameter class in the prelude (so you can use `get` without defining the
class yourself).

## Goal

Learn how to pull elements out of common containers with a single API:

- lists
- arrays

Example uses:

```rex,interactive
( get 0 [10, 20, 30] , get 2 ["a", "b", "c"] ) -- returns (10, "c")
```

## How it works

- Lists and arrays have `Indexable (List a, a)` / `Indexable (Array a, a)` instances.
- Tuples use numeric projection like `.0` and `.1` instead of `get`.

## A more guided example

```rex,interactive
let
  xs = [10, 20, 30],
  first = get 0 xs,
  third = get 2 xs,
  ys = [1, 2, 3],
  last = get 2 ys
in
  (first, third, last)
```

## Note on out-of-bounds

`get` is generally expected to error if the index is out of bounds (depending on the host/runtime
implementation). If you need safe indexing (returning `Option`/`Result`), write it with `match` on
your container type (e.g. `[]` vs `x:xs` for lists).

## Exercises

1. Write `head : List a -> Option a` using `match` (`[]` vs `x:xs`).
2. Use `get` to read elements out of a list `[100, 200, 300, 400]`.
