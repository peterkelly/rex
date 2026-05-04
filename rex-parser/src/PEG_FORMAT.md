# Canonical PEG format

This file defines the canonical textual format for rendering `Grammar<R>`,
`Item<R>`, and `Peg<R>` values. `grammar_to_string` must emit exactly this
format, and the `.peg` parser must parse this format back into structurally
comparable grammar data.

The accepted syntax is specified in `peg.peg`; `peg_syntax_grammar()` is the
Rust-side structural mirror of that file.

The format is intentionally a serialization of the Rust grammar data, not a
general documentation notation. Comments in generated grammar files are
`Grammar` items and round-trip as part of the structural comparison.

## File shape

A file is a sequence of rule definitions and comment blocks. The first rule
definition is the grammar start rule. For the Rex grammar this is `Program`.

Rules are emitted in stable grammar order. The canonical Rex grammar order is
the `Grammar::rules()` order, which follows the `RexRule` ordering.

Each rule is rendered on one logical line unless wrapping is needed. Rule names
are padded so every `<-` starts in the same column, one space after the longest
rule name in the grammar:

```peg
Short          <- expression
LongerRuleName <- expression
```

The canonical renderer may insert line breaks inside long sequences or choices,
but it must do so deterministically.

Comment items render as comment blocks separated from rule definitions by blank
lines. Every line of comment text is prefixed with `# `. The parser strips that
single separator space when present and preserves the remaining comment text
exactly, including internal newlines and leading spaces after the separator.

## Names

Rule references and token references are both bare identifiers. Rule spellings
come from `Display` for the Rust rule enum values. Token spellings come from
an explicit token-name mapping used by `Display` and the `.peg` grammar loader:
Rust variants stay in CamelCase, while canonical `.peg` token names are upper
snake case.

```peg
Program    <- Decl* Expr? EOF
ImportDecl <- IMPORT ImportPath SEMI_COLON
```

The grammar loader resolves left-hand side identifiers as rule names. A
right-hand side identifier must resolve to exactly one rule or token name. If a
name exists in both namespaces, loading the grammar is an error.

Token references use canonical token names, not Rex source spellings. For
example, use `IMPORT`, `ARROW_R`, and `PAREN_L`, not `'import'`, `'->'`, or
`'('`.

## Operators

Canonical precedence, from tightest to loosest:

1. Grouping: `(expression)`
2. Postfix repetition: `?`, `*`, `+`
3. Prefix lookahead: `&`, `!`
4. Sequence: whitespace-separated expressions
5. Ordered choice: `A / B / C`

The renderer must add parentheses whenever omitting them would change the parsed
`Peg` structure.

## PEG constructors

`Peg::Token(kind)` renders as the token name:

```peg
IDENT
ARROW_R
```

`Peg::Rule(rule)` renders as the rule name:

```peg
Expr
TypeExpr
```

`Peg::Seq(items)` renders as a whitespace-separated sequence:

```peg
IMPORT ImportPath ImportClause? ImportAlias? SEMI_COLON
```

Canonical grammar data should not contain nested `Seq` values or one-item
sequences. The grammar loader normalizes parsed sequences before comparison.

`Peg::Choice(items)` renders with `/`:

```peg
ImportDecl / TypeDecl / FnDecl
```

Canonical grammar data should not contain nested `Choice` values or one-item
choices. The grammar loader normalizes parsed choices before comparison.

`Peg::Optional(item)` renders as a postfix `?`:

```peg
WhereConstraints?
```

`Peg::Repeat(item)` renders as a postfix `*`:

```peg
Decl*
```

`Peg::Repeat1(item)` renders as a postfix `+`:

```peg
MatchArm+
```

`Peg::And(item)` renders as prefix `&`:

```peg
&IDENT
```

`Peg::Not(item)` renders as prefix `!`:

```peg
!ASSIGN
```

`Peg::Cut(item)` renders as a function form:

```peg
cut(Expr)
```

`Peg::Label(message, item)` renders as a function form with a Rust-style string
literal:

```peg
label("expected `;` after function body", SEMI_COLON)
```

String literals use Rust escaping rules for `\`, `"`, newlines, tabs, and other
non-printable characters.

The lowercase names `cut` and `label` are reserved for these function forms.

## Normalization before comparison

The grammar loader normalizes syntax sugar into the canonical Rust shape before
comparing against a Rust-defined grammar:

- nested sequences become one flat `Peg::Seq`
- nested choices become one flat `Peg::Choice`
- postfix operators become `Optional`, `Repeat`, and `Repeat1`
- prefix operators become `And` and `Not`
- `cut(...)` and `label(...)` become `Cut` and `Label`

Normalization is not language-equivalence checking. It is only the mechanical
conversion from this surface syntax into the canonical Rust grammar data shape.
