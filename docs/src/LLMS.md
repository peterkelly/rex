# LLM Guidance for Generating Rex Code

This page is written for code-generating LLMs. It focuses on mistakes that are easy to make when
emitting Rex, and on fast validation steps that catch issues early.

## Recommended Context Order

When building or revising Rex code, read docs in this order:

1. This page (`LLMS.md`) for generation pitfalls and validation workflow.
2. `LANGUAGE.md` for syntax and everyday feature usage.
3. `SPEC.md` for locked behavior when edge cases matter.

## High-Value Rules

1. Use `fn` for top-level reusable functions; use `let`/`let rec` for local helpers.
2. For local mutual recursion, use comma-separated `let rec` bindings.
3. Use `x::xs` for list cons in both patterns and expressions (`x::xs` is equivalent to `Cons x xs`).
4. Validate snippets with the Rex CLI before shipping docs.

## Quick Generation Checklist

Before returning generated Rex code:

1. Put top-level reusable functions in `fn` declarations (they are mutually recursive).
2. Use `let rec` only for local recursive helpers inside expressions.
3. Add annotations where constructor or numeric ambiguity is likely (`Empty`, `zero`, overloaded methods).
4. Ensure the final expression returns a visible result (often a tuple for demos).
5. Run `cargo run -p rex -- run /tmp/snippet.rex` and fix all parse/type errors.

## Syntax Pitfalls

### 1) Recursion model

- Top-level `fn` declarations are mutually recursive.
- Single recursive local helper: `let rec`
- Mutually recursive local helpers: `let rec` + commas between bindings

Top-level mutual recursion:

```rex,interactive
fn even : i32 -> bool = \n ->
  if n == 0 then true else odd (n - 1)

fn odd : i32 -> bool = \n ->
  if n == 0 then false else even (n - 1)

even 10
```

```rex,interactive
let rec
  even = \n -> if n == 0 then true else odd (n - 1),
  odd = \n -> if n == 0 then false else even (n - 1)
in
  even 10
```

If you define local helpers in plain `let` and reference each other, you will get unbound-variable
errors. Use `let rec` for local recursion.

### 2) List construction and list patterns

- Pattern matching: `x::xs` is valid in `when` patterns.
- Expression construction: `x::xs` and `Cons x xs` are equivalent (list literals are also valid).
  `Cons` uses normal constructor/function call style (`Cons head tail`).

Equivalent:

```rex
x::xs
Cons x xs
```

### 3) ADT equality is not implicit

Do not assume custom ADTs automatically implement `Eq`. For example, comparing `Node` values with
`==` can fail with a missing-instance type error.

For small enums/ADTs, write an explicit equality helper:

```rex
node_eq = \a b ->
  match (a, b)
    when (A, A) -> true
    when (B, B) -> true
    when _ -> false
```

Related: avoid checking list emptiness with direct equality like `xs == []` in generic code. Prefer
an explicit matcher helper.

### 4) Ambiguous constructors (for example `Empty`)

Constructors like `Empty` can be ambiguous when multiple ADTs define the same constructor name
(for example `List.Empty` and `Tree.Empty`).

Disambiguate with an annotation at the binding site:

```rex,interactive
type Tree = Empty | Node { key: i32, left: Tree, right: Tree }

let
  t0: Tree = Empty
in
  match t0
    when Empty -> 0
    when Node {key, left, right} -> key
```

### 5) Reserved identifiers

Avoid bindings that collide with keywords (for example `as`). Use alternatives like `xs1`, `lefts`,
`rest1`, etc.

### 6) Constructor patterns with literals

Some forms like `Lit 1` inside nested patterns can fail to parse. Prefer simpler constructor
patterns and do literal checks in expression logic if needed.

Also avoid relying on tuple/list patterns that include numeric literals in one branch (for example
`(x::_, 0)`); match structurally first, then use an `if` guard/check in expression code.

## ADT and Pattern Tips

- Custom ADT declarations are straightforward:

```rex,interactive
type Tree = Empty | Node { key: i32, left: Tree, right: Tree }
```

- For record-carrying constructors, destructure directly in patterns:

```rex,interactive
type Tree = Empty | Node { key: i32, left: Tree, right: Tree }

fn root_key_or : i32 -> Tree -> i32 = \fallback t ->
  match t
    when Empty -> fallback
    when Node {key, left, right} -> key

root_key_or 0 (Node { key = 7, left = Empty, right = Empty })
```

- Keep matches exhaustive for ADTs (`Empty` + `Node`, etc.).

## Example Style for LLM Output

For robust generated snippets:

1. Put data type declarations first.
2. Use `fn` for top-level algorithm functions; use `let rec` for local recursive helpers.
3. Use a final `let ... in ...` result tuple for visible outputs.
4. Keep names explicit (`input`, `sorted`, `result`).

## Validation Workflow (Required)

Before emitting generated Rex snippets in docs:

1. Save snippet to a temporary `.rex` file.
2. Run:

```sh
cargo run -p rex -- run /tmp/snippet.rex
```

3. If parse/type errors appear, fix and re-run until clean.

For mdBook interactive demos, also run:

```sh
cd docs
mdbook build
```

## Interactive Docs Notes

Interactive Rex demos use fenced blocks with:

````md
```rex
...
```
````

If you omit `interactive`, the preprocessor will keep the code block static.
