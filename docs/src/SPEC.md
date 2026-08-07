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

## Lexical Comments

Rex comments are lexical trivia and are removed before parsing:

- `//` starts a line comment that runs to the next newline or end of file.
- `/* ... */` starts a block comment. Block comments may span lines.
- Nested block comments are not supported.
- The legacy `{- ... -}` spelling is ordinary syntax, not a comment.

## Character and String Literals

Character and string literals are decoded during lexing.

- Double-quoted (`"..."`) literals produce `String` values.
- Single-quoted (`'...'`) literals produce `Char` values and must decode to exactly one Unicode
  scalar value. `Char` has the same value domain as Rust's `char`: surrogate code points and values
  above `U+10FFFF` are rejected, while every valid Unicode scalar value is accepted.
- C-style simple escapes are supported: `\a`, `\b`, `\f`, `\n`, `\r`, `\t`, `\v`, `\\`, `\"`,
  `\'`, and `\?`.
- Octal escapes use one to three octal digits (`\0` through `\777`).
- Hex escapes use `\x` followed by one or more hexadecimal digits.
- Unicode escapes use `\u` followed by exactly four hexadecimal digits, or `\U` followed by exactly
  eight hexadecimal digits.
- Backslash-newline is a line continuation and produces no character.
- Unsupported or malformed escape sequences are lexical errors.
- At JSON boundaries, `Char` is represented by a JSON string containing exactly one Unicode scalar
  value.

### String Operations

String operations put selectors and modifiers first and the primary input value last, matching the
rest of the collection API. For example, `string_contains needle haystack`,
`string_split separator input`, and `string_replace needle replacement input` can be partially
applied to form reusable predicates or transformations.

- String positions are zero-based Unicode scalar indices, never UTF-8 byte offsets.
  `string_get index input` returns the scalar at `index`, while
  `string_slice start end input` uses a half-open `start..end` range. `string_get` returns `None`
  for an out-of-bounds index. `string_slice` returns `None` unless
  `0 <= start <= end <= length input`; an empty in-bounds range returns `Some ""`.
- `string_contains needle haystack`, `string_starts_with prefix input`, and
  `string_ends_with suffix input` perform literal substring tests. `string_find needle haystack`
  returns the first matching Unicode scalar index or `None`; an empty needle is found at index zero.
- `string_split separator input` splits at non-overlapping literal matches and preserves empty
  segments, including trailing segments. An empty separator produces an empty segment at each end
  and one string for every Unicode scalar in between. `string_join separator parts` inserts the
  separator only between adjacent elements.
- `string_replace needle replacement input` replaces every non-overlapping literal match. An empty
  needle inserts the replacement at every Unicode scalar boundary, including both ends.
- `string_trim`, `string_trim_start`, and `string_trim_end` remove Unicode whitespace as classified
  by Rust's `char::is_whitespace`. `string_to_lower` and `string_to_upper` apply Unicode case
  mappings and may change the number of scalar values.
- `string_to_chars` and `chars_to_string` convert losslessly between strings and lists of Unicode
  scalar values. `string_to_utf8` returns the UTF-8 bytes of a string. `utf8_to_string` returns
  `Some` for valid UTF-8 and `None` for an invalid byte sequence.

## Length

`length` returns a `u64` and is implemented for lists, dictionaries, and strings:

- Lists return their number of elements.
- Dictionaries return their number of entries.
- Strings return their number of Unicode scalar values, not their UTF-8 byte length or number of
  user-perceived grapheme clusters. For example, `length "h\u00e9\U0001F600" == 3` and
  `length "e\u0301" == 2`.

`Option` does not implement `Length`.

## Dictionaries

`Dict a` is an immutable mapping from `String` keys to values of one uniform type `a`. Runtime
dictionary and record field maps store keys as strings; compiler identifiers and statically known
record field names remain symbols only inside the compiler.

Dictionary iteration order is ascending lexicographic string order. This order is observable in
`dict_keys`, `dict_values`, `dict_entries`, and the collision behavior of `dict_map`.

The core operations have these semantics:

