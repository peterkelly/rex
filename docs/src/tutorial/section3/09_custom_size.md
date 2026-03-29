# Example: Custom `Size` type class

This mirrors `rexlang-cli/examples/typeclasses_custom_size.rex`.

## Goal

Use a type class to define a common “size” operation across different data types, without
hard-coding the type at every call site.

```rex,interactive
class Size a
  size : a -> i32

type Blob = Blob { bytes: List i32 }

instance Size (List t)
  size = \xs ->
    match xs
      when Empty -> 0
      when Cons _ t -> 1 + size t

instance Size Blob
  size = \b -> size b.bytes

size (Blob { bytes = [1, 2, 3, 4] })
```

## What this demonstrates

- defining a class with one method,
- writing an instance for a prelude type (`List`),
- writing an instance for your own ADT (`Blob`),
- using `match` for recursion.

## Calling `size` generically

Once you have a class, you can write functions that work for *any* type that has an instance:

```rex,interactive
class Size a
  size : a -> i32

instance Size (List t)
  size = \xs ->
    match xs
      when Empty -> 0
      when Cons _ t -> 1 + size t

let
  bigger = \(x: a) where Size a -> size x + 1
in
  bigger [1, 2, 3]
```

The `where Size a` constraint says: “this function is valid as long as `Size a` exists”.

## Worked examples

### Example: `is_empty`

Problem: write a generic emptiness check from `size`.

```rex,interactive
class Size a
  size : a -> i32

instance Size (List t)
  size = \xs ->
    match xs
      when Empty -> 0
      when Cons _ t -> 1 + size t

let
  is_empty = \(x: a) where Size a -> size x == 0
in
  (is_empty ([] is List i32), is_empty [1, 2, 3])
```

Why this works: any type with a `Size` instance can reuse the same `is_empty` logic.

### Example: `Blob` with `name`

Problem: add a `name` field and keep size based on bytes only.

```rex,interactive
class Size a
  size : a -> i32

instance Size (List t)
  size = \xs ->
    match xs
      when Empty -> 0
      when Cons _ t -> 1 + size t

type Blob = Blob { name: string, bytes: List i32 }

instance Size Blob
  size = \b -> size b.bytes

size (Blob { name = "payload", bytes = [1, 2, 3, 4] })
```

Why this works: `Size Blob` delegates to the `bytes` list, so metadata does not affect size.

### Example: `total_size`

Problem: sum sizes of a list of values.

```rex,interactive
class Size a
  size : a -> i32

instance Size (List t)
  size = \xs ->
    match xs
      when Empty -> 0
      when Cons _ t -> 1 + size t

let
  total_size = \(xs: List a) where Size a ->
    foldl (\acc x -> acc + size x) 0 xs
in
  total_size [[1, 2], [], [3]]
```

Why this works: `foldl` accumulates per-element sizes using the shared `Size a` method.
