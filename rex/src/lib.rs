#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod json;

pub use crate::json::{EnumPatch, JsonOptions, json_to_rex, rex_to_json};
pub use rex_ast::expr::{Decl, Expr, Program, Symbol, intern, sym};
pub use rex_engine::{
    ClassMethodCapability, ClassMethodRequirement, CompileError, CompiledExterns, CompiledProgram,
    CompiledProgramBoundary, Compiler, Engine, EngineError, EngineOptions, EvalError, Evaluator,
    EvaluatorRef, ExecutionError, Export, FromRex, Handle, Heap, HostFnAsync, HostFnSync, IntoRex,
    Module, NativeCapability, NativeFuture, NativeRequirement, PRELUDE_MODULE_NAME, PreludeMode,
    ROOT_MODULE_NAME, ReplState, ResolveRequest, ResolvedModule, ResolvedModuleContent, RexAdt,
    RexDefault, RexType, RuntimeCapabilities, RuntimeCompatibility, RuntimeEnv, RuntimeEnvBoundary,
    RuntimeLinkContract, Value, ValueDisplayOptions, collect_adts_error_to_engine,
    virtual_export_name,
};
pub use rex_lexer::Token;
pub use rex_parser::{Parser, ParserLimits, error::ParserErr};
pub use rex_proc_macro::Rex;
pub use rex_typesystem::{
    error::{AdtConflict, CollectAdtsError, TypeError},
    inference::{infer, infer_typed},
    prelude::prelude_typeclasses_program,
    types::{
        AdtDecl, AdtParam, AdtVariant, BuiltinTypeId, Instance, Predicate, Scheme, Type, TypeConst,
        TypeKind, TypeVar, collect_adts_in_types,
    },
    typesystem::{TypeSystem, TypeVarSupply},
};

pub async fn eval(source: &str) -> Result<String, crate::ExecutionError> {
    let tokens = Token::tokenize(source).map_err(|e| {
        crate::CompileError::from(crate::EngineError::from(format!("lex error: {e}")))
    })?;
    let mut parser = Parser::new(tokens);
    parser.set_limits(ParserLimits::unlimited());

    let mut engine = Engine::with_prelude(()).map_err(|e| {
        crate::CompileError::from(crate::EngineError::from(format!(
            "failed to initialize engine: {e}"
        )))
    })?;
    engine.add_default_resolvers();
    let mut compiler = Compiler::new(engine.clone());
    let runtime = RuntimeEnv::new(engine.clone());
    let program = compiler.compile_snippet(source)?;
    runtime.validate(&program)?;
    let mut evaluator = Evaluator::new(runtime);
    let value = evaluator.run(&program).await?;

    Ok(value
        .display_with(ValueDisplayOptions::default())
        .map_err(crate::EvalError::from)?)
}
