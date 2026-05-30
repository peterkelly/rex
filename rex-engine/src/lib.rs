#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

//! Host environment, compiler, and runtime evaluator for Rex.
//!
//! Embedders build an [`Engine`] with host modules and runtime policy, convert
//! it into a [`Compiler`] to prepare Rex source or ASTs into a
//! [`CompiledProgram`], then run that program with a single-shot [`Evaluator`]
//! and a map of runtime inputs for `main`.

mod builder;
mod compiler;
mod config;
mod env;
mod error;
mod evaluator;
mod handlers;
mod manifest;
mod modules;
mod native_fn;
mod overloaded_fn;
mod prelude;
mod stack;
mod util;
mod value;

pub use builder::{
    engine::Engine,
    export::{Export, HostFnAsync, HostFnSync, NativeFuture},
    markdown::registry_markdown,
};
pub use compiler::{CompileOptions, Compiler, program::CompiledProgram};
pub use config::{
    AsyncCallExecutor, AsyncCallPolicy, EngineOptions, ExecutionBounds, NativeAsyncPermit,
    ParallelismController, PreludeMode,
};
pub use env::Environment;
pub use error::{EngineError, ExecutionError, ModuleError};
pub use evaluator::{Evaluator, context::Context};
pub use handlers::RexDefault;
pub use manifest::{MainInputSpec, MainSignature, Manifest, build_manifest, type_has_vars};
pub use modules::{
    CanonicalSymbol, DenyImporter, ImportRequest, Importer, Module, ModuleExports, ModuleId,
    ModuleInstance, ModuleKey, PRELUDE_MODULE_NAME, ROOT_MODULE_NAME, ResolvedModule,
    ResolvedModuleContent, StdlibImporter, SymbolKind, virtual_export_name,
};
pub use util::collect_adts_error_to_engine;
pub use value::{FromRex, Handle, Heap, IntoRex, Value, ValueDisplayOptions};
