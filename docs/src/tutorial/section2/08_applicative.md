# Applicatives

An `Applicative` is a `Functor` that can inject values and apply wrapped functions:

```rex,interactive
class Applicative f <= Functor f
  pure : a -> f a
  ap : f (a -> b) -> f a -> f b
```

Applicatives are great when you want to combine independent computations that live “in a
container”.

## `pure`

```rex,interactive
pure 1
```

The type depends on context. For example, this forces `Option`:

```rex,interactive
let x: Option i32 = pure 1 in x
```

### A common pattern: building up computations

Because functions are curried, you can apply step-by-step. For `Option`:

```rex,interactive
ap (ap (pure (\x y -> x + y)) (Some 1)) (Some 2)
```

## `ap` with `Option`

```rex,interactive
ap (Some ((+) 1)) (Some 41)
```

If either side is “missing”, the result is missing:

```rex,interactive
( ap None (Some 1)
, ap (Some ((+) 1)) None
)
```

## Applicative style

You can build up multi-argument computations by applying step-by-step:

```rex,interactive
ap (ap (pure (\x y -> x + y)) (Some 1)) (Some 2)
```
