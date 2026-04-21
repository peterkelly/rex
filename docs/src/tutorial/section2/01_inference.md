# Type Inference (Hindley–Milner)

Rex infers types for most expressions. You get type errors when constraints can’t be satisfied or
when the program would require ambiguous instance selection.

This section is intentionally practical: it’s about recognizing when the typechecker needs help,
and what kinds of help work best.

## A simple inference example

```rex,interactive
\x -> x
```

This is polymorphic: it can be used at any type (`a -> a`).

### Try it

Ask the CLI for the type:

```sh
cargo run -p rex-cli -- run --emit-type -c '\\x -> x'
```

## Inference plus operators

```rex,interactive
\x -> x + 1
```

This adds constraints (here, a numeric type class for `+`).

### What changed?

The expression no longer works at “any type” because `+` only exists for types with an
`AdditiveMonoid` instance (numbers and strings in the prelude).

## When inference fails

You’ll see errors when:

- branches of an `if` don’t match types,
- you call a type-class method with no applicable instance,
- you use an overloaded value without enough type information (ambiguity).

For details on ambiguity and defaulting, see [Specification](../../SPEC.md).

## The most common “fixes”

1. Add a type annotation to a `let` binding.
2. Use expression ascription with `is`.
3. Restructure code so an argument forces a type (e.g. apply a polymorphic function).
