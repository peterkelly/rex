#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod json;

pub use rex_ast::expr::{Decl, Expr, Program, Symbol, intern, sym};
pub use rex_engine::{
    AsyncHandler, AsyncNativeCallable, AsyncNativeCallableCancellable, ClassMethodCapability,
    ClassMethodRequirement, CompileError, CompiledExterns, CompiledProgram,
    CompiledProgramBoundary, Compiler, Engine, EngineError, EngineOptions, EvalError, Evaluator,
    EvaluatorRef, ExecutionError, Export, FromPointer, Handler, Heap, IntoPointer, Library,
    NativeCapability, NativeFuture, NativeRequirement, PRELUDE_LIBRARY_NAME, Pointer, PreludeMode,
    ROOT_LIBRARY_NAME, ReplState, ResolveRequest, ResolvedLibrary, ResolvedLibraryContent, RexAdt,
    RexDefault, RexType, RuntimeCapabilities, RuntimeCompatibility, RuntimeEnv, RuntimeEnvBoundary,
    RuntimeLinkContract, SyncNativeCallable, Value, ValueDisplayOptions, assert_pointer_eq,
    closure_debug, closure_eq, collect_adts_error_to_engine, pointer_display, pointer_display_with,
    value_debug, value_eq, virtual_export_name,
};
pub use rex_lexer::Token;
pub use rex_parser::{Parser, ParserLimits, error::ParserErr};
pub use rex_proc_macro::Rex;
pub use rex_typesystem::{
    error::{AdtConflict, CollectAdtsError, TypeError},
    inference::{infer, infer_typed, infer_typed_with_gas, infer_with_gas},
    prelude::prelude_typeclasses_program,
    types::{
        AdtDecl, AdtParam, AdtVariant, BuiltinTypeId, Instance, Predicate, Scheme, Type, TypeConst,
        TypeKind, TypeVar, collect_adts_in_types,
    },
    typesystem::{TypeSystem, TypeVarSupply},
};
pub use rex_util::{GasCosts, GasMeter};

pub use crate::json::{EnumPatch, JsonOptions, json_to_rex, rex_to_json};
