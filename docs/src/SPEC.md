# Rex Spec (Locked Semantics)

This document records the *intended*, production-facing semantics of the current Rex implementation.
When behavior changes, this file and the corresponding regression tests should be updated together.

Regression tests live in:

- `rex/tests/spec_semantics.rs`
- `rex/tests/record_update.rs`
- `rex/tests/typeclasses_system.rs`
- `rex/tests/negative.rs`

## Notation

- `Γ ⊢ e : τ` means “under type environment `Γ`, expression `e` has type `τ`”.
- `C τ` means a typeclass predicate (constraint) for class `C` at type `τ`.
- “Ground” means “contains no free type variables” (`ftv(τ) = ∅`).
- Rex’s multi-parameter classes are represented internally by packing the parameters into tuples:
  - unary `C a` is `Predicate { class: C, typ: a }`
  - binary `C t a` is `Predicate { class: C, typ: (t, a) }`
  - etc.

## Module Imports

Rex distinguishes between:

- program/snippet execution (declarations + one expression), and
- module files used by the import system (declaration-only).

When a `.rex` file is loaded as a module via the module system, it must not contain a top-level
expression result.

### Syntax

Top-level imports support three forms:

```rex
import foo.bar as Bar
import foo.bar (*)
import foo.bar (x, y, z as q)
```

Rules:

- `import <module> as <Alias>` imports the module namespace and requires qualified access
  (`Alias.member`).
- `import <module> (*)` imports all exported **values** into unqualified scope.
- `import <module> (x, y as z)` imports selected exported **values** into unqualified scope.
- `as <Alias>` on the module and `(...)` import clauses are mutually exclusive.

### Visibility and Exports

Only exported (`pub`) values are importable through `(*)` and item clauses.

Module aliases expose all export namespaces for qualified lookup:

- `Alias.value` resolves against exported values (including constructors).
- `Alias.Type` resolves against exported type names in type positions.
- `Alias.Class` resolves against exported class names in class-constraint positions.

- Missing requested exports are module errors.
- Private (non-`pub`) values are not importable.

### Name Binding and Conflicts

- Imported unqualified names participate in lexical shadowing.
- Lexically bound names (lambda params, `let` vars, pattern bindings) shadow imported names.
- Importing a name that conflicts with a local top-level declaration is a module error.
- Importing the same unqualified name more than once (including via aliasing) is a module error.

Type/class rewrites run with declaration ordering semantics:

- In binder forms that carry type syntax (`\ (x : T) -> ...`, `let rec f : T = ...`), the
  binder being introduced does not suppress alias resolution inside its own annotation.
- Missing alias members used in type/class positions (function signatures, annotations, `where`
  constraints, instance headers, and superclass clauses) are reported as module errors.

### Module Initialization

- Importing a module does not execute arbitrary top-level expressions.
- Module initialization is declaration-driven: exported values/types/classes are registered from
  declarations, and import resolution rewrites references to canonical internal symbols.
- Cyclic imports are supported via strongly connected component (SCC) loading of module interfaces.

## Let Rec Bindings

### Syntax

Recursive bindings use `let rec` with comma-separated entries:

```rex
let rec
  a = ...,
  b = ...
in
  body
```

Rules:

- `let rec` entries are separated by commas.
- `let rec` bindings must bind variables (not arbitrary patterns).

## Top-Level `fn` Recursion

Top-level `fn` declarations are mutually recursive within a module.

This means:

- A top-level `fn` may reference itself.
- A top-level `fn` may reference other top-level `fn` declarations in the same module, regardless of
  declaration order.

Operationally, top-level `fn` recursion follows the same fixed-point semantics as recursive
bindings in `let rec`, but at declaration scope.

## Record Projection

### Syntax

Field projection is an expression:

```rex
base.field
```

### Typing (Definite Fields)

Let `Γ ⊢ base : T`. Projection is well-typed iff the field is *definitely available* on `T`:

