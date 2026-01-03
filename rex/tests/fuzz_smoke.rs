use std::panic::{AssertUnwindSafe, catch_unwind};

use rex_ast::expr::Decl;
use rex_engine::Engine;
use rex_gas::{GasCosts, GasMeter};
use rex_lexer::Token;
use rex_parser::{Parser, ParserLimits};
use rex_ts::TypeSystem;

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

#[derive(Clone)]
struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn gen_range(&mut self, lo: usize, hi: usize) -> usize {
        let span = hi.saturating_sub(lo).max(1);
        lo + (self.next_u64() as usize % span)
    }
}

#[test]
fn fuzz_smoke_pipeline_does_not_panic() {
    let iters: usize = std::env::var("REX_FUZZ_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(200);

    let charset: &[char] = &[
        ' ', '\n', '\t', 'a', 'b', 'c', 'x', 'y', 'z', 'A', 'B', 'C', '_', '0', '1', '2', '3',
        '(', ')', '[', ']', '{', '}', ',', ':', '=', '\\', '-', '>', '+', '*', '/', '%', '<', '>',
        '.', '"', '\'', '|', '&', 'λ', '→',
    ];

    let mut rng = XorShift64::new(0x7265_785f_6675_7a7a);
    for i in 0..iters {
        let len = rng.gen_range(0, 250);
        let mut s = String::with_capacity(len);
        for _ in 0..len {
            let idx = rng.gen_range(0, charset.len());
            s.push(charset[idx]);
        }

        let res = catch_unwind(AssertUnwindSafe(|| {
            let tokens = match Token::tokenize(&s) {
                Ok(t) => t,
                Err(_) => return,
            };

            let mut gas = GasMeter::new(Some(200_000), GasCosts::sensible_defaults());

            let mut parser = Parser::new(tokens);
            parser.set_limits(ParserLimits::safe_defaults());
            let program = match parser.parse_program_with_gas(&mut gas) {
                Ok(p) => p,
                Err(_) => return,
            };

            let mut ts = TypeSystem::with_prelude();
            let _ = inject_type_env_decls(&mut ts, &program.decls);
            let _ = ts.infer_with_gas(program.expr.as_ref(), &mut gas);

            let mut engine = Engine::with_prelude();
            let _ = engine.inject_decls(&program.decls);
            let _ = engine.eval_with_gas(program.expr.as_ref(), &mut gas);
        }));

        if res.is_err() {
            panic!("panic in fuzz iteration {i} with input:\n{s}");
        }
    }
}
