# Instances: Implementing Type Classes

Instances attach method implementations to a concrete “head” type.

An instance has three parts:

1. The class name (`Pretty`, `Size`, `Functor`, …)
2. The instance head type (what you’re implementing it for)
3. An optional instance context (`<= ...`) of required constraints

## A monomorphic instance

```rex,interactive
class Pretty a
  pretty : a -> string

instance Pretty i32
  pretty = \_ -> "<i32>"
```

## A polymorphic instance with context

Instance contexts use `<=`:

```rex,interactive
instance Pretty (List a) <= Pretty a
  pretty = \xs ->
    let
      step = \out x ->
        if out == "["
          then out + pretty x
          else out + ", " + pretty x,
      out = foldl step "[" xs
    in
      out + "]"
```

Read this as: “`Pretty (List a)` exists as long as `Pretty a` exists”.

### Why the context matters

Inside `pretty` for lists, we call `pretty x`. That requires `Pretty a`, so we must list it in the
instance context.

## Non-overlap (coherence)

Rex rejects overlapping instance heads for the same class. This keeps method lookup deterministic.

In practical terms: you can’t have two different `Pretty (List a)` instances in scope at once.

