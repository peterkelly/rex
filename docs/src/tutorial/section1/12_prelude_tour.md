# A Tour of the Prelude

Rex ships with a small “prelude” of standard types, type classes, and instances.

The source of truth in this repository is:

- type classes + instances: `rex-ts/src/prelude_typeclasses.rex`
- built-in types + helper functions: `rex-ts/src/prelude.rs`

This page is a guided map so you know what to reach for while writing Rex.

## Core data types

These are available by default:

- `List a` (with constructors `Empty` and `Cons`)
- `Option a` (constructors `Some` and `None`)
- `Result a e` (constructors `Err` and `Ok`)
- `Array a`
- `Dict a`

## Core classes (selected)

### Numeric-like classes

- `AdditiveMonoid a` (`zero`, `+`)
- `MultiplicativeMonoid a` (`one`, `*`)
- `Ring a`, `Field a`, `Integral a` (and friends)

### Equality and ordering

- `Eq a` (`==`, `!=`)
- `Ord a` (`cmp`, `<`, `<=`, `>`, `>=`)
- `Default a` (`default`)

`Default` gives you a value-level default for a type. It is separate from Rex's
defaulting pass, which resolves ambiguous type variables for defaultable classes.

### Collections and effects

- `Functor f` (`map`)
- `Applicative f` (`pure`, `ap`)
- `Monad m` (`bind`)
- `Foldable t` (`foldl`, `foldr`, `fold`)
- `Filterable f` (`filter`, `filter_map`)
- `Sequence f` (`take`, `skip`, `zip`, `unzip`)
- `Indexable t a` (`get`)

For tuples, use numeric projection like `.0` and `.1` instead of `get`.

## A few useful helper functions

The prelude also exposes some generic helpers (type-class-based):

- `sum`, `mean`, `count`, `min`, `max`
- `is_some`, `is_none` (for `Option`)
- `is_ok`, `is_err` (for `Result`)

## How to learn what something is

When you see an unfamiliar function:

1. Ask the CLI for its type: `cargo run -p rexlang-cli -- run --emit-type -c 'the_name'`
2. If it’s a type-class method, find the class in `rex-ts/src/prelude_typeclasses.rex`
3. If it’s a helper function, find it in `rex-ts/src/prelude.rs`

This workflow is especially helpful when you’re building your own abstractions.
