use rex::{Engine, GasCosts, GasMeter, Parser, ParserLimits, Token, TypeSystem};

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

#[tokio::test]
async fn fuzz_smoke_pipeline_does_not_panic() {
    let iters: usize = std::env::var("REX_FUZZ_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(200);

    let charset: &[char] = &[
        ' ', '\n', '\t', 'a', 'b', 'c', 'x', 'y', 'z', 'A', 'B', 'C', '_', '0', '1', '2', '3', '(',
        ')', '[', ']', '{', '}', ',', ':', '=', '\\', '-', '>', '+', '*', '/', '%', '<', '>', '.',
        '"', '\'', '|', '&', 'λ', '→',
    ];

    let mut rng = XorShift64::new(0x7265_785f_6675_7a7a);
    for _ in 0..iters {
        let len = rng.gen_range(0, 250);
        let mut s = String::with_capacity(len);
        for _ in 0..len {
            let idx = rng.gen_range(0, charset.len());
            s.push(charset[idx]);
        }

        let input = s.clone();
        let tokens = match Token::tokenize(&input) {
            Ok(t) => t,
            Err(_) => continue,
        };

        let mut gas = GasMeter::new(Some(200_000), GasCosts::sensible_defaults());

        let mut parser = Parser::new(tokens);
        parser.set_limits(ParserLimits::safe_defaults());
        let program = match parser.parse_program_with_gas(&mut gas) {
            Ok(p) => p,
            Err(_) => continue,
        };

        let mut ts = TypeSystem::with_prelude().unwrap();
        let _ = ts.inject_decls(&program.decls);
        let _ = ts.infer_with_gas(program.expr.as_ref(), &mut gas);

        let mut engine = Engine::with_prelude(()).unwrap();
        let _ = engine.inject_decls(&program.decls);
        let _ = engine.eval(program.expr.as_ref(), &mut gas).await;
    }
}
