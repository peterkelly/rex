# Higher-Kinded Types (and Partial Application)

This page explains the “advanced” type shape behind `Functor`, `Applicative`, and `Monad`.

## What is a “type constructor”?

Some types take type parameters:

- `List a`
- `Option a`
- `Result e a`

The bare names `List` and `Option` are *type constructors* (they still need an `a`).

In informal kind notation:

- `List : * -> *`
- `Option : * -> *`
- `Result : * -> * -> *`

## Why `Functor` talks about `f a`

The class is defined as:

```rex
class Functor f
  map : (a -> b) -> f a -> f b
```

`f` here stands for a unary type constructor like `List` or `Option`.

## `Result` is binary — so how can it be a `Functor`?

The prelude has an instance:

```rex
instance Functor (Result e)
  map = prim_map
```

`Result e` means: “fix the error type to `e`, leaving one type parameter for the `Ok` value”.

So `Result e` behaves like a unary type constructor:

- `Result e a` is “a result with error type `e` and value type `a`”
- `map` transforms the `Ok` value and leaves `Err` alone

## Recognizing partial application in types

Whenever you see something like `(Result e)` or `(Either e)` in other languages, think:

> “We pinned one type parameter to turn a multi-parameter type into a unary constructor.”

This idea shows up again for `Applicative (Result e)` and `Monad (Result e)`.

