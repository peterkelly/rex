#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

//! Evaluation engine for Rex.

mod compiler;
mod engine;
mod env;
mod error;
mod evaluator;
mod modules;
mod prelude;
mod runtime_env;
mod stack;
mod value;

pub use compiler::Compiler;
pub use engine::{
    AsyncCallExecutor, AsyncCallPolicy, ClassMethodCapability, ClassMethodRequirement,
    CompiledExterns, CompiledProgram, CompiledProgramBoundary, Engine, EngineOptions,
    ExecutionBounds, Export, HostFnAsync, HostFnSync, NativeCapability, NativeFuture,
    NativeRequirement, PRELUDE_MODULE_NAME, PreludeMode, ROOT_MODULE_NAME, RexDefault,
    RuntimeCapabilities, RuntimeCompatibility, RuntimeLinkContract, collect_adts_error_to_engine,
};
pub use env::Environment;
pub(crate) use env::RootedEnvironment;
pub use error::{CompileError, EngineError, EvalError, ExecutionError, ModuleError};
pub use evaluator::{Context, Evaluator};
pub use modules::virtual_export_name;
pub use modules::{
    CanonicalSymbol, DenyImporter, ImportRequest, Importer, Module, ModuleExports, ModuleId,
    ModuleInstance, ModuleKey, ResolvedModule, ResolvedModuleContent, StdlibImporter, SymbolKind,
};
pub use runtime_env::{RuntimeEnv, RuntimeEnvBoundary};
pub use value::{FromRex, Handle, Heap, IntoRex, Value, ValueDisplayOptions};
