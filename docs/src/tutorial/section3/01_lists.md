# Example: List Basics

This page is a hand-held tour of the most common list workflow in Rex:

- start with a list value
- transform it with `map`
- keep only what you want with `filter`
- (optionally) reduce it with `foldl` (see the next page)

If you’re new to functional programming, think of `map` as “for each element, compute a new
element”, and `filter` as “keep only elements where the predicate is true”.

## Goal

Take a list of integers and produce new lists by applying simple rules (double, keep even,
increment).

## The simplest transform

## Double everything

```rex,interactive
map ((*) 2) [1, 2, 3, 4]
```

### What to notice

- `map` comes from `Functor List`.
- `(*)` is just a function; `((*) 2)` is a partially applied function.

## Step-by-step: naming intermediate values

The one-liner above is idiomatic, but while learning it helps to name each step:

```rex,interactive
let
  xs = [1, 2, 3, 4],
  doubled = map ((*) 2) xs
in
  doubled
```

This style also makes it easier to debug by temporarily returning an intermediate.

## Filter then map

Filtering needs a predicate `a -> bool`. Let’s define one:

```rex,interactive
let
  is_even = \x -> (x % 2) == 0
in
  filter is_even [1, 2, 3, 4, 5, 6]
```

Now combine `filter` and `map`:

```rex,interactive
let
  xs = [1, 2, 3, 4, 5, 6],
  is_even = \x -> (x % 2) == 0
in
  map ((+) 1) (filter is_even xs)
```

### Variations

Try changing the predicate to keep odd numbers instead:

```rex,interactive
let is_odd = \x -> (x % 2) != 0 in filter is_odd [1, 2, 3, 4, 5, 6]
```

## Common beginner mistake: missing parentheses

Because application is left-associative, nesting calls without parentheses does not do what you
want. Prefer:

```rex,interactive
map ((+) 1) (filter is_even xs)
```

over trying to “read it as English” without grouping.

## Exercises

1. Write `triple_then_keep_big` that triples each element, then keeps only elements greater than 10.
2. Write a predicate `between lo hi x` and use it with `filter`.
3. Replace `((+) 1)` with a named helper `inc` defined via `let`.
