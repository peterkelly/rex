# Types and Annotations

Rex uses Hindley–Milner type inference, but you can (and often should) add annotations.

This page is about the “tools” you use to make types explicit when inference isn’t enough.

## Type names

Examples of primitive and constructed types:

```rex
-- bool, i32, f32, string
-- (a, b) for tuples
-- List a, Option a, Result a e (prelude)
```

Function types are right-associative:

```rex
i32 -> i32 -> i32
```

means:

```rex
i32 -> (i32 -> i32)
```

## Record types

Record types use `:`:

```rex
{ x: i32, y: i32 }
```

Record values use `=`:

```rex
{ x = 1, y = 2 }
```

## Let annotations

```rex
let x: i32 = 1 in x
```

## Lambda parameter annotations

```rex
\(x: i32) -> x + 1
```

## Annotating expressions

You can also annotate via a let-binding when you want to force a particular type:

```rex
let xs: List i32 = [1, 2, 3] in xs
```

## Type ascription with `is`

Rex also supports an expression-level “ascription” form:

```rex
({ a = 1, b = 2 }) is Dict i32
```

You’ll see `is` used in examples for two common reasons:

1. To force a dictionary type (`Dict a`) instead of a specific record type.
2. To disambiguate overloaded values (similar to adding a let-annotation).

> **Warning:** Use `is` when it helps clarity, but don’t overuse it: most of the time, simple `let` annotations are
> easier to read.
