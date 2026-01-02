# Rex Language Notes

Rex is a small, strongly-typed functional DSL with Hindley–Milner inference, ADTs, and
Haskell-style type classes.

This file is intentionally “notes” rather than a formal spec, but it covers the pieces that
matter for day-to-day use and for understanding the implementation.

## Programs

A Rex program is:
- zero or more declarations (`type`, `class`, `instance`, `fn`)
- followed by a single expression

## Expressions (Selected)

### Let-in

```rex
let x = 1 + 2, y = 3 in x * y
```

### Lambdas and Application

Application is left-associative: `f x y` parses as `(f x) y`.

```rex
let add = \\x y -> x + y in add 1 2
```

### Tuples, Lists, Dictionaries

```rex
(1, "hi", true)
[1, 2, 3]
{ a = 1, b = 2 }
```

### Records and Record Update

Records are dictionaries at the syntax level, with two important operations:

- Field projection: `x.field`
- Record update: `{ base with { field = expr } }`

```rex
let p = { x = 1, y = 2 } in
    ({ p with { x = 10 } }).x
```

Record-update is used heavily with ADT variants that carry record payloads:

```rex
type Sum = A { x: i32 } | B { x: i32 }

let s: Sum = A { x = 1 } in
match s
  when A {x} -> { s with { x = x + 1 } }
  when B {x} -> { s with { x = x + 2 } }
```

### Pattern Matching

```rex
match xs
  when Empty -> 0
  when Cons h t -> h
```

Patterns include variables, wildcards (`_`), tuples, list cons (`h:t`), and dict patterns.

## Declarations (Selected)

### ADTs

```rex
type Maybe a = Just a | Nothing
```

### Type Classes

Superclasses and instance contexts use `<=` (read “requires”):

```rex
class Default a
    default : a

instance Default (List a) <= Default a
    default = []
```

The `where` keyword is optional; class/instance method blocks are recognized by indentation and
by the `name : ...` / `name = ...` method head forms.

