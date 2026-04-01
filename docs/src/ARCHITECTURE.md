# Architecture

Rex is implemented as a small set of focused crates that form a pipeline:

1. **Lexing** (`rexlang-lexer`): converts source text into a `Vec<Token>` with spans.
2. **Parsing** (`rexlang-parser`): converts tokens into a `rex_ast::expr::Program { decls, expr }`.
3. **Typing** (`rexlang-typesystem`): Hindley–Milner inference + ADTs + type classes; produces a `rexlang_typesystem::TypedExpr`.
4. **Evaluation** (`rexlang-engine`): evaluates `TypedExpr` to a runtime `rexlang_engine::Value`.

The crates are designed so you can use them independently (e.g. parser-only tooling, typechecking-only checks, or embedding the full evaluator).

## Crates

- `rex-ast`: shared AST types (`Expr`, `Pattern`, `Decl`, `TypeExpr`, `Program`, symbols).
- `rexlang-lexer`: tokenizer + spans (`Span`, `Position`).
- `rexlang-parser`: recursive-descent parser. Entry point: `rexlang_parser::Parser::parse_program`.
  - For untrusted code, set `ParserLimits::safe_defaults` before parsing.
- `rexlang-typesystem`: type system. Entry points:
  - `TypeSystem::with_prelude()?` to create a typing environment with standard types/classes.
  - `TypeSystem::infer_typed` / `TypeSystem::infer` for type inference.
  - For untrusted code, set `TypeSystemLimits::safe_defaults` before inference.
- `rexlang-engine`: runtime evaluator. Entry points:
  - `Engine::with_prelude(state)?` to inject runtime constructors and builtin implementations (`state` can be `()`).
  - `Engine::inject_decls(&program.decls)` to make user declarations available at runtime.
  - `Engine::eval_with_gas(&program.expr, &mut gas).await` to evaluate.
  - `Engine` carries host state as `Engine<State>` (`State: Clone + Sync + 'static`); typed `export` callbacks receive `&State` and return `Result<T, EngineError>`, typed `export_async` callbacks receive `&State` and return `Future<Output = Result<T, EngineError>>`, while pointer-level APIs (`export_native*`) receive `EvaluatorRef<'_, State>`.
  - Host library injection API: `Library` + `Export` + `Engine::inject_library`.
- `rexlang-proc-macro`: `#[derive(Rex)]` bridge for Rust types ↔ Rex ADTs/values.
- `rex`: CLI front-end around the pipeline.
- `rexlang-lsp` / `rexlang-vscode`: editor tooling.

## Design Notes

- **Typed evaluation**: `rexlang-engine` always evaluates a `TypedExpr`; it typechecks first (via `rexlang-typesystem`) and then evaluates. This keeps runtime behavior predictable and makes native-function dispatch type-directed.
- **Prelude split**: The type system prelude is a combination of:
  - ADT/typeclass *heads* injected by `TypeSystem::with_prelude()?`
  - typeclass method *bodies* (written in Rex) loaded from `rexlang-typesystem/src/prelude_typeclasses.rex` and injected by `Engine::with_prelude(state)?` (`state` can be `()`)
- **Depth bounding**: Some parts of the pipeline are naturally recursive (parsing deeply nested parentheses, matching deeply nested terms). Parser/typechecker limit APIs provide bounded recursion for production/untrusted workloads.
- **Import-use rewrite/validation**: library processing resolves import aliases across expression
  vars, constructor patterns, type references, and class references; unresolved qualified alias
  members are rejected as library errors before runtime.

## Intentional String Boundaries

Rex now prefers structured internal representations (for example `NameRef`, `BuiltinTypeId`,
`CanonicalSymbol`, and library/type/class maps) across parser, type system, evaluator, and LSP
rewrite paths. Remaining string usage is intentional in these boundary layers:

- **Source text and parsing**: lexer/parser operate on source strings by definition.
- **Human-facing diagnostics and display**: error messages, hover text, CLI rendering, and debug
  output stringify symbols/types for readability.
- **Protocol/serialization boundaries**: JSON/LSP payloads are string-based and convert structured
  internal symbols/types at the edge.
- **Filesystem/library specifiers**: import specifiers and path labels are textual before being
  resolved into structured library identities.

Non-goal for this pass:

- Eliminating all `.to_string()` calls globally. The design target is to avoid stringly-typed core
  semantics, not to remove string conversion at UI/protocol boundaries.
