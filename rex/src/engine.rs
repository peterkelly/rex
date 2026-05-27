//! Compile and run Rex programs from a Rust host.
//!
//! This module is the main embedding API. A host creates an
//! [`Engine`](crate::engine::Engine), injects Rex modules or Rust-backed
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

/// Runtime-side implementation metadata for a type class method.
pub use rex_engine::ClassMethodCapability;

/// A type class method required by a compiled program at runtime.
pub use rex_engine::ClassMethodRequirement;

/// Error from the compile phase of the Rex pipeline.
pub use rex_engine::CompileError;

/// Options for compiling an already parsed Rex program.
pub use rex_engine::CompileOptions;

/// Summary of external native and type class bindings referenced by compiled code.
pub use rex_engine::CompiledExterns;

/// A prepared Rex program that can be validated and evaluated once.
pub use rex_engine::CompiledProgram;

/// Metadata describing what a [`CompiledProgram`] captures and whether it is serializable.
pub use rex_engine::CompiledProgramBoundary;

/// One named input accepted by a compiled Rex program.
pub use rex_engine::MainInputSpec;

/// Externally visible main input and result types for a compiled program.
pub use rex_engine::MainSignature;

/// JSON-serializable description of a compiled program's external types.
pub use rex_engine::Manifest;

/// Compile-time view of an [`Engine`] used to prepare Rex source for execution.
pub use rex_engine::Compiler;

/// Host-call context passed to dynamic native functions.
pub use rex_engine::Context;

/// Configurable Rex engine for host modules, type information, and runtime policy.
pub use rex_engine::Engine;

/// Importer implementation that rejects every module import.
pub use rex_engine::DenyImporter;

/// Common error type used by the parser, type system bridge, module loader, and evaluator.
pub use rex_engine::EngineError;

/// Error raised while importing or preparing modules.
pub use rex_engine::ModuleError;

/// Options used when constructing an [`Engine`].
pub use rex_engine::EngineOptions;

/// Error from the evaluation phase after code has compiled.
pub use rex_engine::EvalError;

/// Single-shot runtime used to validate and run a compiled Rex program.
pub use rex_engine::Evaluator;

/// Admission limits for evaluator work and pending async host calls.
pub use rex_engine::ExecutionBounds;

/// Lease returned by a parallelism controller for one admitted async native call.
pub use rex_engine::NativeAsyncPermit;

/// Dynamic controller for evaluator ready-work and async native admission.
pub use rex_engine::ParallelismController;

/// Error returned by APIs that perform both compilation and evaluation.
pub use rex_engine::ExecutionError;

/// A staged Rust-backed function export before it is injected into an [`Engine`].
pub use rex_engine::Export;

/// Convert a Rex runtime value into a Rust value.
pub use rex_engine::FromRex;

/// A garbage-collected handle to a Rex runtime value.
pub use rex_engine::Handle;

/// Allocation arena and ownership root for Rex runtime values.
pub use rex_engine::Heap;

/// Trait implemented by typed asynchronous Rust functions that can be exported to Rex.
pub use rex_engine::HostFnAsync;

/// Trait implemented by typed synchronous Rust functions that can be exported to Rex.
pub use rex_engine::HostFnSync;

/// Convert a Rust value into a Rex runtime value.
pub use rex_engine::IntoRex;

/// Request passed to a module importer.
pub use rex_engine::ImportRequest;

/// Async module import boundary implemented by embedders.
pub use rex_engine::Importer;

/// Staged host module containing Rex declarations and Rust-backed exports.
pub use rex_engine::Module;

/// Stable identity assigned to an imported module.
pub use rex_engine::ModuleId;

/// Runtime-side metadata for one native function implementation.
pub use rex_engine::NativeCapability;

/// Boxed future returned by handle-based async native functions.
pub use rex_engine::NativeFuture;

/// Native function signature required by a compiled program at runtime.
pub use rex_engine::NativeRequirement;

/// Name of the automatically injected Rex prelude module.
pub use rex_engine::PRELUDE_MODULE_NAME;

/// Controls whether the Rex prelude is installed when constructing an [`Engine`].
pub use rex_engine::PreludeMode;

/// Internal module name used for declarations injected into the root environment.
pub use rex_engine::ROOT_MODULE_NAME;

/// Importer implementation for bundled stdlib modules.
pub use rex_engine::StdlibImporter;

/// Module payload returned by an importer.
pub use rex_engine::ResolvedModule;

/// Source or pre-parsed AST content returned by an importer.
pub use rex_engine::ResolvedModuleContent;

/// Trait for producing a Rex default value for a Rust-facing type.
pub use rex_engine::RexDefault;

/// Runtime capabilities available to satisfy a compiled program's link contract.
pub use rex_engine::RuntimeCapabilities;

/// Compatibility report between compiled requirements and runtime capabilities.
pub use rex_engine::RuntimeCompatibility;

/// Preflight view of runtime linkage available to an evaluator.
pub use rex_engine::RuntimeEnv;

/// Metadata describing what a [`RuntimeEnv`] captures and whether it is serializable.
pub use rex_engine::RuntimeEnvBoundary;

/// Runtime ABI and callable requirements captured by a [`CompiledProgram`].
pub use rex_engine::RuntimeLinkContract;

/// Safe public view of a Rex runtime value stored in the heap.
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
