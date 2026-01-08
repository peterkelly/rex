# Example: One `map`, many containers

This demonstrates deferred resolution of a `Functor` method value.

## Goal

Understand why one definition of `f` can work for lists, options, and results, and how to avoid
ambiguity when working with overloaded methods.

```rex
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

```rex
let f = map ((+) 1) in f
```

At this point, `f` is still a function, so Rex can keep it “overloaded”.

Now apply it to a list:

```rex
let f = map ((+) 1) in f [1, 2, 3]
```

At this call site, `f` must be `List i32 -> List i32`, so Rex selects `Functor List`.

## Contrast: ambiguous non-function values

Some overloaded values are ambiguous if you don’t force a type. For example, `pure 1` could be a
`List i32`, `Option i32`, `Array i32`, `Result i32 e`, etc.

Fix it by forcing a type:

```rex
let x: Option i32 = pure 1 in x
```

## Exercises

1. Replace `Ok 21` with `Err "boom"` and observe that `map` does not change the error.
2. Create `g = map (\x -> x * x)` and apply it to a list and an option.
3. Try writing `pure 1` without a type annotation and see the error; then fix it with an
   annotation.
