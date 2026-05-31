# `rex-ast` Owns Compiler Lowering

## Problem

`rex-ast` is documented as the crate for shared AST data structures. Its crate docs explicitly describe it as "dumb data" and say complicated control flow belongs in later phases.

However, `CompilationUnit::body_with_fns()` lowers top-level `fn` declarations into nested `let` bindings around the final body expression. That is a compiler transformation, not just an AST data operation.

## Evidence

- `rex-ast/src/lib.rs` describes the crate as dumb AST data.
- `rex-ast/src/ast.rs` implements `CompilationUnit::body_with_fns()`.
- `body_with_fns()`:
  - walks top-level declarations,
  - turns function parameters into lambdas,
  - constructs a function type annotation,
  - wraps the body in nested `Expr::Let` nodes.
- LSP modules call `body_with_fns()` directly for queries, diagnostics, navigation, completion, and code actions.

## Why This Smells

The AST crate is the lowest shared layer. When it starts owning lowering behavior, every consumer inherits one particular compiler interpretation of declarations.

That has several consequences:

- The boundary between syntax representation and semantic preparation becomes fuzzy.
- Tooling code may accidentally rely on lowering details that should belong to the compiler or a shared analysis phase.
- Changes to top-level declaration semantics require edits in the foundational AST crate.
- The AST crate must know enough about function declaration semantics to synthesize expression structure and type syntax.

This also creates a subtle coordination problem: `rex-engine` separately lowers function declarations when injecting runtime definitions. If the expression-lowering behavior and runtime-declaration behavior drift, LSP/type-only behavior may no longer match execution behavior.

## Impact

This is a maintainability smell rather than an immediate failure. The larger Rex's declaration semantics get, the more expensive it becomes to keep lowering logic in a foundational data crate.

The problem will become sharper if top-level declarations gain more features, such as visibility-sensitive lowering, module-qualified lowering, richer constraints, or specialized treatment of `main`.

