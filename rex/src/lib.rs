#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]
#![warn(missing_docs)]

//! Embedder-facing API for Rex.
//!
//! Rex is a strict, statically typed, pure functional language intended to be
//! embedded in Rust applications. Host programs provide modules and native
//! functions, then run user-supplied Rex snippets or module files to coordinate
//! work. The main public API is the [`engine`] module, especially
//! [`Engine`](engine::Engine) for configuring the runtime and
//! [`Module`](engine::Module) for exposing host capabilities to Rex code.
//!
//! Most embedders start with
//! [`Engine::with_prelude`](engine::Engine::with_prelude), register one or more
//! host modules, then compile and run a snippet or module workflow through
//! [`Compiler`](engine::Compiler) and [`Evaluator`](engine::Evaluator). The parser,
//! type system, runtime value, and JSON conversion APIs are also re-exported here so
//! an application can choose how much of the Rex pipeline it wants to control.
//!
//! # Minimal embedding example
//!
//! ```rust,no_run
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! use rex::engine::{Engine, EngineError, Module};
//!
//! let mut engine = Engine::with_prelude(())?;
//!
//! let mut math = Module::new("host.math");
//! math.export("inc", |_state: &(), x: i32| {
//!     Ok::<i32, EngineError>(x + 1)
//! })?;
//! engine.inject_module(math)?;
//!
//! let (value, typ) = engine
//!     .into_evaluator()
//!     .eval_snippet("import host.math (inc);\ninc 41")
//!     .await?;
//!
//! assert_eq!(typ.to_string(), "i32");
//! assert_eq!(value.as_i32()?, 42);
//! # Ok(())
//! # }
//! ```
//!
//! For lightweight tests and command-line style integrations, [`eval`] parses,
//! typechecks, evaluates, and converts the result to JSON in one call. Production
//! embedders usually use [`Engine`](engine::Engine) directly so they can
//! register host modules, set execution bounds, inspect type information, and
//! handle compile and evaluation errors separately.

/// Rex abstract syntax tree types produced by the parser.
pub mod ast;

/// Compile-time and runtime APIs for embedding Rex in a Rust host program.
pub mod engine;

/// Conversion between JSON values and typed Rex runtime values.
pub mod json;

/// Source parser entry points and parse diagnostics.
pub mod parser;

/// Hindley-Milner type inference, type representations, ADTs, and type classes.
pub mod typesystem;

/// Derive bridge for Rust data types that should cross the Rex boundary.
///
/// `#[derive(Rex)]` implements these traits for the derived Rust type:
///
/// - [`RexType`](typesystem::RexType)
/// - [`RexAdt`](typesystem::RexAdt)
/// - [`IntoRex`](engine::IntoRex)
/// - [`FromRex`](engine::FromRex)
///
/// The derive also adds inherent helper methods such as `inject_rex`,
/// `rex_adt_decl`, and `rex_adt_family`. It does not implement
/// [`RexDefault`](engine::RexDefault); use `inject_rex_with_default` only for
/// types that already provide that trait.
pub use rex_proc_macro::Rex;

/// Parse, typecheck, evaluate, and JSON-encode a Rex snippet.
///
/// This is a convenience helper for small integrations, examples, and tests. It
/// creates an [`Engine`](engine::Engine) with the prelude enabled, installs the
/// bundled stdlib importer, compiles `source` as a snippet, evaluates it once, and
/// converts the result to JSON [`Value`](serde_json::Value) using the inferred
/// result type.
///
/// Hosts that need to inject functions, control module loading, preserve compile
/// diagnostics, or set runtime policy should use [`Engine`](engine::Engine)
/// directly instead.
pub async fn eval(source: &str) -> Result<serde_json::Value, engine::ExecutionError> {
    parser::parse(source).map_err(|errs| {
        engine::CompileError::from(engine::EngineError::from(format!("parse error: {errs:?}")))
    })?;

    let engine = engine::Engine::with_prelude(()).map_err(|e| {
        engine::CompileError::from(engine::EngineError::from(format!(
            "failed to initialize engine: {e}"
        )))
    })?;
    let mut compiler = engine.into_compiler();
    let program = compiler.compile_snippet(source).await?;
    let result_type = program.result_type().clone();
    let evaluator = compiler.into_evaluator();
    let type_system = evaluator.type_system();

    let value = evaluator.run(program).await?;

    let json =
        json::rex_to_json(&value, &result_type, &type_system).map_err(engine::EvalError::from)?;
    Ok(json)
}
