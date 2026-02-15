# Architecture

Rex is implemented as a small set of focused crates that form a pipeline:

1. **Lexing** (`rex-lexer`): converts source text into a `Vec<Token>` with spans.
2. **Parsing** (`rex-parser`): converts tokens into a `rex_ast::expr::Program { decls, expr }`.
3. **Typing** (`rex-ts`): Hindley–Milner inference + ADTs + type classes; produces a `rex_ts::TypedExpr`.
4. **Evaluation** (`rex-engine`): evaluates `TypedExpr` to a runtime `rex_engine::Value`.

The crates are designed so you can use them independently (e.g. parser-only tooling, typechecking-only checks, or embedding the full evaluator).

## Crates

- `rex-ast`: shared AST types (`Expr`, `Pattern`, `Decl`, `TypeExpr`, `Program`, symbols).
- `rex-lexer`: tokenizer + spans (`Span`, `Position`).
- `rex-parser`: recursive-descent parser. Entry point: `rex_parser::Parser::parse_program`.
  - For untrusted code, set `ParserLimits::safe_defaults` before parsing.
- `rex-ts`: type system. Entry points:
  - `TypeSystem::with_prelude()?` to create a typing environment with standard types/classes.
  - `TypeSystem::infer_typed` / `TypeSystem::infer` for type inference.
  - For untrusted code, set `TypeSystemLimits::safe_defaults` before inference.
- `rex-engine`: runtime evaluator. Entry points:
  - `Engine::with_prelude(state)?` to inject runtime constructors and builtin implementations (`state` can be `()`).
  - `Engine::inject_decls(&program.decls)` to make user declarations available at runtime.
  - `Engine::eval_with_gas(&program.expr, &mut gas).await` to evaluate.
  - `Engine` carries host state as `Engine<State>` (`State: Clone + Sync + 'static`); typed `export` callbacks receive `&State` and return `Result<T, EngineError>`, typed `export_async` callbacks receive `&State` and return `Future<Output = Result<T, EngineError>>`, while pointer-level APIs (`export_native*`) receive `&Engine<State>`.
  - Host module injection API: `Module` + `Export` + `Engine::inject_module`.
- `rex-proc-macro`: `#[derive(Rex)]` bridge for Rust types ↔ Rex ADTs/values.
- `rex`: CLI front-end around the pipeline.
- `rex-lsp` / `rex-vscode`: editor tooling.

## Design Notes

- **Typed evaluation**: `rex-engine` always evaluates a `TypedExpr`; it typechecks first (via `rex-ts`) and then evaluates. This keeps runtime behavior predictable and makes native-function dispatch type-directed.
- **Prelude split**: The type system prelude is a combination of:
  - ADT/typeclass *heads* injected by `TypeSystem::with_prelude()?`
  - typeclass method *bodies* (written in Rex) loaded from `rex-ts/src/prelude_typeclasses.rex` and injected by `Engine::with_prelude(state)?` (`state` can be `()`)
- **Depth bounding**: Some parts of the pipeline are naturally recursive (parsing deeply nested parentheses, matching deeply nested terms). Parser/typechecker limit APIs provide bounded recursion for production/untrusted workloads.
