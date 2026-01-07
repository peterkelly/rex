# Functions and Lambdas

Functions are values. The most common way to write one is a lambda.

## Lambdas

```rex
\x -> x + 1
```

Lambdas can take multiple arguments:

```rex
\x y -> x + y
```

Rex also accepts the Unicode spellings `λ` and `→`.

### Annotating lambda parameters

You can annotate parameters when you need to force a specific type:

```rex
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

```rex
let add = \x -> (\y -> x + y) in
  (add 1) 2
```

### Partial application

Because functions are curried, you can supply fewer arguments to get a new function back:

```rex
let add1 = (+) 1 in add1 41
```

## Top-level functions (`fn`)

Top-level functions require explicit parameter types and a result type:

```rex
fn add (x: i32) -> i32 -> i32 = \y -> x + y
```

This declares a function that takes an `i32` and returns another function `i32 -> i32`.

### `fn` parameter syntax (two forms)

The parser accepts both of these:

```rex
fn inc (x: i32) -> i32 = x + 1
```

```rex
fn inc x: i32 -> i32 = x + 1
```

For multiple parameters, the “named arrows” form looks like:

```rex
fn add x: i32 -> y: i32 -> i32 = x + y
```

### `fn` constraints with `where`

Top-level functions can also have type-class constraints:

```rex
fn sum_list xs: List i32 -> i32 where Foldable List = foldl (+) 0 xs
```

If you haven’t seen `where` constraints before, Section 2 covers them in detail.
