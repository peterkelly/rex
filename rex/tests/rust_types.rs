use rex::{Engine, GasCosts, GasMeter, Parser, Token, Type, Value, sym};

#[test]
fn vec_from_value() {
    fn accept_vec(items: Vec<i32>) -> String {
        format!("accept_vec: {:?}", items)
    }

    let mut engine = Engine::with_prelude().unwrap();
    engine.inject_fn1("accept_vec", accept_vec).unwrap();

    let expr = r#"accept_vec (prim_array_from_list [1, 2, 3])"#;

    let tokens = Token::tokenize(expr).unwrap();
    let program = Parser::new(tokens).parse_program().unwrap();
    let result = engine.eval(program.expr.as_ref()).unwrap();

    assert_eq!(result, Value::String("accept_vec: [1, 2, 3]".to_string()),);
}

#[test]
fn vec_to_value() {
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
    let result = engine.eval(program.expr.as_ref()).unwrap();

    assert_eq!(
        result,
        Value::Array(vec![
            Value::I32(0),
            Value::I32(1),
            Value::I32(2),
            Value::I32(3),
            Value::I32(4),
        ])
    );
}

#[test]
fn vec_rex_type() {
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

#[test]
fn option_prelude() {
    let expr = r#"(Some 4, None)"#;
    let mut engine = Engine::with_prelude().unwrap();
    let tokens = Token::tokenize(expr).unwrap();
    let program = Parser::new(tokens).parse_program().unwrap();
    let result = engine.eval(program.expr.as_ref()).unwrap();
    assert_eq!(
        result,
        Value::Tuple(vec![
            Value::Adt(sym("Some"), vec![Value::I32(4)]),
            Value::Adt(sym("None"), vec![]),
        ])
    );
}

#[test]
fn option_from_value() {
    fn accept_opt(opt: Option<i32>) -> String {
        format!("accept_opt: {:?}", opt)
    }

    let expr = r#"(accept_opt (Some 4), accept_opt None)"#;
    let mut engine = Engine::with_prelude().unwrap();
    engine.inject_fn1("accept_opt", accept_opt).unwrap();
    let tokens = Token::tokenize(expr).unwrap();
    let program = Parser::new(tokens).parse_program().unwrap();
    let result = engine.eval(program.expr.as_ref()).unwrap();

    assert_eq!(
        result,
        Value::Tuple(vec![
            Value::String("accept_opt: Some(4)".to_string()),
            Value::String("accept_opt: None".to_string()),
        ]),
    );
}

#[test]
fn option_into_value() {
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
    let result = engine.eval(program.expr.as_ref()).unwrap();

    assert_eq!(
        result,
        Value::Tuple(vec![
            Value::Adt(sym("Some"), vec![Value::I32(5)]),
            Value::Adt(sym("None"), vec![]),
        ]),
    );
}

#[test]
fn option_rex_type() {
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
