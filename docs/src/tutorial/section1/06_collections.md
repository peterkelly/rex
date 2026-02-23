# Tuples, Lists, and Dictionaries

Rex supports several lightweight data shapes.

## Tuples

Tuples group fixed-position values:

```rex,interactive
(1, "hi", true)
```

Rex supports tuple patterns in `match` and `let`. For indexing, use numeric projection
like `.0` and `.1`.

### Indexing tuples with `.`

```rex,interactive
let t = (1, "hi", true) in t.1
```

## Lists

List literals use square brackets:

```rex,interactive
[1, 2, 3]
```

Under the hood, lists are a prelude ADT `List a` with constructors `Empty` and `Cons`.

You can construct cons cells either as `Cons h t` (normal constructor call style) or with `h::t` sugar.

```rex,interactive
let xs = 1::2::3::[] in xs
```

```rex,interactive
match [1, 2, 3]
  when Empty -> 0
  when Cons h t -> h
```

### List patterns (sugar)

Rex also supports list-pattern sugar:

```rex,interactive
match [1, 2, 3]
  when [] -> 0
  when [x] -> x
  when x::xs -> x
```

## Lists vs Arrays

Rex supports both `List a` and `Array a`. They are related, but intentionally different.

- `List a` is a linked, recursive ADT (`Cons` / `Empty`) and is the default collection shape in
  user-written Rex code.
- `Array a` is a contiguous, indexed runtime container that maps naturally to host memory layouts
  (for example Rust `Vec<T>`).

Why both:

- Lists are ideal for language-level programming patterns (pattern matching, recursive
  decomposition, and list syntax sugar like `[]` and `x::xs`).
- Arrays are ideal for host interop and performance-sensitive data transfer.

In embedding scenarios, host functions commonly return arrays rather than lists. For example, a
Rust host function that returns `Vec<i32>` is exposed in Rex as returning `Array i32`.

### Matching host results: use `to_list`

Because list patterns (`[]`, `x::xs`, `Cons`) are list-only, convert array results before matching:

```rex,interactive
let
  {- We use to_array here to simulate a host function result of type Array i32. -}
  data = to_array [1, 2, 3]
in
  match (to_list data)
    when x::xs -> x
    when [] -> -1
```

The same shape without `to_list` fails with a type mismatch (array vs list):

```rex,interactive
let
  {- We use to_array here to simulate a host function result of type Array i32. -}
  data = to_array [1, 2, 3]
in
  match data
    when x::xs -> x
    when [] -> -1
```

Use the quick fix on the error and choose `Convert expression to list with to_list`; this rewrites
the mismatched expression for you, which is usually all that is needed.

### Why `to_list` is explicit (not implicit)

Rex keeps this conversion explicit for cost visibility. Converting `Array a` to `List a` allocates
new list nodes and copies references, so it is not a zero-cost operation.

If `to_list` were implicit, those allocations and copies could be inserted automatically in hot
paths (inside loops, repeated matches, pipelines, or nested helper calls) without being obvious in
source code. That would increase allocation rate, add GC/heap pressure, and make performance
regressions harder to find.

At host boundaries this matters even more: arrays are often used for efficient transfer and memory
locality. Keeping `to_list` explicit ensures the representation change happens only where you choose
to pay for it.

### LSP and LLM workflow

Because Rex exposes semantic diagnostics and code actions through LSP, an LLM-assisted workflow can
fix this in one pass:

1. Run typecheck / request diagnostics.
2. Detect the array/list mismatch at the match expression.
3. Apply the `to_list` quick fix code action.
4. Re-check and continue.

This is stronger than raw text editing because the fix is selected from compiler-semantic
information (actual inferred/expected types and targeted edits), not guessed from surface syntax.

## Dictionaries (records / dict values)

Dictionary literals use braces:

```rex,interactive
{ a = 1, b = 2 }
```

These are “record-like” values. Depending on context they may be treated as a record type
(`{ a: i32, b: i32 }`) or as a dictionary-like value; either way, you can project fields when the
field is known to exist:

```rex,interactive
type R = R { a: i32, b: i32 }

let r: R = R { a = 1, b = 2 } in r.a
```

### Forcing a dictionary type

If you want a polymorphic “dictionary” (instead of a specific record type), use type ascription
with `is`:

```rex,interactive
({ a = 1, b = 2 }) is Dict i32
```

### Matching dictionaries

Dictionary patterns check for key presence and bind those keys to variables:

```rex,interactive
let d = ({ a = 1, b = 2 }) is Dict i32 in
match d
  when {a, b} -> a + b
  when {a} -> a
  when {} -> 0
```

`{}` is useful as a fallback: it requires no keys, so it matches any dict.
