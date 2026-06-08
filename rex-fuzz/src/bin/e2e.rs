#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use rex_engine::{Builder, CompileOptions, standard_type_system};
use rex_fuzz::{FuzzError, fuzz_source_input, read_stdin_bytes};
use rex_parser::parse;
use rex_typesystem::inference::infer;

async fn run_one(input: &[u8]) {
    let source = fuzz_source_input(input);
    let program = match parse(&source) {
        Ok(p) => p,
        Err(_) => return,
    };

    let Ok(mut ts) = standard_type_system() else {
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

    let Ok(builder) = Builder::with_prelude(()) else {
        return;
    };
    let compiler = builder.build_compiler();
    let Ok(options) = CompileOptions::for_module("fuzz.main") else {
        return;
    };
    if let Ok((compiled, evaluator)) = compiler.compile_program(&program, options).await {
        let _ = evaluator.run(compiled, Default::default()).await;
    }
}

#[tokio::main]
async fn main() -> Result<(), FuzzError> {
    let input = read_stdin_bytes()?;
    run_one(&input).await;
    Ok(())
}
