#![forbid(unsafe_code)]

use std::io::Read;

use rex_gas::{GasCosts, GasMeter};
use rex_lexer::Token;
use rex_parser::{Parser, ParserLimits};

fn env_usize(name: &str) -> Option<usize> {
    std::env::var(name).ok().and_then(|v| v.parse().ok())
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| v.parse().ok())
}

fn run_one(input: &[u8]) {
    const MAX_BYTES: usize = 1 << 20; // 1MiB
    let input = &input[..input.len().min(MAX_BYTES)];
    let source = String::from_utf8_lossy(input);

    let tokens = match Token::tokenize(&source) {
        Ok(t) => t,
        Err(_) => return,
    };

    let mut parser = Parser::new(tokens);
    let limits = if let Some(max) = env_usize("REX_FUZZ_MAX_NESTING") {
        ParserLimits {
            max_nesting: Some(max),
        }
    } else {
        ParserLimits::safe_defaults()
    };
    parser.set_limits(limits);

    let mut gas = GasMeter::new(
        env_u64("REX_FUZZ_GAS").or(Some(120_000)),
        GasCosts::sensible_defaults(),
    );
    let _ = parser.parse_program_with_gas(&mut gas);
}

fn main() {
    let stack_mb = env_usize("REX_FUZZ_STACK_MB").unwrap_or(8);
    let stack_bytes = stack_mb.saturating_mul(1024 * 1024);

    let mut input = Vec::new();
    std::io::stdin()
        .read_to_end(&mut input)
        .expect("failed to read stdin");

    let handle = std::thread::Builder::new()
        .name("rex-fuzz-parse".to_string())
        .stack_size(stack_bytes)
        .spawn(move || run_one(&input))
        .expect("failed to spawn fuzz thread");

    handle.join().expect("fuzz thread panicked");
}

