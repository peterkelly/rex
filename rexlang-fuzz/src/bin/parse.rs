#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use rexlang_fuzz::{
    FuzzError, gas_meter_from_env, parser_limits_from_env, read_stdin_bytes, run_with_stack,
    stack_bytes_from_env, tokenize_fuzz_input,
};
use rexlang_parser::Parser;

fn run_one(input: &[u8]) {
    let Some(tokens) = tokenize_fuzz_input(input) else {
        return;
    };
    let mut parser = Parser::new(tokens);
    parser.set_limits(parser_limits_from_env());
    let mut gas = gas_meter_from_env(120_000);
    let _ = parser.parse_program(&mut gas);
}

fn main() -> Result<(), FuzzError> {
    let stack_bytes = stack_bytes_from_env(8);
    let input = read_stdin_bytes()?;
    run_with_stack("rexlang-fuzz-parse", stack_bytes, move || run_one(&input))
}
