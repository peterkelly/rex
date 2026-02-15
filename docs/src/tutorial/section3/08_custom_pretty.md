# Example: Custom `Pretty` type class

This mirrors `rex/examples/typeclasses_custom_pretty.rex`.

## Goal

Define your own pretty-printing API that turns values into `string` without baking formatting into
every call site.

We’ll build it up in layers:

1. define the class
2. add a base instance (`i32`)
3. add a structured type (`Point`)
4. (optional) add a container instance (`List a`)

```rex,interactive
class Pretty a
  pretty : a -> string

type Point = Point { x: i32, y: i32 }

instance Pretty i32
  pretty = \_ -> "<i32>"

instance Pretty Point
  pretty = \p -> "Point(" + pretty p.x + ", " + pretty p.y + ")"

pretty (Point { x = 1, y = 2 })
```

## Extending it

Add an instance `Pretty (List a) <= Pretty a` (see `rex/examples/typeclasses_custom_pretty.rex`)
and try `pretty [Point { x = 1, y = 2 }]`.

## A worked `Pretty (List a)` instance

Here is the list instance from the repo example, with commentary:

```rex,interactive
class Pretty a
  pretty : a -> string

instance Pretty i32
  pretty = \_ -> "<i32>"

instance Pretty (List a) <= Pretty a
  pretty = \xs ->
    let
      step = \out x ->
        if out == "["
          then out + pretty x
          else out + ", " + pretty x,
      out = foldl step "[" xs
    in
      out + "]"
```

### Why the `<= Pretty a` constraint?

Because the implementation calls `pretty x` for list elements, so it requires `Pretty a`.

## Exercises

1. Change the list formatting to use `"; "` instead of `", "`.
2. Add a `Pretty bool` instance.
3. Add a `Pretty (Option a) <= Pretty a` instance that prints `Some(...)` and `None`.
