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
  - `Engine::compiler()` to create a preparation view over that environment.
  - `Engine::runtime_env()` / `Engine::evaluator()` to create runtime views over that environment.
  - `Compiler::compile_*` to prepare source into `CompiledProgram`.
  - `RuntimeEnv::validate(&compiled)` to preflight runtime linkage before execution.
  - `Evaluator::run(&compiled, &mut gas).await` to execute a prepared program.
  - convenience helpers like `Evaluator::eval_snippet` still exist, but they are just compile-then-run wrappers.
  - `Engine` carries host state as `Engine<State>` (`State: Clone + Sync + 'static`); typed `export` callbacks receive `&State` and return `Result<T, EngineError>`, typed `export_async` callbacks receive `&State` and return `Future<Output = Result<T, EngineError>>`, while pointer-level APIs (`export_native*`) receive `EvaluatorRef<'_, State>`.
  - public phase errors are split as `CompileError`, `EvalError`, and `ExecutionError` (for convenience entry points that do both phases).
  - Host library injection API: `Library` + `Export` + `Engine::inject_library`.
- `rexlang-proc-macro`: `#[derive(Rex)]` bridge for Rust types ↔ Rex ADTs/values.
- `rex`: CLI front-end around the pipeline.
- `rexlang-lsp` / `rexlang-vscode`: editor tooling.

## Design Notes

- **Typed preparation**: `rexlang-engine` prepares code into a typed form before execution. The
  current `CompiledProgram` still stores a typed AST plus runtime linkage metadata, but the
  compile/runtime boundary is now explicit in the API.
- **Current linkage model**: `CompiledProgram` captures the prepared expression and the environment
  snapshot needed to run it. Rex declarations that are part of the prepared program are captured
  there. Host-provided exports and typeclass method bindings remain runtime-linked through
  `RuntimeEnv`, which is why `RuntimeEnv::validate` exists. So the current model is "prepared plus
  link-validated", not "fully self-contained executable artifact".
- **RuntimeEnv split**: internally, `RuntimeEnv` now distinguishes the execution snapshot used by
  `Evaluator` and native dispatch from the engine-backed loader state still used by convenience
  entry points such as library loading and REPL session syncing. That keeps the public model
  stable while shrinking execution's implicit dependence on the full engine object.
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
