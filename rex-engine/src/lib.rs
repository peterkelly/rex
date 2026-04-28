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
    ClassMethodCapability, ClassMethodRequirement, CompiledExterns, CompiledProgram,
    CompiledProgramBoundary, Engine, EngineOptions, Export, HostFnAsync, HostFnSync,
    NativeCapability, NativeFuture, NativeRequirement, PRELUDE_MODULE_NAME, PreludeMode,
    ROOT_MODULE_NAME, RexAdt, RexDefault, RuntimeCapabilities, RuntimeCompatibility,
    RuntimeLinkContract, collect_adts_error_to_engine,
};
pub use env::Environment;
pub use error::{CompileError, EngineError, EvalError, ExecutionError, ModuleError};
pub use evaluator::{Evaluator, EvaluatorRef};
pub use modules::virtual_export_name;
pub use modules::{
    CanonicalSymbol, Module, ModuleExports, ModuleId, ModuleInstance, ModuleKey, ReplState,
    ResolveRequest, ResolvedModule, ResolvedModuleContent, SymbolKind,
};
pub use runtime_env::{RuntimeEnv, RuntimeEnvBoundary};
pub use value::{FromRex, Handle, Heap, IntoRex, RexType, Value, ValueDisplayOptions};
