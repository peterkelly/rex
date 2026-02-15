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

## Worked examples

### Example: fail `step2` when value is too large

Problem: make the second step return `Err` above a threshold.

```rex,interactive
let
  step1 = \x -> if x < 0 then Err "negative" else Ok (x + 1),
  step2 = \x -> if x > 20 then Err "too-large" else Ok (x * 2),
  run = \x -> bind step2 (bind step1 x)
in
  (run (Ok 10), run (Ok 25))
```

Why this works: `bind` short-circuits on either error source, including the new `step2` condition.

### Example: custom error ADT

Problem: replace string errors with structured errors.

```rex,interactive
type Err = Negative | TooLarge

let
  step1 = \x -> if x < 0 then Err Negative else Ok (x + 1),
  step2 = \x -> if x > 20 then Err TooLarge else Ok (x * 2),
  run = \x -> bind step2 (bind step1 x)
in
  (run (Ok 10), run (Ok 25), run (Ok (0 - 1)))
```

Why this works: error constructors carry precise machine-readable failure categories.

### Example: `and_then` synonym

Problem: define a helper that reads like “then”.

```rex,interactive
let
  and_then = \mx f -> bind f mx,
  safe_inc = \x -> if x < 0 then Err "negative" else Ok (x + 1)
in
  and_then (Ok 1) safe_inc
```

Why this works: `and_then` is just argument-reordered `bind`, so behavior is identical.
