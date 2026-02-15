# Example: `Result` workflows

`Result a e` short-circuits on `Err`.

## Goal

Model a computation that can fail with a useful error, while keeping “happy path” code easy to
read.

```rex,interactive
let
  step1 = \x -> if x < 0 then Err "negative" else Ok (x + 1),
  step2 = \x -> Ok (x * 2)
in
  ( bind step2 (bind step1 (Ok 10))
  , bind step2 (bind step1 (Ok (0 - 1)))
  )
```

## What to notice

- When `step1` returns `Err`, the second `bind` is skipped.
- This lets you write “happy path” code without deeply nested `match`.

## Step-by-step: name the pipeline

```rex,interactive
let
  step1 = \x -> if x < 0 then Err "negative" else Ok (x + 1),
  step2 = \x -> Ok (x * 2),
  run = \x -> bind step2 (bind step1 x)
in
  (run (Ok 10), run (Ok (0 - 1)))
```

## `map` vs `bind`

Use `map` when your function does *not* fail and does *not* change the container:

```rex,interactive
map ((+) 1) (Ok 41)
```

Use `bind` when your function returns another `Result` (and might fail):

```rex,interactive
bind (\x -> if x < 0 then Err "negative" else Ok x) (Ok 1)
```

## Exercises

1. Change `step2` to fail when the value is too large.
2. Define a custom error ADT instead of using strings (e.g. `type Err = Negative | TooLarge`).
3. Write a helper `and_then` as a synonym for `bind` to make your code read more like English.
