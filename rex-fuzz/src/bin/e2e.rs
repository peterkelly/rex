#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use rex_ast::CompilationUnit;
use rex_engine::{Engine, Module};
use rex_fuzz::{FuzzError, fuzz_source_input, read_stdin_bytes};
use rex_parser::parse;
use rex_typesystem::{inference::infer, typesystem::TypeSystem};

async fn run_one(input: &[u8]) {
    let source = fuzz_source_input(input);
    let program = match parse(&source) {
        Ok(p) => p,
        Err(_) => return,
    };

    let Ok(mut ts) = TypeSystem::new_with_prelude() else {
        return;
    };
    if ts.register_decls(&program.decls).is_err() {
        return;
    }
    let Some(body) = program.body.as_ref() else {
        return;
    };
    if infer(&mut ts, body.as_ref()).is_err() {
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
    let mut compiler = engine.into_compiler();
    let body_program = CompilationUnit {
        decls: Vec::new(),
        body: Some(body.clone()),
    };
    if let Ok(compiled) = compiler
        .compile_program(&body_program, Default::default())
        .await
    {
        let _ = compiler
            .into_evaluator()
            .run(compiled, Default::default())
            .await;
    }
}

#[tokio::main]
async fn main() -> Result<(), FuzzError> {
    let input = read_stdin_bytes()?;
    run_one(&input).await;
    Ok(())
}
