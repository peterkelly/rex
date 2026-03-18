# Rex AST (`rexlang-ast`)

This crate defines the core syntax tree for Rex and shared language data types.

## What’s here

- `rexlang_ast::expr`: expression/decl/type AST nodes (`Program`, `Expr`, `Decl`, `TypeExpr`, etc.)
- `Symbol` + interning utilities (`intern`): stable identifiers used across crates
- Span-bearing nodes (via `rexlang-lexer` spans) so diagnostics can point at source locations

This crate is intentionally “dumb”: it’s primarily data structures and small helpers, used by the
lexer, parser, type system, engine, and tooling.

