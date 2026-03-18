#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use std::io::Read;

use rexlang_lexer::{Token, Tokens};
use rexlang_parser::ParserLimits;
use rexlang_util::{GasCosts, GasMeter};

pub const MAX_FUZZ_BYTES: usize = 1 << 20; // 1MiB

pub fn env_usize(name: &str) -> Option<usize> {
    std::env::var(name).ok().and_then(|v| v.parse().ok())
}

pub fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| v.parse().ok())
}

pub fn read_stdin_bytes() -> Result<Vec<u8>, std::io::Error> {
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input)?;
    Ok(input)
}

pub fn stack_bytes_from_env(default_mb: usize) -> usize {
    let stack_mb = env_usize("REX_FUZZ_STACK_MB").unwrap_or(default_mb);
    stack_mb.saturating_mul(1024 * 1024)
}

pub fn parser_limits_from_env() -> ParserLimits {
    if let Some(max) = env_usize("REX_FUZZ_MAX_NESTING") {
        ParserLimits {
            max_nesting: Some(max),
        }
    } else {
        ParserLimits::safe_defaults()
    }
}

pub fn gas_meter_from_env(default: u64) -> GasMeter {
    GasMeter::new(
        env_u64("REX_FUZZ_GAS").or(Some(default)),
        GasCosts::sensible_defaults(),
    )
}

pub fn tokenize_fuzz_input(input: &[u8]) -> Option<Tokens> {
    let input = &input[..input.len().min(MAX_FUZZ_BYTES)];
    let source = String::from_utf8_lossy(input);
    Token::tokenize(&source).ok()
}

#[derive(Debug, thiserror::Error)]
pub enum FuzzError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("fuzz thread panicked")]
    ThreadPanic,
}

pub fn run_with_stack(
    thread_name: &str,
    stack_bytes: usize,
    f: impl FnOnce() + Send + 'static,
) -> Result<(), FuzzError> {
    let handle = std::thread::Builder::new()
        .name(thread_name.to_string())
        .stack_size(stack_bytes)
        .spawn(f)?;
    handle.join().map_err(|_| FuzzError::ThreadPanic)?;
    Ok(())
}
