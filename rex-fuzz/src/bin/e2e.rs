#![forbid(unsafe_code)]

use std::io::Read;

use rex_ast::expr::Decl;
use rex_engine::Engine;
use rex_gas::{GasCosts, GasMeter};
use rex_lexer::Token;
use rex_parser::{Parser, ParserLimits};
use rex_ts::TypeSystem;

fn env_usize(name: &str) -> Option<usize> {
    std::env::var(name).ok().and_then(|v| v.parse().ok())
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| v.parse().ok())
}

fn inject_type_env_decls(ts: &mut TypeSystem, decls: &[Decl]) -> Result<(), rex_ts::TypeError> {
    for decl in decls {
        match decl {
            Decl::Type(ty) => ts.inject_type_decl(ty)?,
            Decl::Class(class_decl) => ts.inject_class_decl(class_decl)?,
            Decl::Instance(inst_decl) => {
                ts.inject_instance_decl(inst_decl)?;
            }
            Decl::Fn(fd) => ts.inject_fn_decl(fd)?,
        }
    }
    Ok(())
}

fn run_one(input: &[u8]) {
    const MAX_BYTES: usize = 1 << 20; // 1MiB
    let input = &input[..input.len().min(MAX_BYTES)];
    let source = String::from_utf8_lossy(input);

    let mut gas = GasMeter::new(
        env_u64("REX_FUZZ_GAS").or(Some(300_000)),
        GasCosts::sensible_defaults(),
    );

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

    let program = match parser.parse_program_with_gas(&mut gas) {
        Ok(p) => p,
        Err(_) => return,
    };

    let mut ts = TypeSystem::with_prelude();
    if inject_type_env_decls(&mut ts, &program.decls).is_err() {
        return;
    }
    if ts.infer_with_gas(program.expr.as_ref(), &mut gas).is_err() {
        return;
    }

    let mut engine = Engine::with_prelude();
    if engine.inject_decls(&program.decls).is_err() {
        return;
    }
    let _ = engine.eval_with_gas(program.expr.as_ref(), &mut gas);
}

fn main() {
    let stack_mb = env_usize("REX_FUZZ_STACK_MB").unwrap_or(16);
    let stack_bytes = stack_mb.saturating_mul(1024 * 1024);

    let mut input = Vec::new();
    std::io::stdin()
        .read_to_end(&mut input)
        .expect("failed to read stdin");

    let handle = std::thread::Builder::new()
        .name("rex-fuzz-e2e".to_string())
        .stack_size(stack_bytes)
        .spawn(move || run_one(&input))
        .expect("failed to spawn fuzz thread");

    handle.join().expect("fuzz thread panicked");
}