1. If `T` is a record type `{ ..., field : τ, ... }`, then `Γ ⊢ base.field : τ`.
2. If `T` is a single-variant ADT whose payload is a record containing `field : τ`, then
   `Γ ⊢ base.field : τ`.
3. If `T` is a multi-variant ADT, projection is accepted only if the typechecker can prove the
   current constructor is a specific record-carrying variant (typically via `match` refinement or
   by tracking known constructors through let-bound variables).

If the typechecker cannot prove the constructor for a multi-variant ADT, the field is considered
“not definitely available”, and projection is rejected.

### Evaluation

Evaluation is strict in `base`. At runtime, projection reads the field out of the record payload:

- for plain records/dicts, it indexes the map by the field symbol.
- for record-carrying ADT values, it indexes the payload record/dict.

Missing fields are a runtime error (`EngineError::UnknownField`) when projection is attempted on a
non-record-like value.

## Record Update

### Syntax

Record update is an expression:

```rex
{ base with { field1 = e1, field2 = e2 } }
```

### Typing (Definite Fields)

Let `Γ ⊢ base : T`. Record update is well-typed iff:

1. Each updated field exists on the *definite* record shape of `T`.
2. `T` is one of:
   - a record type `{ field: Ty, ... }`, OR
   - a single-variant ADT whose payload is a record, OR
   - a multi-variant ADT *after* the expression has been refined to a specific record-carrying
     constructor (the typechecker tracks this refinement).
3. For each update `fieldᵢ = eᵢ`, the update expression unifies with the declared field type.

If the base type is a multi-variant ADT and the typechecker cannot prove the current constructor,
record update is rejected (the field is “not definitely available”).

### Typing: Known-Constructor Refinement

The typechecker refines “which constructor is known” via two mechanisms:

1. **Pattern matching**: within a `when K { ... } -> ...` arm, the scrutinee is known to be `K`.
2. **Let-bound known constructors**: when a variable is bound to a value constructed with a
   record-carrying constructor, the variable may carry “known variant” information forward.

This enables the common pattern:

```rex,interactive
type Sum = A { x: i32 } | B { x: i32 }

let s: Sum = A { x = 1 } in
match s
  when A {x} -> { s with { x = x + 1 } }
  when B {x} -> { s with { x = x + 2 } }
```

### Evaluation

Evaluation is strict:

1. Evaluate `base` to a value.
2. Evaluate all update expressions (left-to-right in the implementation’s map iteration order).
3. Apply updates:
   - If `base` is a plain record/dict value, updates replace existing fields.
   - If `base` is an ADT whose payload is a record/dict, updates replace fields in the payload and
     re-wrap the constructor tag.

Runtime errors:

- Updating a non-record-like runtime value is `EngineError::UnsupportedExpr`.

## Type Classes: Coherence, Resolution, and Ambiguity

### Instance Coherence (No Overlap)

For each class `C`, instance heads are **non-overlapping**:

- When injecting a new instance head `H`, it is rejected if it unifies with any existing head for
  the same class `C`.

This forbids overlap and preserves deterministic method resolution.

Regression: `spec_typeclass_instance_overlap_is_rejected` (`rex/tests/spec_semantics.rs`).

### Qualified Class Names in `instance` Headers

The class name in an instance header may be qualified through a module alias:

```rex
import dep as D

instance D.Pick i32 where
  pick = 7
```

The alias member must be an exported class from the referenced module; otherwise import-use
validation fails before typechecking/evaluation.

### Method Resolution (Runtime)

At runtime, class methods are resolved by unification against the inferred call type.

Let `m` be a class method, and let its call site be typed with monomorphic call type `τ_call`.

Resolution:

1. Determine the “instance parameter type” for the method by unifying `τ_call` with the method’s
   scheme and extracting the predicate corresponding to the method’s defining class.
2. If the instance parameter type is still headed by a type variable (not ground enough to pick an
   instance), the use is ambiguous:
   - If `m` is used as a *function value* (i.e. `τ_call` is a function type), the engine returns an
     overloaded function value and defers resolution until the function is applied with concrete
     arguments.
   - If `m` is used as a *value* (non-function), the engine errors (`EngineError::AmbiguousOverload`).
