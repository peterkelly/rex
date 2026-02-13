use rex::{Engine, GasCosts, GasMeter, Parser, Token, Type, sym};
use rex_engine::assert_pointer_eq;

#[tokio::test]
async fn vec_from_value() {
    fn accept_vec(items: Vec<i32>) -> String {
        format!("accept_vec: {:?}", items)
    }

    let mut engine = Engine::with_prelude().unwrap();
    engine.inject_fn1("accept_vec", accept_vec).unwrap();

    let expr = r#"accept_vec (prim_array_from_list [1, 2, 3])"#;

    let tokens = Token::tokenize(expr).unwrap();
    let program = Parser::new(tokens).parse_program().unwrap();
    let mut gas = GasMeter::unlimited(GasCosts::sensible_defaults());
    let result = engine
        .eval_with_gas(program.expr.as_ref(), &mut gas)
        .await
        .unwrap();

    let heap = engine.heap();
    assert_pointer_eq!(
        heap,
        result,
        heap.alloc_string("accept_vec: [1, 2, 3]".to_string())
            .unwrap(),
    );
}

#[tokio::test]
async fn vec_to_value() {
    fn return_vec(input: String) -> Vec<i32> {
        let mut result: Vec<i32> = Vec::new();
        for i in 0..input.len() {
            result.push(i as i32);
        }
        result
    }

    let mut engine = Engine::with_prelude().unwrap();
    engine.inject_fn1("return_vec", return_vec).unwrap();

    let expr = r#"return_vec "hello""#;

    let tokens = Token::tokenize(expr).unwrap();
    let program = Parser::new(tokens).parse_program().unwrap();
    let mut gas = GasMeter::unlimited(GasCosts::sensible_defaults());
    let result = engine
        .eval_with_gas(program.expr.as_ref(), &mut gas)
        .await
        .unwrap();

    let heap = engine.heap();
    assert_pointer_eq!(
        heap,
        result,
        heap.alloc_array(vec![
            heap.alloc_i32(0).unwrap(),
            heap.alloc_i32(1).unwrap(),
            heap.alloc_i32(2).unwrap(),
            heap.alloc_i32(3).unwrap(),
            heap.alloc_i32(4).unwrap(),
        ])
        .unwrap()
    );
}

#[tokio::test]
async fn vec_rex_type() {
    fn return_vec(input: String) -> Vec<i32> {
        let mut result: Vec<i32> = Vec::new();
        for i in 0..input.len() {
            result.push(i as i32);
        }
        result
    }

    let mut engine = Engine::with_prelude().unwrap();
    engine.inject_fn1("return_vec", return_vec).unwrap();

    let expr = r#"return_vec "hello""#;

    let costs = GasCosts::sensible_defaults();
    let mut gas = GasMeter::unlimited(costs);
    let (_, ty) = engine.infer_snippet_with_gas(expr, &mut gas).unwrap();

    assert_eq!(ty, Type::app(Type::con("Array", 1), Type::con("i32", 0),));
}

#[tokio::test]
async fn option_prelude() {
    let expr = r#"(Some 4, None)"#;
    let mut engine = Engine::with_prelude().unwrap();
    let tokens = Token::tokenize(expr).unwrap();
    let program = Parser::new(tokens).parse_program().unwrap();
    let mut gas = GasMeter::unlimited(GasCosts::sensible_defaults());
    let result = engine
        .eval_with_gas(program.expr.as_ref(), &mut gas)
        .await
        .unwrap();
    let heap = engine.heap();
    assert_pointer_eq!(
        heap,
        result,
        heap.alloc_tuple(vec![
            heap.alloc_adt(sym("Some"), vec![heap.alloc_i32(4).unwrap()],)
                .unwrap(),
            heap.alloc_adt(sym("None"), vec![]).unwrap(),
        ])
        .unwrap()
    );
}

#[tokio::test]
async fn option_from_value() {
    fn accept_opt(opt: Option<i32>) -> String {
        format!("accept_opt: {:?}", opt)
    }

    let expr = r#"(accept_opt (Some 4), accept_opt None)"#;
    let mut engine = Engine::with_prelude().unwrap();
    engine.inject_fn1("accept_opt", accept_opt).unwrap();
    let tokens = Token::tokenize(expr).unwrap();
    let program = Parser::new(tokens).parse_program().unwrap();
    let mut gas = GasMeter::unlimited(GasCosts::sensible_defaults());
    let result = engine
        .eval_with_gas(program.expr.as_ref(), &mut gas)
        .await
        .unwrap();

    let heap = engine.heap();
    assert_pointer_eq!(
        heap,
        result,
        heap.alloc_tuple(vec![
            heap.alloc_string("accept_opt: Some(4)".to_string())
                .unwrap(),
            heap.alloc_string("accept_opt: None".to_string()).unwrap(),
        ])
        .unwrap(),
    );
}

#[tokio::test]
async fn option_into_value() {
    fn return_opt(s: String) -> Option<i32> {
        if s.is_empty() {
            None
        } else {
            Some(s.len() as i32)
        }
    }

    let expr = r#"(return_opt "hello", return_opt "")"#;
    let mut engine = Engine::with_prelude().unwrap();
    engine.inject_fn1("return_opt", return_opt).unwrap();
    let tokens = Token::tokenize(expr).unwrap();
    let program = Parser::new(tokens).parse_program().unwrap();
    let mut gas = GasMeter::unlimited(GasCosts::sensible_defaults());
    let result = engine
        .eval_with_gas(program.expr.as_ref(), &mut gas)
        .await
        .unwrap();

    let heap = engine.heap();
    assert_pointer_eq!(
        heap,
        result,
        heap.alloc_tuple(vec![
            heap.alloc_adt(sym("Some"), vec![heap.alloc_i32(5).unwrap()],)
                .unwrap(),
            heap.alloc_adt(sym("None"), vec![]).unwrap(),
        ])
        .unwrap(),
    );
}

#[tokio::test]
async fn option_rex_type() {
    fn return_opt(s: String) -> Option<i32> {
        if s.is_empty() {
            None
        } else {
            Some(s.len() as i32)
        }
    }

    let expr = r#"return_opt "hello""#;
    let mut engine = Engine::with_prelude().unwrap();
    engine.inject_fn1("return_opt", return_opt).unwrap();

    let costs = GasCosts::sensible_defaults();
    let mut gas = GasMeter::unlimited(costs);
    let (_, ty) = engine.infer_snippet_with_gas(expr, &mut gas).unwrap();

    assert_eq!(ty, Type::app(Type::con("Option", 1), Type::con("i32", 0),));
}
