#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use rex_engine::{Engine, Module};
use rex_fuzz::{FuzzError, parser_limits_from_env, read_stdin_bytes, tokenize_fuzz_input};
use rex_parser::Parser;
use rex_typesystem::{inference::infer, typesystem::TypeSystem};

async fn run_one(input: &[u8]) {
    let Some(tokens) = tokenize_fuzz_input(input) else {
        return;
    };
    let mut parser = Parser::new(tokens);
    parser.set_limits(parser_limits_from_env());

    let program = match parser.parse_program() {
        Ok(p) => p,
        Err(_) => return,
    };

    let Ok(mut ts) = TypeSystem::new_with_prelude() else {
        return;
    };
    if ts.register_decls(&program.decls).is_err() {
        return;
    }
    if infer(&mut ts, program.expr.as_ref()).is_err() {
        return;
    }

    let Ok(mut engine) = Engine::with_prelude(()) else {
        return;
    };
    let mut module = Module::global();
    module.add_decls(program.decls.clone());
    if engine.inject_module(module).is_err() {
        return;
    }
    let _ = engine.into_evaluator().eval(program.expr.as_ref()).await;
}

#[tokio::main]
async fn main() -> Result<(), FuzzError> {
    let input = read_stdin_bytes()?;
    run_one(&input).await;
    Ok(())
}
