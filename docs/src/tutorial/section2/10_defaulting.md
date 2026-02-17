# Defaulting and Ambiguous Numeric Types

Some numeric-like prelude operations (such as `zero`) only require type-class constraints:

```rex,interactive
zero
```

If nothing else forces a concrete type, Rex runs a defaulting pass to pick a sensible type (for a
set of defaultable numeric classes).

## How to recognize defaulting issues

If you see an “ambiguous overload” error and the expression involves numeric operations or `zero`,
you likely need to force a type.

## A common fix: add an annotation

When you want a specific type, annotate it:

```rex,interactive
let z: i32 = zero in z
```

Another common fix is to use the value in a way that forces a type:

```rex,interactive
zero + 1
```

Integer literals behave similarly: they are overloaded over `Integral` and become concrete when a
context requires a specific integer type.

```rex,interactive
let
  x = 4,
  f: u16 -> u16 = \n -> n
in
  f x
```

For negative literals, context must pick a signed type:

```rex,interactive
let
  x: i32 = -3,
  f: i32 -> i32 = \n -> n
in
  f x
```

`let x: u32 = -3 in x` is a type error.

## Learn the rules

Defaulting is specified precisely in [Specification](../../SPEC.md) (“Defaulting”). If you hit an “ambiguous
overload” error, that section explains why and how to resolve it.
