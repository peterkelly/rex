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

```rex
type Maybe a = Just a | Nothing
```

## Use it

```rex
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

Try adding a function `mapMaybe` that applies a function to `Just x` and leaves `Nothing` alone.

## A worked `mapMaybe`

```rex
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

```rex
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

## Exercises

1. Write `orElse : Maybe a -> Maybe a -> Maybe a` that returns the first `Just`, otherwise the
   second value.
2. Write `andThen : (a -> Maybe b) -> Maybe a -> Maybe b` (this is “bind” for `Maybe`).
3. Add a constructor `Unknown` to `Maybe` and see how the exhaustiveness checker reacts.