3. If exactly one instance head unifies with the instance parameter type, its method body is
   specialized and evaluated.
4. If none match, the engine errors (`EngineError::MissingTypeclassImpl`).
5. If more than one match (should not occur given non-overlap), the engine errors
   (`EngineError::AmbiguousTypeclassImpl`).

Regression: `spec_typeclass_method_value_without_type_is_ambiguous` (`rex/tests/spec_semantics.rs`).

### Overloaded Method Values (Deferred Resolution)

If a class method is used as a *function value*, the engine may defer instance selection until the
function is applied with concrete argument types. This supports idioms like:

```rex,interactive
let f = map ((+) 1) in
  ( f [1, 2, 3]
  , f (Some 41)
  )
```

Here `f` is polymorphic over the `Functor` dictionary; at each call site, the engine resolves
`map` using the argument type (`List i32` vs `Option i32`) and dispatches to the corresponding
instance method body.

### Instance-Method Checking (Static)

Inside an instance method body, only the instance context is available as “given” constraints:

- Given predicates start with the instance’s explicit context.
- The superclass closure of that context is added (repeat until fixed point).
- The instance head itself is also considered given (dictionary recursion).

Rules:

- Ground predicates required by the method body must be entailed by the given set (via instance
  search).
- Non-ground predicates are **not** resolved by instance search (that would be unsound); they must
  appear explicitly in the instance context.

This is what makes instance methods predictable and prevents “magical” selection based on unifying
type variables with arbitrary instance heads.

## Integer Literals

Integer literals are overloaded over integral types.

- A literal like `4` introduces a fresh type variable `α` with predicate `Integral α`.
- A negative literal like `-3` introduces `α` with predicates `Integral α` and `AdditiveGroup α`
  (so it can only specialize to signed numeric types).
- Context can specialize `α` (for example, `let x: u64 = 4 in x`).
- Unannotated `let` bindings whose definition is an integer literal are kept monomorphic. This lets
  use sites specialize the binding consistently in that scope (for example, `let x = 4 in (x + 1,
  x + 2)`).
- If `α` remains ambiguous, normal defaulting rules apply.

Examples:

```rex
let x: u8 = 4 in x
let f: i64 -> i64 = \x -> x in f 4
let x = 4 in (x is u16)
let x: i16 = -3 in x
```

Attempting to use a negative literal at an unsigned type is a type error (for example
`let x: u8 = -3 in x`).

## Defaulting

Defaulting runs after type inference and before evaluation.

### Eligible Variables

A type variable `α` is eligible for defaulting iff:

- `α` appears only in *simple* predicates of the form `C α` (not in compound types), and
- every such `C` is in the defaultable set:
  `AdditiveMonoid`, `MultiplicativeMonoid`, `AdditiveGroup`, `Ring`, `Field`, `Integral`.

If `α` appears in any non-simple predicate or any non-defaultable class predicate, it is not
defaulted.

### Candidate Types (Order Matters)

The candidate list is constructed in this order:

1. Traverse the typed expression (depth-first) and collect every **concrete** (ground) 0-arity type
   constructor that appears as the type of a subexpression (unique, in first-seen order).
2. Append (if not already present): `f32`, `i32`, `string`.

### Choosing a Default

For an eligible variable `α` with required predicates `{ C₁ α, ..., Cₙ α }`, choose the first
candidate type `T` such that all predicates are satisfied in the empty context:

```text
entails([], Cᵢ T) for all i
```

If no candidate satisfies all predicates, `α` remains ambiguous.

Example: `zero` (type `α` with `AdditiveMonoid α`) defaults to `f32` when no other concrete type is
present:

```rex,interactive
zero
```

Regression: `spec_defaulting_picks_a_concrete_type_for_numeric_classes` (`rex/tests/spec_semantics.rs`).
