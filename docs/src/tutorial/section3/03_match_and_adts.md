# Example: ADTs + `match`

This example shows the usual pattern:

1. define an ADT
2. consume it with `match`

## Goal

Build a tiny “Maybe” API:

- `fromMaybe` (extract with default)
- `mapMaybe` (transform inside `Just`)
- `isJust` (check which constructor you have)

## Define an ADT

```rex,interactive
type Maybe a = Just a | Nothing
```

## Use it

```rex,interactive
type Maybe a = Just a | Nothing

let
  fromMaybe = \d m ->
    match m
      when Just x -> x
      when Nothing -> d
in
  ( fromMaybe 0 (Just 5)
  , fromMaybe 0 Nothing
  )
```

### Next steps

The next example implements `mapMaybe`, which applies a function to `Just x` and leaves
`Nothing` unchanged.

## A worked `mapMaybe`

```rex,interactive
type Maybe a = Just a | Nothing

let
  mapMaybe = \f m ->
    match m
      when Just x -> Just (f x)
      when Nothing -> Nothing
in
  ( mapMaybe ((+) 1) (Just 41)
  , mapMaybe ((+) 1) Nothing
  )
```

## Testing constructors with `match`

There’s no special “isJust” operator — you write it with `match`:

```rex,interactive
type Maybe a = Just a | Nothing

let
  isJust = \m ->
    match m
      when Just _ -> true
      when Nothing -> false
in
  (isJust (Just 1), isJust Nothing)
```

## Common mistake: missing arms

When matching on an ADT, Rex checks exhaustiveness. If you forget an arm, you’ll get an error that
names the missing constructors.

## Worked examples

### Example: `orElse`

Problem: return the first `Just` value, otherwise return the fallback `Maybe`.

```rex,interactive
type Maybe a = Just a | Nothing

let
  orElse = \ma mb ->
    match ma
      when Just x -> Just x
      when Nothing -> mb
in
  (orElse (Just 1) (Just 2), orElse Nothing (Just 2))
```

Why this works: `match` chooses `ma` when it is `Just`, and only uses `mb` when `ma` is `Nothing`.

### Example: `andThen` (`Maybe` bind)

Problem: chain a function that returns `Maybe`, failing early on `Nothing`.

```rex,interactive
type Maybe a = Just a | Nothing

let
  andThen = \f ma ->
    match ma
      when Just x -> f x
      when Nothing -> Nothing,
  step = \x -> if x > 0 then Just (x + 1) else Nothing
in
  (andThen step (Just 3), andThen step Nothing, andThen step (Just (0 - 1)))
```

Why this works: `Just` unwraps and continues with `f`; `Nothing` short-circuits immediately.

### Example: adding `Unknown` and handling exhaustiveness

Problem: extend `Maybe` and still keep matches exhaustive.

```rex,interactive
type Maybe a = Just a | Nothing | Unknown

let
  fromMaybe = \d m ->
    match m
      when Just x -> x
      when Nothing -> d
      when Unknown -> d
in
  (fromMaybe 0 (Just 5), fromMaybe 0 Unknown)
```

Why this works: the `Unknown` arm makes the match complete for all constructors.
