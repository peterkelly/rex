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

Rex only accepts the ASCII spellings `\` and `->`.

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

Top-level functions require parameter types, a return type, and a semicolon terminator. The
recommended form puts each parameter name next to its type in the `fn` header:

```rex,interactive
fn add x: i32 -> y: i32 -> i32 = x + y;
```

This declares a function that takes an `i32` and returns another function `i32 -> i32`. The
semicolon terminates the top-level declaration, so multi-line bodies do not depend on indentation.

Top-level `fn` declarations are mutually recursive, so they can reference each other:

```rex,interactive
fn even n: i32 -> Bool =
  if n == 0 then true else odd (n - 1);

fn odd n: i32 -> Bool =
  if n == 0 then false else even (n - 1);

even 10
```

### Alternative `fn` forms

Rex also accepts a parenthesized parameter in the header:

```rex,interactive
fn inc (x: i32) -> i32 = x + 1;
```

You can also write the full function type after the function name and provide a lambda body:

```rex,interactive
fn add : i32 -> i32 -> i32 = \x y -> x + y;
```

These are equivalent alternatives. The named-parameter header form is recommended because it keeps
parameter names and types adjacent to each other.

### `fn` constraints with `where`

Top-level functions can also have type-class constraints:

```rex,interactive
fn sum_list xs: List i32 -> i32 where Foldable List = foldl (+) 0 xs;
```

If you haven’t seen `where` constraints before, Section 2 covers them in detail.
