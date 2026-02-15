# Functors

The prelude defines:

```rex,interactive
class Functor f
  map : (a -> b) -> f a -> f b
```

`map` applies a pure function inside a container `f`.

If you can describe an operation as “change the values without changing the structure”, it’s a
good fit for `map`.

## Mapping over lists

```rex,interactive
map ((*) 2) [1, 2, 3]
```

### Mental model

Each element is transformed independently; list length stays the same.

## Mapping over `Option`

```rex,interactive
( map ((+) 1) (Some 41)
, map ((+) 1) None
)
```

`None` acts like “no value to transform”.

## Mapping over `Result`

```rex,interactive
( map ((*) 2) (Ok 21)
, map ((*) 2) (Err "boom")
)
```

`map` transforms `Ok` values but leaves `Err` unchanged.

## A useful pattern: composing transforms

Instead of branching on shapes early, keep your code “in the functor”:

```rex,interactive
let inc = \x -> x + 1 in
  map inc (Some 1)
```
