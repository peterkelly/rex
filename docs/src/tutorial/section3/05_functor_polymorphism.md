# Example: One `map`, many containers

This demonstrates deferred resolution of a `Functor` method value.

## Goal

Understand why one definition of `f` can work for lists, options, and results, and how to avoid
ambiguity when working with overloaded methods.

```rex,interactive
let f = map ((+) 1) in
  ( f [1, 2, 3]
  , f (Some 41)
  , f (Ok 21)
  )
```

## Why this is cool

`f` is a single definition that can be applied to different container types. Rex defers selecting
the `Functor` instance until you apply `f` to a concrete container.

## Step-by-step

Start by binding the method value:

```rex,interactive
let f = map ((+) 1) in f
```

At this point, `f` is still a function, so Rex can keep it “overloaded”.

Now apply it to a list:

```rex,interactive
let f = map ((+) 1) in f [1, 2, 3]
```

At this call site, `f` must be `List i32 -> List i32`, so Rex selects `Functor List`.

## Contrast: ambiguous non-function values

Some overloaded values are ambiguous if you don’t force a type. For example, `pure 1` could be a
`List i32`, `Option i32`, `Array i32`, `Result i32 e`, etc.

Fix it by forcing a type:

```rex,interactive
let x: Option i32 = pure 1 in x
```

## Worked examples

### Example: mapping over `Err`

Problem: verify that `map` does not change the error branch.

```rex,interactive
map ((+) 1) (Err "boom")
```

Why this works: `Functor (Result e)` maps only the `Ok` value, leaving `Err` unchanged.

### Example: one `g`, list and option

Problem: define `g = map (\x -> x * x)` once and apply it to multiple containers.

```rex,interactive
let g = map (\x -> x * x) in
  (g [1, 2, 3], g (Some 4))
```

Why this works: instance resolution for `map` is deferred until each concrete call site.

### Example: fixing ambiguous `pure 1`

Problem: choose a concrete container for `pure 1`.

```rex,interactive
let x: Option i32 = pure 1 in x
```

Why this works: the annotation forces `pure` to use the `Applicative Option` instance.
