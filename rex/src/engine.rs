//! Compile and run Rex programs from a Rust host.
//!
//! This module is the main embedding API. A host creates an
//! [`Builder`](crate::engine::Builder), injects Rex modules or Rust-backed
//! [`Module`](crate::engine::Module) exports, compiles user source into a
//! [`CompiledProgram`](crate::engine::CompiledProgram), and runs it with an
//! [`Evaluator`](crate::engine::Evaluator).
//!
//! The public types here are re-exported from the engine crate so applications
//! can depend on `rex` as their primary embedding crate.

/// Hook used by [`AsyncCallPolicy`] to decide where admitted async host calls run.
pub use rex_engine::AsyncCallExecutor;

/// Runtime policy for polling or dispatching async host-call futures.
pub use rex_engine::AsyncCallPolicy;

/// Options for compiling an already parsed Rex program.
pub use rex_engine::CompileOptions;

/// A prepared Rex program that can be evaluated once.
pub use rex_engine::CompiledProgram;

/// One named input accepted by a compiled Rex program.
pub use rex_engine::MainInputSpec;

/// Externally visible main input and result types for a compiled program.
pub use rex_engine::MainSignature;

/// JSON-serializable description of a compiled program's external types.
pub use rex_engine::Manifest;

/// Compile-time view of a [`Builder`] used to prepare Rex source for execution.
pub use rex_engine::Compiler;

/// Host-call context passed to dynamic native functions.
pub use rex_engine::Context;

/// Builder for host modules, type information, and runtime policy.
pub use rex_engine::Builder;

/// Importer implementation that rejects every module import.
pub use rex_engine::DenyImporter;

/// Common error type used by the parser, type system bridge, module loader, and evaluator.
pub use rex_engine::EngineError;

/// Error raised while importing or preparing modules.
pub use rex_engine::ModuleError;

/// Options used when constructing a [`Builder`].
pub use rex_engine::EngineOptions;

/// Single-shot runtime used to run a compiled Rex program.
pub use rex_engine::Evaluator;

/// Admission limits for evaluator work and pending async host calls.
pub use rex_engine::ExecutionBounds;

/// Lease returned by a parallelism controller for one admitted async native call.
pub use rex_engine::NativeAsyncPermit;

/// Dynamic controller for evaluator ready-work and async native admission.
pub use rex_engine::ParallelismController;

/// Error returned by APIs that perform both compilation and evaluation.
pub use rex_engine::ExecutionError;

/// A staged Rust-backed function export before it is injected into a [`Builder`].
pub use rex_engine::Export;

/// Internal target used by exported Rust handlers to register native functions.
pub use rex_engine::ExportTarget;

/// Convert a Rex runtime value into a Rust value.
pub use rex_engine::FromRex;

/// Trait implemented by typed asynchronous Rust functions that can be exported to Rex.
pub use rex_engine::HostFnAsync;

/// Trait implemented by typed synchronous Rust functions that can be exported to Rex.
pub use rex_engine::HostFnSync;

/// Native registration payload used by exported Rust handlers.
pub use rex_engine::NativeRegistration;

/// Convert a Rust value into a Rex runtime value.
pub use rex_engine::IntoRex;

/// Request passed to a module importer.
pub use rex_engine::ImportRequest;

/// Async module import boundary implemented by embedders.
pub use rex_engine::Importer;

/// Staged host module containing Rex declarations and Rust-backed exports.
pub use rex_engine::Module;

pub use rex_engine::CompilationPackage;

pub use rex_engine::Declarations;

pub use rex_engine::StagedAdtDecl;

/// Stable identity assigned to an imported module.
pub use rex_engine::ModuleId;

/// Boxed future returned by value-based async native functions.
pub use rex_engine::NativeFuture;

/// Name of the automatically injected Rex prelude module.
pub use rex_engine::PRELUDE_MODULE_NAME;

/// Controls whether the Rex prelude is installed when constructing a [`Builder`].
pub use rex_engine::PreludeMode;

/// Return the parsed Rex program that implements prelude type class methods.
pub use rex_engine::prelude_typeclasses_program;

/// Internal module name used for declarations injected into the root environment.
pub use rex_engine::ROOT_MODULE_NAME;

/// Create a typing environment with the standard Rex prelude.
pub use rex_engine::standard_type_system;

/// Module payload returned by an importer.
pub use rex_engine::ResolvedModule;

/// Source or pre-parsed AST content returned by an importer.
pub use rex_engine::ResolvedModuleContent;

/// Take-once Rust module payload returned by an importer.
pub use rex_engine::ResolvedRustModule;

/// Trait for producing a Rex default value for a Rust-facing type.
pub use rex_engine::RexDefault;

/// Owned semantic data exchanged between Rex and host code.
pub use rex_engine::Value;

/// Formatting options for displaying Rex runtime values.
pub use rex_engine::ValueDisplayOptions;

/// Build a manifest from named inputs plus a result type.
pub use rex_engine::build_manifest;

/// Return true when a type still contains type variables.
pub use rex_engine::type_has_vars;

/// Convert ADT collection errors into an embedder-facing [`EngineError`].
pub use rex_engine::collect_adts_error_to_engine;

/// Build the internal symbol used for a virtual module export.
pub use rex_engine::virtual_export_name;
