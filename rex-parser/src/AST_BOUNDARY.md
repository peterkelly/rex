# Rex PEG parser tree boundary

The PEG parser is grammar-driven. `grammar.rs` defines the formal Rex grammar
as Rust data, `formal.rs` interprets that grammar over lexer tokens, and
`ast_builder.rs` converts the resulting CST into `rex_ast::expr` values.

This boundary exists for three reasons:

- The grammar must be inspectable independently of AST construction.
- Recognition should live in the generic PEG interpreter, not in Rex-specific
  parser control flow.
- AST construction can preserve existing desugaring and diagnostics without
  making semantic code decide which syntax matches.

## Rule outputs

The generic PEG interpreter returns a `CstNode` for every named grammar rule and
token leaves for consumed lexer tokens. The CST is intentionally parser-local:
it is not exposed as the public parser result and is not intended to preserve
whitespace or comments.

The AST builder consumes only successful CST nodes and produces the existing
pipeline types: `Program`, `Decl`, `Expr`, `Pattern`, `TypeExpr`, declaration
structs, import structs, `NameRef`, and `TypeConstraint`.

## Semantic actions

Semantic actions belong in the CST-to-AST pass. They may inspect successful CST
nodes, validate semantic restrictions, and construct AST values, but they must
not drive token recognition or choose grammar alternatives.

Desugaring that is already part of current parser behavior remains parser-side:

- Binary operators lower to nested function application.
- `::` lowers to `Cons lhs rhs`.
- Unary `-` lowers to application of `negate`.
- Non-variable `let` bindings lower to single-arm `match` expressions.
- Signature-form top-level functions are flattened into `FnDecl` params and
  return type, preserving the current eta-expansion behavior.
- Uppercase zero-argument pattern identifiers become constructor patterns.

Semantic checks that require broader program knowledge remain outside the PEG
grammar. Examples include import alias conflicts, duplicate imported item names,
signature arity validation, and typeclass/typechecking rules.

## Spans

Every AST value constructed from the CST should use spans from the tokens the
grammar consumed. For desugared nodes, the span should preserve current parser
behavior unless we explicitly choose to change diagnostics.

## Memoization

The generic PEG interpreter memoizes CST rule results by `(rule, token
position)`. AST construction is a separate pass over the successful tree.
