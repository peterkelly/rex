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
and call `demo_pretty [Point { x = 1, y = 2 }]`.

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

## Worked examples

### Example: use `"; "` as the list separator

Problem: format list output with semicolons.

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
          else out + "; " + demo_pretty x,
      out = foldl step "[" xs
    in
      out + "]"

demo_pretty [1, 2, 3]
```

Why this works: only the separator string changed; the fold structure stays the same.

### Example: `DemoPretty bool`

Problem: add pretty-print support for booleans.

```rex,interactive
class DemoPretty a
  demo_pretty : a -> string

instance DemoPretty bool
  demo_pretty = \b -> if b then "true!" else "false!"

(demo_pretty true, demo_pretty false)
```

Why this works: the instance defines one method body specialized to `bool`.

### Example: `DemoPretty (Option a)`

Problem: print `Some(...)` and `None` for options.

```rex,interactive
class DemoPretty a
  demo_pretty : a -> string

instance DemoPretty i32
  demo_pretty = \_ -> "<i32>"

instance DemoPretty (Option a) <= DemoPretty a
  demo_pretty = \ox ->
    match ox
      when Some x -> "Some(" + demo_pretty x + ")"
      when None -> "None"

(demo_pretty (Some 1), demo_pretty None)
```

Why this works: pattern matching distinguishes constructors and delegates formatting of payloads.
