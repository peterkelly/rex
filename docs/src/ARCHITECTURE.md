# Architecture

Rex is implemented as a small set of focused crates that form a pipeline:

1. **Parsing** (`rex-parser`): converts source text into a `rex_ast::CompilationUnit { decls, body }`.
2. **Typing** (`rex-typesystem`): Hindley–Milner inference + ADTs + type classes; produces a `rex_typesystem::TypedExpr`.
3. **Execution** (`rex-engine`): builds the host environment, prepares typed code into a
   `CompiledProgram`, and evaluates it to a runtime `rex_engine::Handle`.

The crates are designed so you can use them independently (e.g. parser-only tooling, typechecking-only checks, or embedding the full evaluator).

## Crates

- `rex-ast`: shared AST types (`Expr`, `Pattern`, `Decl`, `TypeExpr`, `CompilationUnit`, symbols, spans).
- `rex-parser`: source parser. Entry point: `rex_parser::parse`.
  - Parsing enforces a fixed cap on AST nesting.
- `rex-typesystem`: type system. Entry points:
  - `TypeSystem::new()` to create an explicit typing environment.
  - `infer_typed(&mut ts, expr)` / `infer(&mut ts, expr)` for type inference.
  - The inference implementation itself lives in `rex-typesystem/src/inference.rs`; `typesystem.rs` now holds the shared core types, environments, and registration logic.
  - For untrusted code, set `rex_typesystem::TypeSystemLimits::safe_defaults()` before inference.
- `rex-engine`: host environment builder, compiler, and runtime evaluator. Entry points:
  - `Builder::with_prelude(state)?` to inject runtime constructors and builtin implementations (`state` can be `()`).
  - `standard_type_system()?` to create a typing environment with the `rex-engine` standard prelude.
  - `Builder::build_compiler()` to consume the prepared builder into a compilation view.
  - `Compiler::compile_program` to consume the compiler and prepare a parsed program entry
    point into `(CompiledProgram, Evaluator)`. `Compiler::infer_*` consumes the compiler for
    type-only checks.
  - `Evaluator::run(compiled, inputs).await` to execute one prepared program. `inputs` is a
    `BTreeMap<String, Handle>` for the program's external `main` interface; `run` consumes the
    evaluator, compiled program, and input map.
  - `Builder` carries host state as `Builder<State>` (`State: Clone + Send + Sync + 'static`);
    typed `export` callbacks receive `&State` and return `Result<T, EngineError>`, typed
    `export_async` callbacks receive `&State` and return
    `Future<Output = Result<T, EngineError>>`, while handle-based native APIs
    (`export_native*`) receive `Context<State>`.
  - compile and evaluation APIs return `EngineError`; convenience entry
    points that cross phases return `ExecutionError`.
  - Host module injection API: `Module` + `Export` + `Builder::inject_module`.
- `rex-proc-macro`: `#[derive(Rex)]` bridge for Rust types ↔ Rex ADTs/values.
- `rex`: CLI front-end around the pipeline.
- `rex-lsp` / `rex-vscode`: editor tooling.

`rex-engine` is organized internally around the same phases:

- `builder/` owns builder-facing host/module registration and registry markdown.
- `compiler/` owns program preparation, import rewriting, typechecking, module loading state, and
  `CompiledProgram` construction.
- `evaluator/` owns execution, scheduling, native dispatch, `Context`, and the runtime core.
- `modules/`, `value.rs`, and `config.rs` hold shared module identities, heap values/GC roots,
  and runtime options.

## Design Notes

- **Typed preparation**: `rex-engine` prepares code into a typed form before execution. The
  current `CompiledProgram` stores a typed AST plus the environment snapshot needed to run it.
- **Single-shot execution**: evaluation is intentionally one-shot. `CompiledProgram` is moved into
  `Evaluator::run` with its runtime input map, consuming the evaluator as well. Prepare all
  required declarations/modules before constructing or consuming the evaluator.
- **Same-lineage runtime model**: a `CompiledProgram` is intended to run on the evaluator produced
  from the same compiler. Rex programs are supplied as source and compiled per run, so Rex does
  not expose a portable compiled-artifact or cross-runtime linking model.
- **Phase ownership**: `Builder` owns embedder configuration only until `build_compiler()`.
  `Compiler` then owns preparation state: the type environment, runtime declaration environment,
  runtime registries, heap, module loader caches, and execution policy snapshot. `Evaluator` is
  built only by consuming that compiler, so runtime code cannot mutate modules or compile new
  declarations.
- **Prelude ownership**: `rex-engine` owns the standard prelude source, standard typing
  environment, and runtime contract. The split is:
  - typeclass and instance declarations written in Rex at `rex-engine/src/prelude/typeclasses.rex`
  - `rex-engine/src/prelude/type_system.rs` builds the prelude-enabled `TypeSystem`, including ADTs, parsed declarations, and primop schemes
  - `rex-engine/src/prelude/mod.rs` parses the Rex source and injects runtime method bodies/native implementations for `Builder::with_prelude(state)?`
  - `rex-typesystem` exposes generic registration/inference APIs and does not own the standard prelude
- **Depth bounding**: Some parts of the pipeline are naturally recursive (parsing deeply nested parentheses, matching deeply nested terms). The parser enforces a fixed AST-depth cap, and the typechecker-limit API provides bounded recursion for production/untrusted workloads.
- **Import-use rewrite/validation**: module processing resolves import aliases across expression
  vars, constructor patterns, type references, and class references; unresolved qualified alias
  members are rejected as module errors before runtime.

## Intentional String Boundaries

Rex now prefers structured internal representations (for example `NameRef`, `BuiltinTypeId`,
`CanonicalSymbol`, and module/type/class maps) across parser, type system, evaluator, and LSP
rewrite paths. Remaining string usage is intentional in these boundary layers:

- **Source text and parsing**: the parser accepts source strings by definition.
- **Human-facing diagnostics and display**: error messages, hover text, CLI rendering, and debug
  output stringify symbols/types for readability.
- **Protocol/serialization boundaries**: JSON/LSP payloads are string-based and convert structured
  internal symbols/types at the edge.
- **Module specifiers**: parsed import names are textual before being resolved into structured
  `ModuleId` values.

Non-goal for this pass:

- Eliminating all `.to_string()` calls globally. The design target is to avoid stringly-typed core
  semantics, not to remove string conversion at UI/protocol boundaries.
