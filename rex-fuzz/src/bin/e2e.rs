#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use rex_engine::{Engine, Heap};
use rex_fuzz::{
    FuzzError, gas_meter_from_env, parser_limits_from_env, read_stdin_bytes, run_with_stack,
    stack_bytes_from_env, tokenize_fuzz_input,
};
use rex_parser::Parser;
use rex_ts::TypeSystem;

fn run_one(input: &[u8]) {
    let mut gas = gas_meter_from_env(300_000);
    let Some(tokens) = tokenize_fuzz_input(input) else {
        return;
    };
    let mut parser = Parser::new(tokens);
    parser.set_limits(parser_limits_from_env());

    let program = match parser.parse_program_with_gas(&mut gas) {
        Ok(p) => p,
        Err(_) => return,
    };

    let Ok(mut ts) = TypeSystem::with_prelude() else {
        return;
    };
    if ts.inject_decls(&program.decls).is_err() {
        return;
    }
    if ts.infer_with_gas(program.expr.as_ref(), &mut gas).is_err() {
        return;
    }

    let heap = Heap::new();
    let Ok(mut engine) = Engine::with_prelude(&heap) else {
        return;
    };
    if engine.inject_decls(&program.decls).is_err() {
        return;
    }
    let _ = engine.eval_with_gas(program.expr.as_ref(), &mut gas);
}

fn main() -> Result<(), FuzzError> {
    let stack_bytes = stack_bytes_from_env(16);
    let input = read_stdin_bytes()?;
    run_with_stack("rex-fuzz-e2e", stack_bytes, move || run_one(&input))
}
