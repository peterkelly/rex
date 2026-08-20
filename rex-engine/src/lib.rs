#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

//! Host environment, compiler, and runtime evaluator for Rex.
//!
//! Embedders build a [`Builder`] with host modules and runtime policy, convert
//! it into a [`Compiler`] to prepare Rex source or ASTs into a
//! [`CompiledProgram`], then run that program with a single-shot [`Evaluator`]
//! and a map of runtime inputs for `main`. Runtime values cross the embedding
//! boundary as owned, heap-independent [`Value`] trees.

mod builder;
mod compiler;
mod config;
mod env;
mod error;
mod evaluator;
mod handlers;
mod manifest;
mod memory;
pub mod module_semantics;
mod modules;
mod native_fn;
mod overloaded_fn;
mod prelude;
mod stack;
mod util;

pub use builder::{
    core::Builder,
    core::NativeRegistration,
    export::{Export, ExportTarget, HostFnAsync, HostFnSync, NativeFuture},
};
pub use compiler::{CompileOptions, Compiler, program::CompiledProgram};
pub use config::{
    AsyncCallExecutor, AsyncCallPolicy, EngineOptions, ExecutionBounds, NativeAsyncPermit,
    ParallelismController, PreludeMode,
};
pub use error::{EngineError, ExecutionError, ModuleError};
pub use evaluator::{Evaluator, context::Context};
pub use handlers::{Rex, RexDefault};
pub use manifest::{MainInputSpec, MainSignature, Manifest, build_manifest, type_has_vars};
pub use memory::{
    heap::ValueDisplayOptions,
    traits::{FromRex, IntoRex},
    value::Value,
};
pub use modules::{
    CanonicalSymbol, CompilationPackage, Declarations, DenyImporter, ImportRequest, Importer,
    Module, ModuleExports, ModuleId, ModuleKey, PRELUDE_MODULE_NAME, ROOT_MODULE_NAME,
    ResolvedModule, ResolvedModuleContent, ResolvedRustModule, StagedAdtDecl, SymbolKind,
    virtual_export_name,
};
pub use prelude::{prelude_typeclasses_program, standard_type_system};
pub use util::collect_adts_error_to_engine;