- `dict_get` returns `Some value` for a present key and `None` for an absent key.
- `dict_has` tests key presence.
- `dict_insert`, `dict_remove`, and `dict_update` return new dictionaries without changing their
  inputs. `dict_update key f` calls `f` with the current optional value; `Some value` in the result
  inserts or replaces the key, while `None` removes it.
- `dict_keys`, `dict_values`, and `dict_entries` return lexicographically ordered lists.
- `dict_from_entries` processes its input list from first to last, so the last tuple for a duplicate
  key wins.

`Dict` implements `Functor` and `Filterable`. `map`, `filter`, and `filter_map` apply their callbacks
to values only and preserve the corresponding input keys. Callback applications for different
entries may evaluate in parallel; callback completion order does not affect the result.

`dict_map` has type `((String, a) -> (String, b)) -> Dict a -> Dict b`. Its callback applications
may evaluate in parallel. After every callback completes, results are applied in the original
dictionary's lexicographic key order. If multiple callbacks return the same output key, the result
from the latest input key in that order wins.

`dict_filter` has type `((String, a) -> Bool) -> Dict a -> Dict a`. Its callback applications may
also evaluate in parallel. It preserves each accepted entry's original key and value.

## Primitive Host Types

The zero-arity primitive types are `u8`, `u16`, `u32`, `u64`, `i8`, `i16`, `i32`, `i64`,
`f32`, `f64`, `Bool`, `Char`, `String`, `UUID`, `Hash`, and `DateTime`. `Char` corresponds exactly
to Rust's `char` and implements `RexType`, `IntoRex`, and `FromRex`. The `Hash` type corresponds to a
`blake3::Hash` in Rust. At JSON boundaries, hash values are exactly 32 bytes encoded as hexadecimal
strings; Rex emits the canonical lowercase 64-character representation. `show` uses the same
representation. The `Parse Hash` instance accepts the same representation and returns `None` for
invalid input.

## Program Entry Points

A Rex source is a compilation unit with zero or more declarations and an optional
final expression. Entry-point execution uses a single program entry point:

- If the source defines a top-level `fn main`, that function is the entry point.
  It is an error for the same source to also contain a final expression.
- If the source does not define `main`, the final expression is treated as an
  implicit zero-argument entry point.
- If the source does not define `main` and has no final expression, entry-point
  execution is an error.

The CLI supplies arguments to an explicit `main` from a JSON object passed with
`--inputs`. The object keys must exactly match the `main` parameter names, and
each value is converted with `json_to_rex` using the corresponding parameter
type. JSON inputs require concrete parameter types. A runnable source without
`main` uses its final expression as an implicit entry point and conceptually has
the empty input object `{}`.

## Module Imports

Rex distinguishes between:

- program entry-point execution, and
- modules loaded by the import system.

When Rex source is loaded as a module via the module system, it must not contain a top-level
expression result. Host-backed Rust modules may also be returned by importers; they expose their
declared interface and native implementations without a Rex source body.

### Syntax

Top-level imports support three forms:

```rex
import foo.bar as Bar;
import foo.bar (*);
import foo.bar (x, y, z as q);
```

Rules:

- Import declarations are terminated by explicit semicolons.
- `import <module> as <Alias>` imports the module namespace and requires qualified access
  (`Alias.member`).
- `import <module> (*)` imports all exported values, types, and classes into unqualified scope.
- `import <module> (x, y as z)` imports selected exported values, types, and classes into
  unqualified scope.
- `as <Alias>` on the module and `(...)` import clauses are mutually exclusive.
- A module identity is a validated qualified name with one or more segments. Each segment starts
  with a letter or `_` and then contains only letters, digits, or `_`.
- The engine treats module identities as names in an abstract namespace. Mapping those names to
  `.rex` files, generated source, parsed ASTs, databases, or Rust host modules is importer policy.
- Imports are module names, not URLs, filesystem paths owned by the engine, or content hashes.

### Visibility and Exports

