#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod ast;
pub mod engine;
pub mod json;
pub mod parser;
pub mod typesystem;

pub use rex_proc_macro::Rex;

pub async fn eval(source: &str) -> Result<String, engine::ExecutionError> {
    let tokens = parser::Token::tokenize(source).map_err(|e| {
        engine::CompileError::from(engine::EngineError::from(format!("lex error: {e}")))
    })?;
    let mut parser = parser::Parser::new(tokens);
    parser.set_limits(parser::ParserLimits::unlimited());

    let mut engine = engine::Engine::with_prelude(()).map_err(|e| {
        engine::CompileError::from(engine::EngineError::from(format!(
            "failed to initialize engine: {e}"
        )))
    })?;
    engine.add_default_resolvers();
    let mut compiler = engine.into_compiler();
    let program = compiler.compile_snippet(source)?;
    let evaluator = compiler.into_evaluator();
    let value = evaluator.run(program).await?;

    Ok(value
        .display_with(engine::ValueDisplayOptions::default())
        .map_err(engine::EvalError::from)?)
}
