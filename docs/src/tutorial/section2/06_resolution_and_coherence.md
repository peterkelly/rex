# Resolution, Coherence, and Ambiguity

Type-class methods in Rex are resolved based on the inferred type at the call site.

This page answers “why did the typechecker complain?” when you’re using overloaded methods.

## Coherence: why overlap is rejected

If two instances could match the same call, the runtime wouldn’t know which method to pick.
Rex rejects such overlaps per class.

## Deferred resolution for function values

Rex can keep an overloaded *function value* around and resolve it later when you apply it:

```rex
let f = map ((+) 1) in
  ( f [1, 2, 3]
  , f (Some 41)
  )
```

Here `map` is a `Functor` method. The engine picks the right `map` implementation when it sees the
argument type (`List i32` vs `Option i32`).

### Why this works

`map ((+) 1)` is still a function, so Rex can defer selecting the `Functor` instance until the
function is applied to a concrete container.

## Ambiguity for non-function values

If you use an overloaded method as a *non-function value* and the type is not determined,
resolution can be ambiguous and Rex will error.

For example, `pure 1` is ambiguous by itself because it could be `List i32`, `Option i32`,
`Array i32`, `Result i32 e`, etc.

Fix it by forcing a type:

```rex
let x: Option i32 = pure 1 in x
```

For the exact rules, see [Specification](../../SPEC.md) (“Type Classes: Coherence, Resolution, and Ambiguity”).
