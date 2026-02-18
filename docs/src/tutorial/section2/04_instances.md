# Instances: Implementing Type Classes

Instances attach method implementations to a concrete “head” type.

An instance has three parts:

1. The class name (`Show`, `Size`, `Functor`, …)
2. The instance head type (what you’re implementing it for)
3. An optional instance context (`<= ...`) of required constraints

## A monomorphic instance

```rex
class Show a
  show : a -> string

instance Show i32
  show = \_ -> "<i32>"
```

## A polymorphic instance with context

Instance contexts use `<=`:

```rex
instance Show (List a) <= Show a
  show = \xs ->
    let
      step = \out x ->
        if out == "["
          then out + show x
          else out + ", " + show x,
      out = foldl step "[" xs
    in
      out + "]"
```

Read this as: “`Show (List a)` exists as long as `Show a` exists”.

### Why the context matters

Inside `show` for lists, we call `show x`. That requires `Show a`, so we must list it in the
instance context.

## Non-overlap (coherence)

Rex rejects overlapping instance heads for the same class. This keeps method lookup deterministic.

In practical terms: you can’t have two different `Show (List a)` instances in scope at once.

