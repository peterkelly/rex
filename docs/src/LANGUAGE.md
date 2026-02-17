# Rex Language Guide

Rex is a small, strongly-typed functional DSL with:

- Hindley–Milner type inference (let-polymorphism)
- algebraic data types (ADTs), including record-carrying constructors
- Haskell-style type classes (including higher-kinded classes like `Functor`)

This guide is meant for users and embedders. For locked/production-facing semantics and edge cases,
see [SPEC.md](SPEC.md).

## A Program

A Rex program consists of:

- zero or more declarations (`type`, `class`, `instance`, `fn`, `import`)
- followed by a single expression (the program result)

Example:

```rex,interactive
fn inc : i32 -> i32 = \x -> x + 1

let
  xs = [1, 2, 3]
in
  map inc xs
```

## Modules and Imports

Rex modules are `.rex` files. Imports are top-level declarations.

Supported forms:

```rex
import foo.bar as Bar
import foo.bar (*)
import foo.bar (x, y as z)
```

Semantics:

- `import foo.bar as Bar` imports a module alias; use qualified access (`Bar.name`).
- `import foo.bar (*)` imports all exported values into local unqualified scope.
- `import foo.bar (x, y as z)` imports selected exported values; `y` is bound locally as `z`.
- Module alias imports and clause imports are mutually exclusive in one import declaration.
- Only `pub` values are importable from another module.
- If two imports introduce the same unqualified name (including via `(*)`), resolution fails with
  a module error.
- Importing a name that conflicts with a local top-level declaration is a module error.
- Lexical bindings (`let`, lambda params, pattern bindings) can shadow imported names.

Path resolution:

- `foo.bar` resolves to `foo/bar.rex`.
- Local module paths resolve relative to the importing file.
- Leading `super` path segments walk up directories (for example `super.core.calc`).

## Lexical Structure

### Whitespace and Comments

- Whitespace (including newlines) is generally insignificant.
- Comments use `{- ... -}` and are stripped before parsing.
- Nested block comments are not supported in current Rex builds.

### Identifiers and Operators

- Identifiers start with a letter or `_`, followed by letters/digits/underscores.
- Operators are non-alphanumeric symbol sequences (`+`, `*`, `==`, `<`, …).
- Operators can be used as values by parenthesizing: `(+)`, `(==)`, `(<)`.

### Lambdas

The lambda syntax is `\x -> expr`. Some docs/examples may also use Unicode `λ` and `→`.

## Expressions

### Literals

- `true`, `false`
- integers and floats (integer literals are overloaded over `Integral` and default to `i32` when ambiguous)
- strings: `"hello"`
- UUID and datetime literals (if present in your lexer source)

Examples:

```rex,interactive
( (4 is u8)
, (4 is u64)
, (4 is i16)
, (-3 is i16)
)
```

Negative literals only specialize to signed types. For example, `(-3 is u8)` is a type error.

### Function Application

Application is left-associative: `f x y` parses as `(f x) y`.

```rex,interactive
let add = \x y -> x + y in add 1 2
```

### Let-In

Let binds one or more definitions and then evaluates a body:

```rex,interactive
let
  x = 1 + 2,
  y = 3
in
  x * y
```

Let bindings are polymorphic (HM “let-generalization”):

```rex,interactive
let id = \x -> x in (id 1, id true, id "hi")
```

Integer-literal bindings are a special case: unannotated `let x = 4` is kept monomorphic so use
sites can specialize it through context.

```rex,interactive
let
  x = 4,
  f: u8 -> u8 = \y -> y
in
  f x
```

### Recursive Let (`let rec`)

Use `let rec` for self-recursive and mutually-recursive bindings.

```rex,interactive
let rec
  even = \n -> if n == 0 then true else odd (n - 1),
  odd = \n -> if n == 0 then false else even (n - 1)
in
  (even 10, odd 11)
```

Notes:

- Bindings in `let rec` are separated by commas.

### If-Then-Else

```rex,interactive
if 1 < 2 then "ok" else "no"
```

### Tuples, Lists, Dictionaries

```rex
(1, "hi", true)
[1, 2, 3]
{ a = 1, b = 2 }
```

Notes:

- Lists are implemented as a `List a` ADT (`Empty`/`Cons`) in the prelude.
- Cons expressions use `::` (for example `x::xs`), equivalent to `Cons x xs`.
- `Cons` is used with normal constructor-call syntax (`Cons head tail`), while `::` is infix sugar.
- Dictionary literals `{ k = v, ... }` build record/dict values. They become *records* when used as
  the payload of an ADT record constructor, or when their type is inferred/annotated as a record.

`::` is right-associative, so `1::2::[]` means `1::(2::[])`.

```rex,interactive
let
  xs = 1::2::3::[]
in
  xs
```

### Pattern Matching

`match` performs structural matching with one or more `when` arms:

```rex
match xs
  when Empty -> 0
  when Cons h t -> h
```

Patterns include:

