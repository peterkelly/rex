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
class DemoPretty a
  demo_pretty : a -> string

type Point = Point { x: i32, y: i32 }

instance DemoPretty i32
  demo_pretty = \_ -> "<i32>"

instance DemoPretty Point
  demo_pretty = \p -> "Point(" + demo_pretty p.x + ", " + demo_pretty p.y + ")"

demo_pretty (Point { x = 1, y = 2 })
```

## Extending it

Add an instance `DemoPretty (List a) <= DemoPretty a` (see `rex/examples/typeclasses_custom_pretty.rex`)
and try `demo_pretty [Point { x = 1, y = 2 }]`.

## A worked `DemoPretty (List a)` instance

Here is the list instance from the repo example, with commentary:

```rex,interactive
class DemoPretty a
  demo_pretty : a -> string

instance DemoPretty i32
  demo_pretty = \_ -> "<i32>"

instance DemoPretty (List a) <= DemoPretty a
  demo_pretty = \xs ->
    let
      step = \out x ->
        if out == "["
          then out + demo_pretty x
          else out + ", " + demo_pretty x,
      out = foldl step "[" xs
    in
      out + "]"
```

### Why the `<= DemoPretty a` constraint?

Because the implementation calls `demo_pretty x` for list elements, so it requires `DemoPretty a`.

## Exercises

1. Change the list formatting to use `"; "` instead of `", "`.
2. Add a `Pretty bool` instance.
3. Add a `DemoPretty (Option a) <= DemoPretty a` instance that prints `Some(...)` and `None`.
