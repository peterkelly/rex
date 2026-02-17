# Colon and Double-Colon Syntax in Rex

## Status

Accepted design.

This document specifies the role of `:` and `::` in Rex and explains the reasoning behind the decision, with particular emphasis on LLM-friendly syntax design.

---

# Summary of Decisions

Rex uses:

* `:` for type annotations
* `::` for list construction and list pattern matching

These roles are strictly separated and must never overlap.

---

# Explicit Requirements

## 1. `:` is reserved for type annotations

`:` must only appear in syntactic positions where a type annotation is expected.

Required forms:

Function type annotation:

```
fn f: A -> B = ...
```

Parameter annotation (if supported):

```
fn f (x: A) = ...
```

Expression ascription (if supported):

```
(expr: Type)
```

Hard rule:

`:` must never be used as:

* a list constructor
* a pattern operator
* a value-level operator

It belongs exclusively to the type system.

---

## 2. `::` is the list cons operator

`::` is used for:

* List construction in expressions
* List decomposition in patterns

Expression example:

```
x :: xs
```

Pattern example:

```
match xs with
| x :: xs -> ...
```

Canonical desugaring rule:

List literal sugar:

```
[x, y, z]
```

must desugar to:

```
x :: y :: z :: []
```

Single-element list:

```
[x]
```

must desugar to:

```
x :: []
```

---

# Why This Design Was Chosen

Primary design goal:

Make the language easy for LLMs to generate and understand.

---

## 1. One token = one domain

`:` belongs to the type system.

`::` belongs to list structure.

There is no syntactic overlap.

LLMs perform better when tokens have single, stable meanings. Overloading `:` for both type annotations and list construction would increase ambiguity and error rates.

---

## 2. Bidirectional symmetry

The same operator builds and matches lists:

Construction:

```
x :: xs
```

Pattern matching:

```
x :: xs
```

This symmetry makes it easier for both humans and models to infer valid patterns.

The model can generalize:

“If I build lists with `::`, I also match them with `::`.”

---

## 3. Reduced ambiguity during generation

If `:` were used for both types and list structure, partial generations like:

```
x : y
```

would require contextual disambiguation.

By separating `:` and `::`, the parser and model can immediately distinguish:

* type position
* value-level list structure

This significantly reduces token-level confusion.

---

## 4. Alignment with ML-family conventions

This choice aligns with OCaml and related ML-family languages:

* `:` for type annotations
* `::` for cons

This familiarity lowers the cognitive barrier for experienced developers while remaining predictable for LLMs trained on ML-like syntax.

---

# Reserved Tokens

`:` is reserved exclusively for type annotations.
`::` is reserved exclusively for list construction and list pattern matching.

They may not be redefined as operators.

---

# Non-Goals

* Rex does not support `:` as a value-level operator.
* Rex does not support alternative cons spellings.
* Rex does not allow both `:` and `::` to act as cons.

Consistency is prioritized over flexibility.

---

# Implementation Notes

1. The lexer must treat `::` as a distinct token, not as two `:` tokens.
2. `:` must only be valid in type annotation positions.
3. List literals must desugar to `::` chains ending in `[]`.
4. Pattern matching must use the same `::` operator as expression construction.
5. The grammar must enforce the separation strictly.
