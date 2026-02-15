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

### Worked example: bracketed join

Problem: join strings with commas and wrap the result in brackets.

```rex,interactive
let
  step = \out x ->
    if out == "" then x else out + ", " + x,
  joined = foldl step "" ["a", "b", "c"]
in
  "[" + joined + "]"
```

Why this works: the fold builds `"a, b, c"` from left to right, then the final expression adds the
outer brackets.

## Using folds to compute “length”

You can compute list length by ignoring elements and incrementing a counter:

```rex,interactive
foldl (\n _ -> n + 1) 0 [10, 20, 30, 40]
```

## When to prefer `match` recursion vs `foldl`

Both are fine. Rules of thumb:

- Use `foldl` when you’re “reducing” to a single value (sum, count, join).
- Use explicit `match` recursion when you need more complex control flow.

## Worked examples

### Example: `product` with `foldl`

Problem: multiply all numbers in a list, starting from `1`.

```rex,interactive
foldl (*) 1 [2, 3, 4]
```

Why this works: `1` is the multiplicative identity, so each element is accumulated by multiplication.

### Example: `all` over booleans

Problem: check whether every boolean in a list is `true`.

```rex,interactive
let
  all = \xs -> foldl (\acc x -> acc && x) true xs
in
  (all [true, true, true], all [true, false, true])
```

Why this works: once `acc` becomes `false`, `acc && x` stays `false` for the rest of the fold.

### Example: `any` over booleans

Problem: check whether at least one boolean in a list is `true`.

```rex,interactive
let
  any = \xs -> foldl (\acc x -> acc || x) false xs
in
  (any [false, false, true], any [false, false, false])
```

Why this works: the accumulator starts `false` and flips to `true` as soon as any element is `true`.