Only exported (`pub`) values, types, and classes are importable through `(*)` and item clauses.

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
- Source imports are parsed as `CompilationUnit`s, then converted into `CompilationPackage`s for
  module processing. Source-derived and prebuilt `CompilationPackage` imports are loaded through
  strongly connected component (SCC) loading of module interfaces, so cyclic source imports are
  supported.
- Rust modules returned by importers are installed lazily through the same named-module machinery as
  eager `Builder::inject_module`. They must be named modules matching the resolved module identity,
  not root/global modules, and they do not run nested Rex import graph loading.

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
- A syntactic lambda binding is a recursive function binding. Type annotations around the lambda do
  not change this classification.
- Non-lambda bindings are value bindings. They are initialized sequentially and may only reference
  earlier bindings in the same `let rec` group.
- Function bodies may reference any binding in the same `let rec` group.
- A value binding is rejected if it depends on itself, a later binding, or an earlier function whose
  body can reach a binding that is not initialized yet.

## Top-Level Declaration Terminators

Top-level `type`, `fn`, `declare fn`, and `import` declarations are terminated by explicit
semicolons:

```rex
import math.core as Math;
type Box a = Box a;
type Point = { x: i32, y: i32 };
fn inc x: i32 -> i32 = x + 1;
declare fn host_value : i32;
```

Rules:

- The semicolon terminates the declaration, not a nested type or expression itself.
- The terminating semicolon is found at top-level expression/type depth; semicolons nested inside
  parentheses, brackets, braces, or blocks do not terminate the declaration.
- Indentation and newlines do not delimit declarations.

## Explicit Type Parameters

Type variables used in annotations, constraints, class heads, or instance heads must be declared by
the syntactic form that binds them.

Examples:

```rex
type Box a = Box a;
fn id<a> x: a -> a = x;
declare fn host_id<a> x: a -> a;
let id<a>: a -> a = \x -> x in id 1
class Size a where { size : a -> i32; }
instance<a> Show (List a) <= Show a where { show = prim_show; }
```

Rules:

- A bare unknown type name is an error, even when it starts with a lowercase letter.
- Top-level `fn`, `declare fn`, named `let`, class methods, and instance methods bind type
  parameters with `<...>` after the value name.
- `type` and `class` declarations bind type parameters with whitespace after the declaration head.
- `instance` declarations bind type parameters with `<...>` immediately after `instance`.

## Named Record Aliases

### Syntax

A `type` declaration whose right-hand side begins with a record type declares a transparent record
alias rather than an ADT:

```rex
type Point = { x: i32, y: i32 };
type Tagged a = { tag: String, value: a };
```

### Typing and Runtime Representation

- Applying a record alias is equivalent to writing its expanded record type. Alias names do not
  make otherwise identical record shapes distinct.
- Alias parameters are substituted structurally, so `Tagged i32` expands to
  `{ tag: String, value: i32 }`.
- Aliases may refer to other aliases and ADTs. Cyclic aliases are rejected.
- A record alias introduces no value-level constructor and no runtime tag. Its values are ordinary
  record/dict values.
- A record literal checked against a record type receives the expected type of each field. This
  permits heterogeneous and nested literals such as
  `let user: { name: String, age: i32 } = { name = "Ada", age = 36 } in user`.

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

1. **Pattern matching**: within a `case K { ... } -> ...` arm, the scrutinee is known to be `K`.
2. **Let-bound known constructors**: when a variable is bound to a value constructed with a
   record-carrying constructor, the variable may carry “known variant” information forward.

This enables the common pattern:

