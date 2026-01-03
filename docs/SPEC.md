# Rex Spec (Locked Semantics)

This document records the *intended*, production-facing semantics of the current Rex implementation.
When behavior changes, this file and the corresponding regression tests should be updated together.

## Record Update

### Syntax

Record update is an expression:

```rex
{ base with { field1 = e1, field2 = e2 } }
```

### Typing

Let `base : T`. Record update is well-typed iff:

1. The update field set is non-empty, and each `fieldᵢ` exists on the target record shape.
2. `T` is one of:
   - a record type `{ field: Ty, ... }`, OR
   - a single-variant ADT whose payload is a record (e.g. `Foo = Bar { x: i32 }`), OR
   - a multi-variant ADT *after* the expression has been refined to a single variant by `match`
     (the type system tracks this refinement).
3. Each update expression `eᵢ` typechecks to a type that unifies with the declared field type.

If the base type is a multi-variant ADT and the typechecker cannot prove the current variant,
record update is rejected (the field is “not definitely available”).

### Evaluation

Evaluation is strict:

1. Evaluate `base` to a value.
2. Evaluate all update expressions.
3. Apply updates:
   - If `base` evaluates to a plain record/dict value, updates replace existing fields.
   - If `base` evaluates to an ADT whose payload is a record/dict, updates replace fields in the
     payload and re-wrap the ADT constructor tag.

## Type Classes: Coherence and Ambiguity

### Coherence (No Overlap)

At runtime, instance heads are **non-overlapping** per class:

- When injecting instances, a new instance head is rejected if it unifies with any existing head for
  the same class. This forbids overlap and preserves deterministic method resolution.

### Method Resolution

Class methods are resolved by unification against the inferred (monomorphic) call type:

- If the required instance parameter type is still headed by a type variable, the use is treated as
  ambiguous:
  - If the method is used as a function value, resolution is deferred via an overloaded value.
  - If the method is used as a value (non-function), the engine errors.
- If exactly one instance matches, its method body is specialized and evaluated.
- If none match, the engine errors.

### Instance-Method Checking

Inside an instance method body, only the instance context (plus superclass closure + the instance
head itself) is available as “given” constraints. Ground constraints are discharged by instance
search; non-ground constraints must appear explicitly in the instance context.

## Defaulting

Defaulting runs after type inference and before evaluation.

### When Defaulting Applies

A type variable `a` is eligible for defaulting iff:

- `a` appears only in *simple* class predicates of the form `C a` (no compound types), and
- every such `C` is in the defaultable set:
  `AdditiveMonoid`, `MultiplicativeMonoid`, `AdditiveGroup`, `Ring`, `Field`, `Integral`.

### Candidate Types and Selection

For each eligible type variable, the engine builds a candidate list:

1. All concrete (free-type-variable-empty) 0-arity type constructors that occur in the typed
   expression.
2. Then (if not already present): `f32`, `i32`, `string`.

The chosen default is the **first** candidate type that satisfies *all* required predicates
(`entails` succeeds in the empty context). If no candidate works, the variable remains ambiguous.

