# Pattern Matching Syntax in Rex

## Rationale

The decision to use match <expr> followed by arms introduced with when and
separated from their bodies by -> was made to maximize structural clarity and
reliability for LLM generation while keeping the syntax minimal. Because LLMs
generate code incrementally and can struggle with implicit or indentation-only
boundaries, each arm must begin with a clear, mandatory marker; requiring when
for every case establishes a strong, repetitive template—when <pattern> ->
<expression>—that models can continue predictably.

This explicit structure reduces the likelihood of accidentally merging arms,
extending a previous expression incorrectly, or omitting a necessary delimiter.
By enforcing a single canonical form with no optional variations, the syntax
lowers grammatical entropy and improves first-pass correctness, making match
expressions both simple to parse and straightforward for LLMs to generate
accurately.

## Status

Accepted design.

This document specifies the structure of `match` expressions and the future-reserved guard syntax.

---

# Summary of Decisions

Rex uses the following canonical match form:

```
match <expr> with
| <pattern> -> <expr>
| <pattern> -> <expr>
```

* `|` introduces each arm.
* `->` separates pattern from expression.
* `with` marks the beginning of arms.
* `where` is reserved for future guard support (not yet implemented).

---

# Explicit Requirements

## 1. Core Syntax

The only canonical form is:

```
match <expr> with
| <pattern> -> <expr>
| <pattern> -> <expr>
```

Constraints:

* The first arm must begin with `|`.
* All arms must begin with `|`.
* No optional omission of the first `|`.
* `match` must always include `with`.

---

## 2. Arm Structure

Each arm has the form:

```
| <pattern> -> <expression>
```

The right-hand side must be exactly one expression.

If multiple steps are needed, they must be composed using `let ... in`.

Example:

```
| pat ->
    let x = ...
    in ...
```

No statement blocks are allowed.

Rex remains expression-oriented.

---

## 3. Guard Syntax (Reserved, Not Implemented Yet)

To preserve future flexibility, Rex reserves the following form:

```
| <pattern> where <boolean-expression> -> <expression>
```

Guards are not currently implemented.

If encountered, the parser must emit a clear error:

“Guards are not yet supported.”

This prevents future breaking changes.

---

# Why This Design Was Chosen

Primary design goal:

Make match expressions easy for LLMs to generate correctly.

---

## 1. Dedicated arm delimiter (`|`)

`|` is a pure structural token.

It exists solely to mark the beginning of a new arm.

This is critical for LLM robustness because:

* It creates a strong repeating structure.
* It clearly separates arms.
* It allows reliable parser resynchronization.
* It reduces accidental continuation of previous arms.

This is more robust than relying on indentation alone.

---

## 2. Separation of structural and semantic tokens

In Rex:

* `|` = structural arm separator
* `where` = future guard introducer

Structural tokens and semantic modifiers must not overlap.

This keeps the grammar extensible and avoids ambiguity.

---

## 3. Avoiding layout sensitivity as the sole delimiter

Rex does not rely solely on indentation to delimit arms.

Indentation may be stylistic, but it is not required for parsing.

The presence of `|` ensures structure remains explicit.

This reduces:

* Indentation drift errors
* Partial-generation structural failures
* Accidental arm merging

---

## 4. Mandatory `->`

The arrow is required in every arm.

It clearly separates:

* Pattern space
* Expression space

This reduces blending errors during code generation.

---

# Grammar Sketch

match_expr :=
"match" expr "with"
arm+

arm :=
"|" pattern guard? "->" expr

guard :=
"where" expr

---

# Future-Proofing Rules

1. `where` is reserved even before guards are implemented.
2. Guards must appear between pattern and `->`.
3. Any use of `where` before guard support is implemented must produce a dedicated error message.
4. No alternative guard syntax may be introduced without updating this specification.

---

If this still renders oddly in your browser, tell me and I’ll instead provide a single downloadable combined document format optimized for copy extraction.