- wildcards: `_`
- variables: `x`
- constructors: `Ok x`, `Cons h t`, `Pair a b`
- list patterns: `[]`, `[x]`, `[x, y]`
- cons patterns: `h::t` (equivalent to `Cons h t`)
- dict key presence: `{foo, bar}` (keys are identifiers)
- record patterns on record-carrying constructors: `Bar {x, y}`

```rex,interactive
match [1, 2, 3]
  when h::t -> h
  when [] -> 0
```

Rex checks ADT matches for exhaustiveness and reports missing constructors.

## Types

### Primitive Types

Common built-in types include:

- `bool`
- `i32` (default integer-literal fallback type)
- `f32` (float literal type)
- `string`
- `uuid`
- `datetime`

### Function Types

Functions are right-associative: `a -> b -> c` means `a -> (b -> c)`.

### Tuples, Lists, Arrays, Dicts

- Tuple type: `(a, b, c)`
- List type: `List a` (prelude)
- Array type: `Array a` (prelude)
- Dict type: `Dict a` (prelude; key type is a symbol/field label at runtime)

### ADTs

Define an ADT with `type`:

```rex,interactive
type Maybe a = Just a | Nothing
```

Constructors are values (functions) in the prelude environment:

```rex
Just 1
Nothing
```

#### Record-Carrying Constructors

ADT variants can carry a record payload:

```rex,interactive
type User = User { name: string, age: i32 }

let u: User = User { name = "Ada", age = 36 } in u
```

### Type Annotations

Annotate let bindings, lambda parameters, and function declarations:

```rex,interactive
let x: i32 = 1 in x
```

Annotations can mention ADTs and prelude types:

```rex,interactive
let xs: List i32 = [1, 2, 3] in xs
```

## Records: Projection and Update

Rex supports:

- projection: `x.field`
- record update: `{ base with { field = expr } }`

Projection and update are valid when the field is *definitely available* on the base:

- on plain record types `{ field: Ty, ... }`
- on single-variant ADTs whose payload is a record
- on multi-variant ADTs only after the constructor has been proven (typically by `match`)

Example (multi-variant refinement via `match`):

```rex,interactive
type Sum = A { x: i32 } | B { x: i32 }

let s: Sum = A { x = 1 } in
match s
  when A {x} -> { s with { x = x + 1 } }
  when B {x} -> { s with { x = x + 2 } }
```

## Declarations

### Functions (`fn`)

Top-level functions are declared with an explicit type signature and a value (typically a lambda):

```rex,interactive
fn add : i32 -> i32 -> i32 = \x y -> x + y
```

Top-level `fn` declarations are mutually recursive, so they can refer to each other in the same
module:

```rex,interactive
fn even : i32 -> bool = \n ->
  if n == 0 then true else odd (n - 1)

fn odd : i32 -> bool = \n ->
  if n == 0 then false else even (n - 1)

even 10
```

### Type Classes (`class`)

Type classes declare overloaded operations. Method signatures live in the class:

```rex,interactive
class Size a
  size : a -> i32
```

Methods can be operators (use parentheses to refer to them as values if needed):

```rex
class Eq a
  == : a -> a -> bool
```

Superclasses use `<=` (read “requires”):

```rex
class Ord a <= Eq a
  < : a -> a -> bool
```

### Instances (`instance`)

Instances attach method implementations to a concrete head type, optionally with constraints:

```rex,interactive
class Size a
  size : a -> i32

instance Size (List t)
  size = \xs ->
    match xs
      when Empty -> 0
      when Cons _ rest -> 1 + size rest
```

Instance contexts use `<=`:

```rex
class Pretty a
  pretty : a -> string

instance Pretty i32
  pretty = \_ -> "<i32>"

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

Notes:

- Instance heads are non-overlapping per class (overlap is rejected).
- Inside instance method bodies, the instance context is the only source of “given” constraints.

## Prelude Type Classes (Selected)

Rex ships a prelude that provides common abstractions and instances. Highlights:

- numeric hierarchy: `AdditiveMonoid`, `Semiring`, `Ring`, `Field`, …
- `Eq` / `Ord`
- `Functor` / `Applicative` / `Monad` for `List`, `Array`, `Option`, `Result`
- `Foldable`, `Filterable`, `Sequence`
- multi-parameter `Indexable t a` with instances for lists/arrays

Example: `Functor` across different container types:

```rex,interactive
( map ((*) 2) [1, 2, 3]
, map ((+) 1) (Some 41)
, map ((*) 2) (Ok 21)
)
```

Example: `Indexable`:

```rex,interactive
get 0 [10, 20, 30]
```

## Defaulting (Ambiguous Types)

Rex supports defaulting for certain numeric-like classes (e.g. `AdditiveMonoid`).
This matters for expressions like `zero` where no concrete type is otherwise forced.

Example:

```rex,interactive
zero
```

With no other constraints, `zero` defaults to a concrete numeric type (see [SPEC.md](SPEC.md) for the
exact algorithm and order).
