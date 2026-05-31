# Pipeline Boundaries Leak Through Prelude Parsing

## Problem

The documented architecture presents Rex as a clean pipeline:

1. `rex-parser` parses source into `rex_ast::CompilationUnit`.
2. `rex-typesystem` performs Hindley-Milner inference over parsed AST.
3. `rex-engine` prepares and evaluates typed programs.

In the implementation, `rex-typesystem` depends directly on `rex-parser` because the type-system prelude parses `prelude_typeclasses.rex` at runtime. That means the type system is not actually independent of the parser crate, despite the architecture describing it as a reusable typing layer after parsing.

## Evidence

- `rex-typesystem/Cargo.toml` has a normal dependency on `rex-parser`, not just a dev dependency.
- `rex-typesystem/src/prelude.rs` imports `rex_parser::parse`.
- `prelude_typeclasses_program()` uses `include_str!("prelude_typeclasses.rex")` and parses that source through the parser on first use.

## Why This Smells

This creates an upward dependency from the semantic/type layer back into source parsing. The parser is no longer just a producer of ASTs for downstream phases; it is also part of type-system initialization.

The result is a blurred crate boundary:

- Parser-only changes can affect type-system initialization.
- Type-system consumers cannot depend on `rex-typesystem` without also bringing in parser machinery.
- The architecture documentation overstates the independence of `rex-typesystem`.
- Prelude initialization becomes a runtime parse step rather than a checked build-time or data-level dependency.

This is especially awkward because `rex-typesystem` already works primarily over `rex_ast` types. The parser dependency exists for one prelude-loading path, not because inference fundamentally requires parsing.

## Impact

This is not currently a correctness bug, but it makes the layering harder to reason about. It also raises the cost of future attempts to expose the type system as a standalone library, reduce compile-time dependencies, or support alternative AST producers.

The risk grows as more semantic assets are represented as Rex source and loaded from inside downstream crates.

