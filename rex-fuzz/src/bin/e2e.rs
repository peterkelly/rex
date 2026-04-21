#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use rex_engine::{Engine, Library};
use rex_fuzz::{
    FuzzError, gas_meter_from_env, parser_limits_from_env, read_stdin_bytes, tokenize_fuzz_input,
};
use rex_parser::Parser;
use rex_typesystem::{inference::infer_with_gas, typesystem::TypeSystem};

async fn run_one(input: &[u8]) {
    let mut gas = gas_meter_from_env(300_000);
    let Some(tokens) = tokenize_fuzz_input(input) else {
        return;
    };
    let mut parser = Parser::new(tokens);
    parser.set_limits(parser_limits_from_env());

    let program = match parser.parse_program(&mut gas) {
        Ok(p) => p,
        Err(_) => return,
    };

    let Ok(mut ts) = TypeSystem::new_with_prelude() else {
        return;
    };
    if ts.register_decls(&program.decls).is_err() {
        return;
    }
    if infer_with_gas(&mut ts, program.expr.as_ref(), &mut gas).is_err() {
        return;
    }

    let Ok(mut engine) = Engine::with_prelude(()) else {
        return;
    };
    let mut library = Library::global();
    library.add_decls(program.decls.clone());
    if engine.inject_library(library).is_err() {
        return;
    }
    let _ = rex_engine::Evaluator::new_with_compiler(
        rex_engine::RuntimeEnv::new(engine.clone()),
        rex_engine::Compiler::new(engine.clone()),
    )
    .eval(program.expr.as_ref(), &mut gas)
    .await;
}

#[tokio::main]
async fn main() -> Result<(), FuzzError> {
    let input = read_stdin_bytes()?;
    run_one(&input).await;
    Ok(())
}
