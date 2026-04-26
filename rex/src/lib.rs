#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod json;

pub use rex_ast::expr::{Decl, Expr, Program, Symbol, intern, sym};
pub use rex_engine::{
    AsyncHandler, AsyncNativeCallable, AsyncNativeCallableCancellable, ClassMethodCapability,
    ClassMethodRequirement, CompileError, CompiledExterns, CompiledProgram,
    CompiledProgramBoundary, Compiler, DEFAULT_STACK_SIZE_BYTES, Engine, EngineError,
    EngineOptions, EvalError, Evaluator, EvaluatorRef, ExecutionError, Export, FrApp, FrAppArg,
    FrAppState, FrBool, FrBranchState, FrDateTime, FrDict, FrFloat, FrHole, FrInt, FrIte, FrLam,
    FrLet, FrLetRec, FrLetRecState, FrLetState, FrList, FrMatch, FrMatchArm, FrMatchState,
    FrProject, FrRecordUpdate, FrRecordUpdateState, FrSequenceState, FrString, FrTuple, FrUint,
    FrUuid, FrValueState, FrVar, Frame, FromPointer, Handler, Heap, IntoPointer, Module,
    NativeCapability, NativeFuture, NativeRequirement, PRELUDE_MODULE_NAME, Pointer, PreludeMode,
    ROOT_MODULE_NAME, ReplState, ResolveRequest, ResolvedModule, ResolvedModuleContent, RexAdt,
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

pub async fn eval(source: &str) -> Result<String, crate::ExecutionError> {
    let tokens = Token::tokenize(source).map_err(|e| {
        crate::CompileError::from(crate::EngineError::from(format!("lex error: {e}")))
    })?;
    let mut parser = Parser::new(tokens);
    parser.set_limits(ParserLimits::unlimited());
    let mut gas = GasMeter::unlimited(GasCosts::sensible_defaults());

    let mut engine = Engine::with_prelude(()).map_err(|e| {
        crate::CompileError::from(crate::EngineError::from(format!(
            "failed to initialize engine: {e}"
        )))
    })?;
    engine.add_default_resolvers();
    let mut compiler = Compiler::new(engine.clone());
    let runtime = RuntimeEnv::new(engine.clone());
    let program = compiler.compile_snippet(source, &mut gas)?;
    runtime.validate(&program)?;
    let mut evaluator = Evaluator::new(runtime);
    let pointer = evaluator.run(&program, &mut gas).await?;

    Ok(
        pointer_display_with(&engine.heap, &pointer, ValueDisplayOptions::default())
            .map_err(crate::EvalError::from)?,
    )
}
