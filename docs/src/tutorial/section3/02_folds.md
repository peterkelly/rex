# Example: Folding

`Foldable` gives you `foldl` and `foldr`-style iteration.

If `map` changes values, folds *reduce* a collection down to a single result.

## Goal

Learn to take a list and reduce it to:

- a number (sum, product, count)
- a string (joining)

## A mental model: accumulator + step

`foldl` has the shape:

```text
foldl : (b -> a -> b) -> b -> t a -> b
```

Read it as:

1. Start with an accumulator of type `b`
2. For each element of type `a`, update the accumulator
3. Return the final accumulator

## Sum a list

```rex,interactive
foldl (+) 0 [1, 2, 3, 4]
```

### How to read it

- Start with accumulator `0`
- For each element, add it to the accumulator
- Return the final accumulator

### The same thing, spelled out

```rex,interactive
let
  step = \acc x -> acc + x
in
  foldl step 0 [1, 2, 3, 4]
```

When debugging, spelling out `step` makes it easier to reason about types.

## Build a string

```rex,interactive
let
  step = \out x ->
    if out == "" then x else out + ", " + x
in
  foldl step "" ["a", "b", "c"]
```

### Exercise

Modify this to wrap the output in brackets: `"[a, b, c]"`.

## Using folds to compute “length”

You can compute list length by ignoring elements and incrementing a counter:

```rex,interactive
foldl (\n _ -> n + 1) 0 [10, 20, 30, 40]
```

## When to prefer `match` recursion vs `foldl`

Both are fine. Rules of thumb:

- Use `foldl` when you’re “reducing” to a single value (sum, count, join).
- Use explicit `match` recursion when you need more complex control flow.

## Exercises

1. Write `product` using `foldl` and `(*)` starting from `1`.
2. Write `all` that checks whether all booleans in a list are true.
3. Write `any` that checks whether any booleans in a list are true.
