# Example: `Indexable`

`Indexable` is a multi-parameter class in the prelude (so you can use `get` without defining the
class yourself).

## Goal

Learn how to pull elements out of common containers with a single API:

- lists
- arrays

Example uses:

```rex,interactive
( get 0 [10, 20, 30] , get 2 ["a", "b", "c"] )
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
your container type (e.g. `[]` vs `x::xs` for lists).

## Worked examples

### Example: `head : List a -> Option a`

Problem: return the first list element safely.

```rex,interactive
let
  head = \xs ->
    match xs
      when [] -> None
      when x::_ -> Some x
in
  (head [] is Option i32, head [10, 20, 30])
```

Why this works: pattern matching handles empty and non-empty shapes explicitly.

### Example: `get` on a larger list

Problem: read specific indices from `[100, 200, 300, 400]`.

```rex,interactive
let
  xs = [100, 200, 300, 400]
in
  (get 0 xs, get 2 xs, get 3 xs)
```

Why this works: `Indexable (List a, a)` provides `get` for list element access.
