# Expressions: Values and Control Flow

Rex is expression-oriented: everything produces a value.

This page introduces the “everyday” expression forms you’ll use constantly.

## Literals

```rex,interactive
( true
, false
, 123
, 3.14
, "hello"
)
```

Common primitive types are `bool`, `i32`, `f32`, `string` (plus `uuid`, `datetime` if enabled by
the host).

### Integers vs floats

`123` is an integer literal. It can specialize to any `Integral` type from context, and defaults to
`i32` when ambiguous.
`3.14` is a float literal and defaults to `f32`.

If you need to force a different numeric type, you can use an annotation (covered later).

```rex,interactive
( (4 is u8)
, (4 is i64)
, (-3 is i32)
)
```

### Negative numbers

Rex supports negative integer literals:

```rex,interactive
-420
```

Negative literals require a signed numeric type. For example, `(-3 is u8)` is a type error, while
`(-3 is i16)` is valid.

When you’re unsure about parsing, you can always write subtraction explicitly:

```rex,interactive
0 - 1
```

## If / then / else

`if` is an expression and must have an `else`:

```rex,interactive
let x = 10 in
  if x < 0 then "neg" else "non-neg"
```

### A common mistake

`if` requires both branches and they must have the same type:

```rex
-- Not OK: the branches disagree ("string" vs "i32")
if true then "yes" else 0
```

## Equality and comparisons

Comparisons are ordinary functions (usually from the prelude type classes):

```rex,interactive
( 1 == 2
, 1 != 2
, 1 < 2
, 2 >= 2
)
```

If you try to compare a type without an `Eq` / `Ord` instance, typechecking will fail.

## Working with strings

String concatenation uses `+` (via `AdditiveMonoid string`):

```rex,interactive
"Rex " + "rocks"
```

Because `+` is type-class-based, the same syntax also works for numeric addition.

## Grouping: parentheses are your friend

When in doubt, add parentheses—especially when mixing application and infix operators:

```rex,interactive
let f = \x -> x + 1 in
  f (1 + 2)
```