```rex,interactive
type Sum = A { x: i32 } | B { x: i32 };

let s: Sum = A { x = 1 } in
match s with {
  case A {x} -> { s with { x = x + 1 } };
  case B {x} -> { s with { x = x + 2 } };
}
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
import dep as D;

instance D.Pick i32 where {
  pick = 7;
}
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

### Prelude Parsing

The prelude class `Parse a` provides `parse : String -> Option a`. It has instances for `Bool`,
`u8`, `u16`, `u32`, `u64`, `i8`, `i16`, `i32`, `i64`, `f32`, `f64`, `Char`, `UUID`, `Hash`, and
`DateTime`. A successful conversion returns `Some value`; malformed or out-of-range input returns
`None` rather than raising an evaluation error. Parsing a `Char` succeeds only when the input
contains exactly one Unicode scalar value.

The desired result type must be determined by context so that the corresponding `Parse` instance
can be selected, for example:

```rex
let port: Option u16 = parse "8080" in port
```

Regressions: `parse_returns_some_for_every_supported_type` and
`parse_returns_none_for_every_supported_type` (`rex/tests/typeclasses_system.rs`).

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

### Prelude List Additive Monoid

The prelude defines `AdditiveMonoid (List a)` for every element type `a`, with no constraint on
`a`. Its identity `zero` is `[]`, and `xs + ys` concatenates `xs` and `ys` in that order.

```rex,interactive
[1, 2, 3] + [4, 5, 6]
```

Regression: `additive_monoid_list_concatenates_in_order` and
`additive_monoid_list_requires_no_element_constraint` (`rex/tests/typeclasses_system.rs`).

### Prelude Ordering

The prelude defines the algebraic data type `Ordering` with exactly three unit variants:
`Less`, `Equal`, and `Greater`. The `Ord` method `cmp : a -> a -> Ordering` returns the variant that
describes the left operand relative to the right operand. Floating-point comparisons involving
NaN remain runtime type errors.

Regression: `ord_cmp_returns_ordering_variants` and `ordering_variants_can_be_pattern_matched`
(`rex/tests/typeclasses_system.rs`).

### Prelude List Ranges

The prelude exposes strict list range helpers:

- `first n xs` returns the first `n` visible elements of `xs`.
- `last n xs` returns the last `n` visible elements of `xs`.
- `slice n m xs` returns the half-open visible range `n..m`.

For all three helpers, list positions are counted from the Rex-level list view, independent of
whether the runtime stores the list as cons cells, a vector-backed slice, or cons cells followed by
a vector-backed slice. Bounds are checked at runtime. Negative bounds, bounds greater than the list
length, and `slice n m xs` with `m < n` are runtime errors. `n == length`/`m == length` is valid for
empty suffixes and half-open slice endpoints.

The `Sequence` methods `take` and `skip` use a `u64` count. Counts greater than the list length are
clamped to the list length, so `take` returns the whole list and `skip` returns an empty list.

The list-specific operations use `u64` positions and return ordinary data failures as options:

- `list_get index xs` returns `Some` for an in-bounds zero-based index and `None` otherwise.
- `list_slice start end xs` returns `Some` of the half-open range `start..end` when
  `0 <= start <= end <= length xs`, including `Some []` for an empty valid range, and `None` for
  invalid bounds.
- `list_reverse`, `list_concat`, and `list_repeat` preserve element values and construct a new list
  in the order implied by their names. `list_repeat 0 value` returns `[]`.
- `list_any`, `list_all`, `list_find`, and `list_find_index` evaluate predicates from left to right
  and stop once their result is known. On an empty list, `list_any` is `false`, `list_all` is
  `true`, and both find operations return `None`.
- `list_count` and `list_partition` evaluate every predicate application. Independent applications
  may run concurrently. `list_partition` returns matching elements first and rejected elements
  second, preserving relative input order in both lists.

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
- Binary subtraction uses `Subtractive`, which includes unsigned integer types. Unary negation still
  requires `AdditiveGroup`.
- Division uses `Divisive`, which includes primitive integer and floating-point types. Integer
  division follows Rust's integer division semantics.
- Integer `+`, `-`, `*`, `/`, and `%` are checked at runtime for primitive integer types. Arithmetic
  overflow raises `integer overflow (T)` and arithmetic underflow raises `integer underflow (T)`,
  where `T` is the concrete integer type.
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

## Implicit Integer Widening

Rex inserts an implicit integer widening conversion only when the surrounding expression context
already requires a concrete primitive integer type.

- The source and target must both be primitive integer types.
- The conversion must be lossless for every value of the source type.
- The target type must already be known; Rex does not infer a common numeric type for unconstrained
  mixed-width expressions.

Allowed widening conversions are:

- `i8 -> i16`, `i8 -> i32`, `i8 -> i64`
- `i16 -> i32`, `i16 -> i64`
- `i32 -> i64`
- `u8 -> u16`, `u8 -> u32`, `u8 -> u64`, `u8 -> i16`, `u8 -> i32`, `u8 -> i64`
- `u16 -> u32`, `u16 -> u64`, `u16 -> i32`, `u16 -> i64`
- `u32 -> u64`, `u32 -> i64`

Examples:

```rex
fn f : i32 -> i32 = \x -> x;
fn g : i8 -> i8 = \x -> x;

