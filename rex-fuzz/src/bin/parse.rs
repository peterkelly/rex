#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use rex_fuzz::{
    FuzzError, fuzz_source_input, parser_limits_from_env, read_stdin_bytes, run_with_stack,
    stack_bytes_from_env,
};
use rex_parser::parse_with_limits;

fn run_one(input: &[u8]) {
    let source = fuzz_source_input(input);
    let _ = parse_with_limits(&source, parser_limits_from_env());
}

fn main() -> Result<(), FuzzError> {
    let stack_bytes = stack_bytes_from_env(8);
    let input = read_stdin_bytes()?;
    run_with_stack("rex-fuzz-parse", stack_bytes, move || run_one(&input))
}
