# Import and Module Semantics Are Duplicated

## Problem

Import resolution, module export handling, name qualification, and import projection rewriting exist in both `rex-engine` and `rex-lsp`. The engine owns the runtime/compiler path, while the LSP has its own implementation for diagnostics, completions, navigation, and quick fixes.

The engine source already contains comments acknowledging duplicated helpers.

## Evidence

- `rex-engine/src/modules/mod.rs` has comments:
  - "There are three copies of this function"
  - "There is another copy of this function in rex-lsp"
- `rex-engine/src/builder/rewrite.rs` implements import rewriting and validation for compilation.
- `rex-lsp/src/imports.rs` implements its own import loading, type/value/class maps, export collection, and projection rewriting.
- The LSP implementation parses imported modules, builds prefixes, injects type declarations, maps public functions/constructors, and rewrites imported references separately from the engine path.

## Why This Smells

Module semantics are core language semantics. Having two implementations means there are two chances for behavior to drift:

- The compiler may accept a program that the LSP flags incorrectly.
- The LSP may offer completions or code actions for names that the compiler would reject.
- Qualified value/type/class lookup rules can diverge.
- Public/private export behavior can diverge.
- Cyclic import and default import behavior can diverge.

This is not just duplicated utility code. It is duplicated language semantics.

The LSP has legitimate extra needs: spans, diagnostics, completion metadata, open-document snapshots, and graceful partial failure. But those are presentation and tooling concerns around the same semantic core. They should not require a parallel import system.

## Impact

This is the highest-risk smell found in the pass. It threatens semantic consistency between user-facing editor feedback and actual compilation.

The risk increases every time module/import behavior changes. Any change must be remembered and implemented twice, with different error models and data structures.

