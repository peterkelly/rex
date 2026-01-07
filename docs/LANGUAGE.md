# Rex Language Guide

Rex is a small, strongly-typed functional DSL with:

- Hindley–Milner type inference (let-polymorphism)
- algebraic data types (ADTs), including record-carrying constructors
- Haskell-style type classes (including higher-kinded classes like `Functor`)

This guide is meant for users and embedders. For locked/production-facing semantics and edge cases,
see `docs/SPEC.md`.

## A Program

A Rex program consists of:

- zero or more declarations (`type`, `class`, `instance`, `fn`)
- followed by a single expression (the program result)

Example:

```rex
fn inc : i32 -> i32 = \x -> x + 1

let
  xs = [1, 2, 3]
in
  map inc xs
```

## Lexical Structure

### Whitespace and Comments

- Whitespace (including newlines) is generally insignificant.
- Block comments use `{- ... -}` and can nest in the token stream but are stripped before parsing.

### Identifiers and Operators

- Identifiers start with a letter or `_`, followed by letters/digits/underscores.
- Operators are non-alphanumeric symbol sequences (`+`, `*`, `==`, `<`, …).
- Operators can be used as values by parenthesizing: `(+)`, `(==)`, `(<)`.

### Lambdas

The lambda syntax is `\x -> expr`. Some docs/examples may also use Unicode `λ` and `→`.

## Expressions

### Literals

- `true`, `false`
- integers and floats (currently integer literals are `i32`)
- strings: `"hello"`
- UUID and datetime literals (if present in your lexer source)

### Function Application

Application is left-associative: `f x y` parses as `(f x) y`.

```rex
let add = \x y -> x + y in add 1 2
```

### Let-In

Let binds one or more definitions and then evaluates a body:

```rex
let
  x = 1 + 2,
  y = 3
in
  x * y
```

Let bindings are polymorphic (HM “let-generalization”):

```rex
let id = \x -> x in (id 1, id true, id "hi")
```

### If-Then-Else

```rex
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
- Dictionary literals `{ k = v, ... }` build record/dict values. They become *records* when used as
  the payload of an ADT record constructor, or when their type is inferred/annotated as a record.

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
- cons patterns: `h:t`
- dict key presence: `{foo, bar}` (keys are identifiers)
- record patterns on record-carrying constructors: `Bar {x, y}`

Rex checks ADT matches for exhaustiveness and reports missing constructors.

## Types

### Primitive Types

Common built-in types include:

- `bool`
- `i32` (integer literal type)
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

```rex
type Maybe a = Just a | Nothing
```

Constructors are values (functions) in the prelude environment:

```rex
Just 1
Nothing
```

#### Record-Carrying Constructors

ADT variants can carry a record payload:

```rex
type User = User { name: string, age: i32 }

let u: User = User { name = "Ada", age = 36 } in u
```

### Type Annotations

Annotate let bindings, lambda parameters, and function declarations:

```rex
let x: i32 = 1 in x
```

Annotations can mention ADTs and prelude types:

```rex
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

```rex
type Sum = A { x: i32 } | B { x: i32 }

let s: Sum = A { x = 1 } in
match s
  when A {x} -> { s with { x = x + 1 } }
  when B {x} -> { s with { x = x + 2 } }
```

## Declarations

### Functions (`fn`)

Top-level functions are declared with an explicit type signature and a value (typically a lambda):

```rex
fn add : i32 -> i32 -> i32 = \x y -> x + y
```

### Type Classes (`class`)

Type classes declare overloaded operations. Method signatures live in the class:

```rex
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

```rex
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
- multi-parameter `Indexable t a` with instances for lists/arrays/tuples

Example: `Functor` across different container types:

```rex
( map ((*) 2) [1, 2, 3]
, map ((+) 1) (Some 41)
, map ((*) 2) (Ok 21)
)
```

Example: `Indexable`:

```rex
( get 0 [10, 20, 30]
, get 2 (1, 2, 3)
)
```

## Defaulting (Ambiguous Types)

Rex supports defaulting for certain numeric-like classes (e.g. `AdditiveMonoid`).
This matters for expressions like `zero` where no concrete type is otherwise forced.

Example:

```rex
zero
```

With no other constraints, `zero` defaults to a concrete numeric type (see `docs/SPEC.md` for the
exact algorithm and order).
