# Polymorphism and Let-Generalization

The most visible “HM feature” in Rex is that `let` bindings can be reused at multiple types.

If you’re new to HM languages, this is the big shift from “everything has one type” to “some
definitions are generic”.

## A polymorphic helper

```rex,interactive
let id = \x -> x in
  (id 1, id true, id "hi")
```

## Why lambdas aren’t generalized

Inside a lambda body, parameters are monomorphic unless you explicitly abstract:

```rex,interactive
\f ->
  let x = f 1 in
    f x
```

If `f` were required to work at multiple unrelated types, that would be rejected.

### Practical implication

If you want something reusable, `let`-bind it at the outer level:

```rex,interactive
let
  id = \x -> x,
  use = \x -> (id x, id x)
in
  use 1
```

## Practical tip

If a definition should be reusable, prefer `let`-binding it and giving it a clear name (and often a
type annotation).

In practice, you’ll use:

- `let` for reusable helpers,
- lambdas for “inline glue” (callbacks passed to `map`, `foldl`, `bind`, …),
- `fn` for API-like top-level functions with stable signatures.

