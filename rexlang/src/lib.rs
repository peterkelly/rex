pub use rexlang_core::*;

pub async fn eval(source: &str) -> Result<String, EngineError> {
    let tokens = Token::tokenize(source).map_err(|e| format!("lex error: {e}"))?;
    let mut parser = Parser::new(tokens);
    parser.set_limits(ParserLimits::unlimited());
    let mut gas = GasMeter::unlimited(GasCosts::sensible_defaults());

    let mut engine =
        Engine::with_prelude(()).map_err(|e| format!("failed to initialize engine: {e}"))?;
    engine.add_default_resolvers();

    let (pointer, _) = engine
        .evaluator()
        .eval_snippet(source, &mut gas)
        .await
        .map_err(|e| format!("{e}"))?;

    pointer_display_with(&engine.heap, &pointer, ValueDisplayOptions::default())
}
