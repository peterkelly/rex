# Monads

Monads are about *sequencing* computations where the next step depends on the previous result.

In Rex, the core monad operation is `bind`:

```rex
class Monad m <= Applicative m
  bind : (a -> m b) -> m a -> m b
```

Note the argument order: function first, then the monadic value.

If you come from Haskell: Rex’s `bind` corresponds to `(>>=)` but with the arguments flipped.

## `Option` as a Monad

```rex
let
  safe_inc = \x -> Some (x + 1),
  step = \x -> bind safe_inc x
in
  step (Some 1)
```

More realistically, you inline the next step:

```rex
bind (\x -> Some (x + 1)) (Some 41)
```

### Why monads matter

With `Option`, monadic sequencing means “stop early if something is missing” without deeply nested
`match` expressions.

## `Result` as a Monad

`Result e a` is useful for short-circuiting on the first `Err`:

```rex
let
  ok = Ok 1,
  boom = Err "boom"
in
  ( bind (\x -> Ok (x + 1)) ok
  , bind (\x -> Ok (x + 1)) boom
  )
```

## “Do notation” without syntax

Rex doesn’t require special syntax. You can write sequencing explicitly with `bind` and lambdas:

```rex
bind (\x ->
  bind (\y ->
    pure (x + y)
  ) (Some 2)
) (Some 1)
```

:::{tip}
When your `bind` chains get hard to read, consider extracting the steps into named `let` bindings.
:::
