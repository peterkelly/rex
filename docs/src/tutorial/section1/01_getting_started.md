# Getting Started

Small Rex programs can be written as *one expression*, optionally preceded by
top-level declarations:

- `type` — algebraic data types (ADTs)
- `class` / `instance` — type classes and instances
- `fn` — top-level functions

> **Note:** This tutorial focuses on writing Rex code. If you want to embed Rex in Rust, see [Embedding](../../EMBEDDING.md).

## Running Rex

A runnable Rex file either defines `main` or ends with a final expression. The
examples in this tutorial usually use final expressions because they keep small
programs compact.

From this repository, you can run a Rex file:

```sh
cargo run -p rex-cli --bin rex -- rex-cli/examples/record_update.rex
```

Or evaluate a small snippet inline:

```sh
cargo run -p rex-cli --bin rex -- -c 'map ((*) 2) [1, 2, 3]'
```

### What you should see

The CLI prints the evaluated value of the program entry point in JSON format. If
something fails, you’ll get a parse/type/eval error (often with a span).

## What “one expression” means

Even with declarations, a program without `main` uses the final expression as
its result:

```rex,interactive
fn inc x: i32 -> i32 = x + 1;

let xs = [1, 2, 3] in
  map inc xs
```

The program above:

1. Declares a top-level function `inc`.
2. Creates a list `xs`.
3. Evaluates `map inc xs` as the program’s result.

## Comments

Comments use `// ...` for line comments and `/* ... */` for block comments:

```rex,interactive
/* This is a comment */
1 + 2
```

## Whitespace

Most whitespace is insignificant, and indentation has no syntactic meaning. Multi-line expressions
are often easier to read:

```rex,interactive
let
  x = 1,
  y = 2
in
  x + y
```

Commas between `let` bindings are required. Top-level function declarations end with semicolons,
so multi-line bodies do not depend on indentation. The parser also accepts many one-line forms, but
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
cargo run -p rex-cli --bin rex -- hello.rex
```

## Lambda and Arrow Spelling

Rex uses ASCII-only syntax for lambdas and arrows:

- `\` and `->`

The Unicode lambda and right-arrow glyphs are not accepted.
