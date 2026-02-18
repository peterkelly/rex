# Default: Typeclass and Defaulting

In Rex, "default" can mean two different things:

- The `Default` typeclass method `default : a`
- The defaulting pass that resolves ambiguous type variables for defaultable classes

## 1) `Default` Typeclass (`default : a`)

The prelude provides `Default` and a set of built-in instances.

Built-in types with `Default`:

- `bool`
- `u8`, `u16`, `u32`, `u64`
- `i8`, `i16`, `i32`, `i64`
- `f32`, `f64`
- `string`
- `List a`
- `Array a`
- `Option a`
- `Result a e` (when `Default a` is available)

### Implementing `Default` for custom ADTs

You can implement `Default` for many ADT shapes.

Single constructor with unnamed fields:

```rex
type Pair = Pair i32 bool

instance Default Pair
    default = Pair 42 true
```

Single constructor with named fields:

```rex
type Config = Config { retries: i32, enabled: bool }

instance Default Config
    default = Config { retries = 3, enabled = false }
```

Multiple variants (enum) with no fields:

```rex
type Mode = Fast | Safe | Debug

instance Default Mode
    default = Safe
```

Multiple variants with mixed payload shapes:

```rex
type Token = Eof | IntLit i32 | Meta { line: i32, col: i32 }

instance Default Token
    default = Meta { line = 1, col = 1 }
```

Generic ADTs with constraints:

```rex
type Box a = Box a | Missing

instance Default (Box a) <= Default a
    default = Box default
```

### Ambiguous `default` calls and `is`

When multiple `Default` instances are in scope, `default` may be ambiguous until you pin the type.
Record updates require a definitely known base type.

Failing example:

```rex,interactive
type A = A { x: i32, y: i32 }
type B = B { x: i32, y: i32 }

instance Default A
    default = A { x = 1, y = 2 }

instance Default B
    default = B { x = 10, y = 20 }

{ default with { x = 9 } }
```

In the editor/playground, use the quick fix on this error to insert `is` for the intended ADT.
Try it on the example above.

Passing example (same setup, with explicit `is`):

```rex,interactive
type A = A { x: i32, y: i32 }
type B = B { x: i32, y: i32 }

instance Default A
    default = A { x = 1, y = 2 }

instance Default B
    default = B { x = 10, y = 20 }

{ (default is A) with { x = 9 } }
```

Another failing example (same ambiguity in a `let` binding):

```rex,interactive
type A = A { x: i32, y: i32 }
type B = B { x: i32, y: i32 }

instance Default A
    default = A { x = 1, y = 2 }

instance Default B
    default = B { x = 10, y = 20 }

let
    a = { default with { x = 9 } },
    b = { default with { y = 8 } }
in
    (a, b)
```

For this `let`-binding form, quick fixes offer two styles: add `is` to the
`default` call, or add a type annotation on the binding (for example
`a: A = ...`). These same quick fixes are also exposed through LSP code
actions and can be used by LLM-driven tooling.

## 2) Type Defaulting (Ambiguous Types)

Some overloaded prelude operations (such as `zero`) only require class constraints:

```rex,interactive
zero
```

If nothing else forces a concrete type, Rex runs defaulting to pick a concrete type from
the defaulting candidates that satisfy the required class constraints.

If you see an "ambiguous overload" error around numeric expressions, force a type:

```rex,interactive
let z: i32 = zero in z
```

Or use the value in a way that constrains it:

```rex,interactive
zero + 1
```

Integer literals are also overloaded (over `Integral`) and become concrete from context:

```rex,interactive
let
  x = 4,
  f: u16 -> u16 = \n -> n
in
  f x
```

Negative literals must resolve to a signed type:

```rex,interactive
let
  x: i32 = -3,
  f: i32 -> i32 = \n -> n
in
  f x
```

`let x: u32 = -3 in x` is a type error.

The defaulting algorithm is specified in [Specification](../../SPEC.md) ("Defaulting").
