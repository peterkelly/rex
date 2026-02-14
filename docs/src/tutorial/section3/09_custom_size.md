# Example: Custom `Size` type class

This mirrors `rex/examples/typeclasses_custom_size.rex`.

## Goal

Use a type class to define a common “size” operation across different data types, without
hard-coding the type at every call site.

```rex
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

```rex
class Size a
  size : a -> i32

let
  bigger = \x where Size a -> size x + 1
in
  bigger [1, 2, 3]
```

The `where Size a` constraint says: “this function is valid as long as `Size a` exists”.

## Exercises

1. Write `is_empty : a -> bool where Size a` that checks `size x == 0`.
2. Extend `Blob` with a `name: string` field and decide whether the name should affect size.
3. Write `total_size : List a -> i32 where Size a` that sums sizes of elements.
