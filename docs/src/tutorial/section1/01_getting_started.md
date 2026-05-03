# Getting Started

Rex programs are *one expression*, optionally preceded by top-level declarations:

- `type` — algebraic data types (ADTs)
- `class` / `instance` — type classes and instances
- `fn` — top-level functions

> **Note:** This tutorial focuses on writing Rex code. If you want to embed Rex in Rust, see [Embedding](../../EMBEDDING.md).

## Running Rex

From this repository, you can run a file:

```sh
cargo run -p rex-cli -- run rex-cli/examples/record_update.rex
```

Or evaluate a small snippet inline:

```sh
cargo run -p rex-cli -- run -c 'map ((*) 2) [1, 2, 3]'
```

### What you should see

The CLI prints the evaluated value of the final expression in your program. If something fails,
you’ll get a parse/type/eval error (often with a span).

## What “one expression” means

Even with declarations, the program result is the final expression:

```rex,interactive
fn inc : i32 -> i32 = \x -> x + 1

let xs = [1, 2, 3] in
  map inc xs
```

The program above:

1. Declares a top-level function `inc`.
2. Creates a list `xs`.
3. Evaluates `map inc xs` as the program’s result.

## Comments

Comments use `{- ... -}`:

```rex,interactive
{- This is a comment -}
1 + 2
```

## Whitespace

Most whitespace is insignificant, but multi-line expressions are often easier to read:

```rex,interactive
let
  x = 1,
  y = 2
in
  x + y
```

Commas between `let` bindings are required. The parser also accepts many one-line forms, but
multi-line formatting tends to be easier to debug.

Type-class and instance method blocks use explicit braces and semicolon-separated methods:

```rex,interactive
class Size a where {
  size : a -> i32;
}
```

An empty marker class or instance uses a semicolon:

```rex,interactive
class Marker a;
instance Marker i32;
```

## Your first “real” Rex file

Create a file `hello.rex`:

```rex,interactive
let
  greet = \name -> "hello, " + name
in
  greet "world"
```

Run it:

```sh
cargo run -p rex-cli -- run hello.rex
```

## Unicode (λ, →) versus ASCII

Rex accepts both:

- `\` and `->`
- `λ` and `→`

Use whichever your editor makes pleasant. The repo’s examples use a mix.
