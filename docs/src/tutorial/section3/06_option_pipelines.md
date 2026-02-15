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

## Exercises

1. Modify `step1` to also fail when `x == 0`.
2. Write `add2opt : Option i32 -> Option i32 -> Option i32` using `ap` and `pure (\x y -> x + y)`.
3. Use `filter_map` to validate values in a list into an `Option` and keep only the successful
   ones.
