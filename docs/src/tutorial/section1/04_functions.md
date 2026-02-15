# Functions and Lambdas

Functions are values. The most common way to write one is a lambda.

## Lambdas

```rex,interactive
\x -> x + 1
```

Lambdas can take multiple arguments:

```rex,interactive
\x y -> x + y
```

Rex also accepts the Unicode spellings `λ` and `→`.

### Annotating lambda parameters

You can annotate parameters when you need to force a specific type:

```rex,interactive
\(x: i32) -> x + 1
```

## Application

Function application is left-associative:

```rex
f x y
```

is parsed as:

```rex
(f x) y
```

This is why parentheses are important when an argument is itself an application.

## Functions returning functions (currying)

```rex,interactive
let add = \x -> (\y -> x + y) in
  (add 1) 2
```

### Partial application

Because functions are curried, you can supply fewer arguments to get a new function back:

```rex,interactive
let add1 = (+) 1 in add1 41
```

## Top-level functions (`fn`)

Top-level functions require an explicit type signature:

```rex,interactive
fn add : i32 -> i32 -> i32 = \x y -> x + y
```

This declares a function that takes an `i32` and returns another function `i32 -> i32`.

Top-level `fn` declarations are mutually recursive, so they can reference each other:

```rex,interactive
fn even : i32 -> bool = \n ->
  if n == 0 then true else odd (n - 1)

fn odd : i32 -> bool = \n ->
  if n == 0 then false else even (n - 1)

even 10
```

### Legacy `fn` header forms

The parser still accepts older forms that put parameter names/types in the header:

```rex,interactive
fn inc (x: i32) -> i32 = x + 1
```

```rex,interactive
fn inc x: i32 -> i32 = x + 1
```

For multiple parameters, the “named arrows” form looks like:

```rex,interactive
fn add x: i32 -> y: i32 -> i32 = x + y
```

### `fn` constraints with `where`

Top-level functions can also have type-class constraints:

```rex,interactive
fn sum_list : List i32 -> i32 where Foldable List = \xs -> foldl (+) 0 xs
```

If you haven’t seen `where` constraints before, Section 2 covers them in detail.
