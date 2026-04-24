#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

//! Evaluation engine for Rex.

mod cancel;
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

pub use cancel::CancellationToken;
pub use compiler::Compiler;
pub use engine::{
    AsyncHandler, AsyncNativeCallable, AsyncNativeCallableCancellable, ClassMethodCapability,
    ClassMethodRequirement, CompiledExterns, CompiledProgram, CompiledProgramBoundary, Engine,
    EngineOptions, Export, Handler, NativeCapability, NativeFn, NativeFuture, NativeRequirement,
    OverloadedFn, PRELUDE_MODULE_NAME, PreludeMode, ROOT_MODULE_NAME, RexAdt, RexDefault,
    RuntimeCapabilities, RuntimeCompatibility, RuntimeLinkContract, SyncNativeCallable,
    collect_adts_error_to_engine,
};
pub use env::Env;
pub use error::{CompileError, EngineError, EvalError, ExecutionError, ModuleError};
pub use evaluator::{Evaluator, EvaluatorRef};
pub use modules::virtual_export_name;
pub use modules::{
    CanonicalSymbol, Module, ModuleExports, ModuleId, ModuleInstance, ModuleKey, ReplState,
    ResolveRequest, ResolvedModule, ResolvedModuleContent, SymbolKind,
};
pub use runtime_env::{RuntimeEnv, RuntimeEnvBoundary};
pub use stack::DEFAULT_STACK_SIZE_BYTES;
pub use value::{
    Closure, FromPointer, Heap, IntoPointer, Pointer, RexType, Value, ValueDisplayOptions,
    ValueRef, closure_debug, closure_eq, pointer_display, pointer_display_with, pointer_eq,
    value_debug, value_eq,
};

#[macro_export]
macro_rules! assert_pointer_eq {
    ($heap:expr, $left:expr, $right:expr $(,)?) => {{
        let __heap = $heap;
        match (&$left, &$right) {
            (__left, __right) => {
                let __left_ptr: &$crate::Pointer = __left;
                let __right_ptr: &$crate::Pointer = __right;
                let __equal =
                    $crate::pointer_eq(__heap, __left_ptr, __right_ptr).unwrap_or_else(|err| {
                        panic!("assert_pointer_eq failed to compare pointers: {err}")
                    });
                if !__equal {
                    let __left_value = __heap.get(__left_ptr).unwrap_or_else(|err| {
                        panic!("assert_pointer_eq failed to dereference left pointer: {err}")
                    });
                    let __right_value = __heap.get(__right_ptr).unwrap_or_else(|err| {
                        panic!("assert_pointer_eq failed to dereference right pointer: {err}")
                    });
                    let __left_dbg = $crate::value_debug(__heap, __left_value.as_ref())
                        .unwrap_or_else(|err| format!("<value_debug error: {err}>"));
                    let __right_dbg = $crate::value_debug(__heap, __right_value.as_ref())
                        .unwrap_or_else(|err| format!("<value_debug error: {err}>"));
                    panic!(
                        "assertion `pointer values are equal` failed\n  left: {}\n right: {}",
                        __left_dbg, __right_dbg
                    );
                }
            }
        }
    }};
}
