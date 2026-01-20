# Let Bindings and Scope

`let ... in ...` introduces local bindings.

Think of `let` as: “name some sub-expressions so you can reuse them and make types clearer”.

## One binding

```rex
let x = 1 + 2 in x * 10
```

## Multiple bindings

Bindings can be written on separate lines (typically separated by commas):

```rex
let
  x = 1 + 2,
  y = x * 3
in
  (x, y)
```

### Local helper functions

Because functions are values, `let` is the normal way to define local helpers:

```rex
let
  inc = \x -> x + 1,
  double = \x -> x * 2
in
  double (inc 10)
```

## Scope

Bindings are visible only in the `in` body (and later bindings):

```rex
let
  x = 10,
  y = x + 1
in
  y
```

## Recursive bindings

Rex supports writing recursive helpers via `let`. This is the easiest way to write loops:

```rex
let
  sum = \xs ->
    match xs
      when [] -> 0
      when x:xs -> x + sum xs
in
  sum [1, 2, 3, 4]
```

:::{tip}
If you’re coming from languages with `for` loops, think “write a recursive function + match on a
list” in Rex.
:::

## Let-polymorphism (preview)

Let bindings are generalized (HM let-polymorphism), so one binding can be used at multiple types:

```rex
let id = \x -> x in (id 1, id true, id "hi")
```

This is one of the core reasons to use `let`: it lets you build small reusable utilities without
constantly writing type annotations.
