pub use rex_core::*;

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
