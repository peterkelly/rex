# Example: Custom `Show` type class

This mirrors `rex/examples/typeclasses_custom_show.rex`.

## Goal

Define your own show-printing API that turns values into `string` without baking formatting into
every call site.

We’ll build it up in layers:

1. define the class
2. add a base instance (`i32`)
3. add a structured type (`Point`)
4. (optional) add a container instance (`List a`)

```rex,interactive
class DemoShow a
  demo_show : a -> string

type Point = Point { x: i32, y: i32 }

instance DemoShow i32
  demo_show = \_ -> "<i32>"

instance DemoShow Point
  demo_show = \p -> "Point(" + demo_show p.x + ", " + demo_show p.y + ")"

demo_show (Point { x = 1, y = 2 })
```

## Extending it

Add an instance `DemoShow (List a) <= DemoShow a` (see `rex/examples/typeclasses_custom_show.rex`)
and call `demo_show [Point { x = 1, y = 2 }]`.

## A worked `DemoShow (List a)` instance

Here is the list instance from the repo example, with commentary:

```rex,interactive
class DemoShow a
  demo_show : a -> string

instance DemoShow i32
  demo_show = \_ -> "<i32>"

instance DemoShow (List a) <= DemoShow a
  demo_show = \xs ->
    let
      step = \out x ->
        if out == "["
          then out + demo_show x
          else out + ", " + demo_show x,
      out = foldl step "[" xs
    in
      out + "]"
```

### Why the `<= DemoShow a` constraint?

Because the implementation calls `demo_show x` for list elements, so it requires `DemoShow a`.

## Worked examples

### Example: use `"; "` as the list separator

Problem: format list output with semicolons.

```rex,interactive
class DemoShow a
  demo_show : a -> string

instance DemoShow i32
  demo_show = \_ -> "<i32>"

instance DemoShow (List a) <= DemoShow a
  demo_show = \xs ->
    let
      step = \out x ->
        if out == "["
          then out + demo_show x
          else out + "; " + demo_show x,
      out = foldl step "[" xs
    in
      out + "]"

demo_show [1, 2, 3]
```

Why this works: only the separator string changed; the fold structure stays the same.

### Example: `DemoShow bool`

Problem: add show-print support for booleans.

```rex,interactive
class DemoShow a
  demo_show : a -> string

instance DemoShow bool
  demo_show = \b -> if b then "true!" else "false!"

(demo_show true, demo_show false)
```

Why this works: the instance defines one method body specialized to `bool`.

### Example: `DemoShow (Option a)`

Problem: print `Some(...)` and `None` for options.

```rex,interactive
class DemoShow a
  demo_show : a -> string

instance DemoShow i32
  demo_show = \_ -> "<i32>"

instance DemoShow (Option a) <= DemoShow a
  demo_show = \ox ->
    match ox
      when Some x -> "Some(" + demo_show x + ")"
      when None -> "None"

(demo_show (Some 1), demo_show None)
```

Why this works: pattern matching distinguishes constructors and delegates formatting of payloads.