let x: i32 = (7 is i8) in (f (g 5), x)
```

Mixed operators remain homogeneous unless an enclosing context fixes the target type. For example,
`(1 is i8) + (2 is i32)` is a type error.

## Float Literals

Float literals are overloaded over primitive floating-point types.

- A literal like `3.0` introduces a fresh type variable `α` with predicate `Field α`.
- Context can specialize `α` to `f32` or `f64` (for example,
  `let x: f64 = 3.0 in x`).
- If `α` remains ambiguous, normal defaulting rules choose `f32`.
- Unannotated `let` bindings whose definition is a float literal are kept monomorphic, matching
  integer literal bindings.
- Float literals do not imply integer-to-float coercions. A mixed expression such as `1 + 2.0`
  is still a type error unless the values are explicitly converted by user code.

Examples:

```rex
let x: f64 = 3.0 in x
let f: f64 -> f64 = \x -> x in f 3.0
let x = 3.0 in (x is f32)
```

## Defaulting

Defaulting runs after type inference and before evaluation.

### Eligible Variables

A type variable `α` is eligible for defaulting iff:

- `α` appears in at least one *simple* predicate of the form `C α`, and at least one such `C` is
  in the numeric defaultable set:
  `AdditiveMonoid`, `MultiplicativeMonoid`, `Subtractive`, `AdditiveGroup`, `Ring`, `Divisive`,
  `Field`, `Integral`; and
- every simple predicate involving `α` is either in that numeric set or is an allowed companion.

`Eq` and `Ord` are allowed as companion predicates when a numeric defaultable predicate is also
present. They do not make a type variable defaultable on their own. This lets expressions like
`if x == 0.0 then ...` default through the float literal's `Field` predicate without making
unconstrained equality default to an arbitrary numeric type.

Compound predicates do not make a variable eligible for defaulting. They also do not prevent an
otherwise eligible numeric variable from defaulting, provided that substituting a candidate makes
each compound predicate ground and the resulting predicate is satisfied. A candidate is rejected
if substitution leaves another unresolved variable in one of those predicates.

### Candidate Types (Order Matters)

The candidate list is constructed in this order:

1. Traverse the typed expression (depth-first) and collect every **concrete** (ground) 0-arity type
   constructor that appears as the type of a subexpression (unique, in first-seen order).
2. Append (if not already present): `f32`, `i32`, `String`.

### Choosing a Default

For an eligible variable `α`, choose the first candidate type `T` such that substituting `T` for
`α` makes every predicate involving `α` ground and satisfied in the empty context:

```text
entails([], Pᵢ[α := T]) for every predicate Pᵢ involving α
```

If no candidate satisfies all predicates, `α` remains ambiguous.

Example: `zero` (type `α` with `AdditiveMonoid α`) defaults to `f32` when no other concrete type is
present:

```rex,interactive
zero
```

Regression: `spec_defaulting_picks_a_concrete_type_for_numeric_classes` (`rex/tests/spec_semantics.rs`).

For example, the integer literals below provide the simple predicate `Integral α`, while list
addition provides the compound predicate `AdditiveMonoid (List α)`. Substituting `i32` satisfies
both, so the result type is `List i32`:

```rex,interactive
[1, 2, 3] + [4, 5, 6]
```

Regressions: `spec_defaulting_accepts_satisfied_compound_predicates` and
`spec_defaulting_requires_a_simple_numeric_predicate` (`rex/tests/spec_semantics.rs`).
