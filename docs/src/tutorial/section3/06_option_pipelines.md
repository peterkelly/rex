# Example: `Option` pipelines (Applicative + Monad)

`Option a` represents “a value that might not exist”.

## Goal

Write a multi-step computation that:

- fails early by producing `None`
- otherwise returns a final value wrapped in `Some`

Then rewrite it in three styles:

1. plain `match`
2. `bind` chaining (monadic)
3. `ap` application (applicative)

## Applicative: apply a wrapped function

```rex,interactive
ap (Some ((*) 2)) (Some 21)
```

If either side is `None`, the result is `None`.

## Monad: sequence steps with `bind`

```rex,interactive
let
  step1 = \x -> if x < 0 then None else Some (x + 1),
  step2 = \x -> Some (x * 2)
in
  bind step2 (bind step1 (Some 10))
```

### Refactoring tip

If you have many steps, name them:

```rex,interactive
let
  step1 = \x -> if x < 0 then None else Some (x + 1),
  step2 = \x -> Some (x * 2),
  run = \x -> bind step2 (bind step1 x)
in
  run (Some 10)
```

## The same logic using `match`

`bind` is convenience. Under the hood, it’s the same “if None, stop” flow you would write with
`match`:

```rex,interactive
let
  step1 = \x -> if x < 0 then None else Some (x + 1),
  step2 = \x -> Some (x * 2)
in
  match (Some 10)
    when None -> None
    when Some v1 ->
      match (step1 v1)
        when None -> None
        when Some v2 -> step2 v2
```

## When to use `ap` vs `bind`

- Use `ap` when you have independent optional pieces and want to apply a function if all exist.
- Use `bind` when the next step depends on the previous result.

## Worked examples

### Example: fail `step1` on `x == 0` too

Problem: update `step1` so non-positive input fails.

```rex,interactive
let
  step1 = \x ->
    if x < 0 then None else
    if x == 0 then None else
      Some (x + 1)
in
  (step1 10, step1 0, step1 (0 - 1))
```

Why this works: the additional guard handles zero before success.

### Example: `add2opt` via `ap` and `pure`

Problem: add two optional integers when both are present.

```rex,interactive
let
  add2opt = \ox oy -> ap (ap (pure (\x y -> x + y)) ox) oy
in
  (add2opt (Some 1) (Some 2), add2opt None (Some 2))
```

Why this works: `pure` lifts the function, then each `ap` applies one argument inside `Option`.

### Example: validate list values with `filter_map`

Problem: keep only non-negative values and increment them.

```rex,interactive
let
  validate = \x -> if x < 0 then None else Some (x + 1)
in
  filter_map validate [3, (0 - 1), 0, 5]
```

Why this works: `filter_map` drops `None` results and unwraps `Some` values into the output list.
