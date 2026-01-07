# Writing Your Own Instances (Including Functor/Applicative/Monad)

This page is a hands-on guide to defining your own instances for custom ADTs.

We’ll build a tiny container type and give it `Functor`, `Applicative`, and `Monad` instances.

:::{note}
`Functor`, `Applicative`, and `Monad` are provided by the Rex prelude. The examples below assume
those classes already exist (so we only write `type`/`instance`).
:::

## Step 1: define a container ADT

```rex
type Box a = Box a
```

This is a single-variant ADT that “wraps” a value.

## Step 2: make it a Functor

```rex
instance Functor Box where
  map = \f bx ->
    match bx
      when Box x -> Box (f x)
```

Now you can:

```rex
map ((+) 1) (Box 41)
```

## Step 3: make it an Applicative

```rex
instance Applicative Box <= Functor Box where
  pure = \x -> Box x
  ap = \bf bx ->
    match bf
      when Box f -> map f bx
```

Try:

```rex
ap (Box ((*) 2)) (Box 21)
```

## Step 4: make it a Monad

```rex
instance Monad Box <= Applicative Box where
  bind = \f bx ->
    match bx
      when Box x -> f x
```

Try:

```rex
bind (\x -> Box (x + 1)) (Box 41)
```

## Common pitfalls

- **Overlapping instances**: Rex rejects overlap for the same class; keep instance heads distinct.
- **Missing context constraints**: if your method body calls another overloaded method, you often
  need to list the required class in the instance context.
- **Wrong argument order for `bind`**: Rex’s `bind` is `(a -> m b)` first, then `m a`.
