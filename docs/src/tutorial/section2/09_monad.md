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

```rex,interactive
let
  safe_inc = \x -> Some (x + 1),
  step = \x -> bind safe_inc x
in
  step (Some 1)
```

More realistically, you inline the next step:

```rex,interactive
bind (\x -> Some (x + 1)) (Some 41)
```

### Why monads matter

With `Option`, monadic sequencing means “stop early if something is missing” without deeply nested
`match` expressions.

## `Result` as a Monad

`Result a e` is useful for short-circuiting on the first `Err`:

```rex,interactive
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

```rex,interactive
bind (\x ->
  bind (\y ->
    pure (x + y)
  ) (Some 2)
) (Some 1)
```

> **Tip:** When your `bind` chains get hard to read, consider extracting the steps into named `let` bindings.

For example, the same logic with named steps:

```rex,interactive
let
  add_x_y = \x y -> pure (x + y),
  step_y = \x -> bind (add_x_y x) (Some 2),
  run = \mx -> bind step_y mx
in
  run (Some 1)
```
