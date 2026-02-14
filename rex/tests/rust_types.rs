use rex::{Engine, FromPointer, GasCosts, GasMeter, Parser, Token, Type, sym};
use rex_engine::assert_pointer_eq;
use rex_proc_macro::Rex;

#[tokio::test]
async fn vec_from_value() {
    fn accept_vec(_state: &(), items: Vec<i32>) -> String {
        format!("accept_vec: {:?}", items)
    }

    let mut engine = Engine::with_prelude(()).unwrap();
    engine.inject_fn1("accept_vec", accept_vec).unwrap();

    let expr = r#"accept_vec (prim_array_from_list [1, 2, 3])"#;

    let tokens = Token::tokenize(expr).unwrap();
    let program = Parser::new(tokens).parse_program().unwrap();
    let mut gas = GasMeter::unlimited(GasCosts::sensible_defaults());
    let result = engine.eval(program.expr.as_ref(), &mut gas).await.unwrap();

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
    fn return_vec(_state: &(), input: String) -> Vec<i32> {
        let mut result: Vec<i32> = Vec::new();
        for i in 0..input.len() {
            result.push(i as i32);
        }
        result
    }

    let mut engine = Engine::with_prelude(()).unwrap();
    engine.inject_fn1("return_vec", return_vec).unwrap();

    let expr = r#"return_vec "hello""#;

    let tokens = Token::tokenize(expr).unwrap();
    let program = Parser::new(tokens).parse_program().unwrap();
    let mut gas = GasMeter::unlimited(GasCosts::sensible_defaults());
    let result = engine.eval(program.expr.as_ref(), &mut gas).await.unwrap();

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
    fn return_vec(_state: &(), input: String) -> Vec<i32> {
        let mut result: Vec<i32> = Vec::new();
        for i in 0..input.len() {
            result.push(i as i32);
        }
        result
    }

    let mut engine = Engine::with_prelude(()).unwrap();
    engine.inject_fn1("return_vec", return_vec).unwrap();

    let expr = r#"return_vec "hello""#;

    let costs = GasCosts::sensible_defaults();
    let mut gas = GasMeter::unlimited(costs);
    let (_, ty) = engine.infer_snippet(expr, &mut gas).unwrap();

    assert_eq!(ty, Type::app(Type::con("Array", 1), Type::con("i32", 0),));
}

#[tokio::test]
async fn option_prelude() {
    let expr = r#"(Some 4, None)"#;
    let mut engine = Engine::with_prelude(()).unwrap();
    let tokens = Token::tokenize(expr).unwrap();
    let program = Parser::new(tokens).parse_program().unwrap();
    let mut gas = GasMeter::unlimited(GasCosts::sensible_defaults());
    let result = engine.eval(program.expr.as_ref(), &mut gas).await.unwrap();
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
    fn accept_opt(_state: &(), opt: Option<i32>) -> String {
        format!("accept_opt: {:?}", opt)
    }

    let expr = r#"(accept_opt (Some 4), accept_opt None)"#;
    let mut engine = Engine::with_prelude(()).unwrap();
    engine.inject_fn1("accept_opt", accept_opt).unwrap();
    let tokens = Token::tokenize(expr).unwrap();
    let program = Parser::new(tokens).parse_program().unwrap();
    let mut gas = GasMeter::unlimited(GasCosts::sensible_defaults());
    let result = engine.eval(program.expr.as_ref(), &mut gas).await.unwrap();

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
    fn return_opt(_state: &(), s: String) -> Option<i32> {
        if s.is_empty() {
            None
        } else {
            Some(s.len() as i32)
        }
    }

    let expr = r#"(return_opt "hello", return_opt "")"#;
    let mut engine = Engine::with_prelude(()).unwrap();
    engine.inject_fn1("return_opt", return_opt).unwrap();
    let tokens = Token::tokenize(expr).unwrap();
    let program = Parser::new(tokens).parse_program().unwrap();
    let mut gas = GasMeter::unlimited(GasCosts::sensible_defaults());
    let result = engine.eval(program.expr.as_ref(), &mut gas).await.unwrap();

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
    fn return_opt(_state: &(), s: String) -> Option<i32> {
        if s.is_empty() {
            None
        } else {
            Some(s.len() as i32)
        }
    }

    let expr = r#"return_opt "hello""#;
    let mut engine = Engine::with_prelude(()).unwrap();
    engine.inject_fn1("return_opt", return_opt).unwrap();

    let costs = GasCosts::sensible_defaults();
    let mut gas = GasMeter::unlimited(costs);
    let (_, ty) = engine.infer_snippet(expr, &mut gas).unwrap();

    assert_eq!(ty, Type::app(Type::con("Option", 1), Type::con("i32", 0),));
}

#[tokio::test]
async fn result_prelude() {
    let expr = r#"(Ok 42, Err "error")"#;
    let mut engine = Engine::with_prelude(()).unwrap();
    let tokens = Token::tokenize(expr).unwrap();
    let program = Parser::new(tokens).parse_program().unwrap();
    let mut gas = GasMeter::unlimited(GasCosts::sensible_defaults());
    let result = engine.eval(program.expr.as_ref(), &mut gas).await.unwrap();
    let heap = engine.heap();
    assert_pointer_eq!(
        heap,
        result,
        heap.alloc_tuple(vec![
            heap.alloc_adt(sym("Ok"), vec![heap.alloc_i32(42).unwrap()])
                .unwrap(),
            heap.alloc_adt(
                sym("Err"),
                vec![heap.alloc_string("error".to_string()).unwrap()]
            )
            .unwrap(),
        ])
        .unwrap()
    );
}

#[tokio::test]
async fn result_from_value_primitives() {
    fn accept_result(_state: &(), res: Result<i32, String>) -> String {
        format!("accept_result: {:?}", res)
    }

    let expr = r#"(accept_result (Ok 42), accept_result (Err "failed"))"#;
    let mut engine = Engine::with_prelude(()).unwrap();
    engine.inject_fn1("accept_result", accept_result).unwrap();
    let tokens = Token::tokenize(expr).unwrap();
    let program = Parser::new(tokens).parse_program().unwrap();
    let mut gas = GasMeter::unlimited(GasCosts::sensible_defaults());
    let result = engine.eval(program.expr.as_ref(), &mut gas).await.unwrap();

    let heap = engine.heap();
    assert_pointer_eq!(
        heap,
        result,
        heap.alloc_tuple(vec![
            heap.alloc_string("accept_result: Ok(42)".to_string())
                .unwrap(),
            heap.alloc_string("accept_result: Err(\"failed\")".to_string())
                .unwrap(),
        ])
        .unwrap(),
    );
}

#[tokio::test]
async fn result_from_value_different_primitives() {
    fn accept_result(_state: &(), res: Result<f32, i32>) -> String {
        format!("accept_result: {:?}", res)
    }

    let expr = r#"(accept_result (Ok 3.14), accept_result (Err 404))"#;
    let mut engine = Engine::with_prelude(()).unwrap();
    engine.inject_fn1("accept_result", accept_result).unwrap();
    let tokens = Token::tokenize(expr).unwrap();
    let program = Parser::new(tokens).parse_program().unwrap();
    let mut gas = GasMeter::unlimited(GasCosts::sensible_defaults());
    let result = engine.eval(program.expr.as_ref(), &mut gas).await.unwrap();

    let heap = engine.heap();
    assert_pointer_eq!(
        heap,
        result,
        heap.alloc_tuple(vec![
            heap.alloc_string("accept_result: Ok(3.14)".to_string())
                .unwrap(),
            heap.alloc_string("accept_result: Err(404)".to_string())
                .unwrap(),
        ])
        .unwrap(),
    );
}

#[tokio::test]
async fn result_into_value_primitives() {
    fn return_result(_state: &(), s: String) -> Result<i32, String> {
        if s.is_empty() {
            Err("empty string".to_string())
        } else {
            Ok(s.len() as i32)
        }
    }

    let expr = r#"(return_result "hello", return_result "")"#;
    let mut engine = Engine::with_prelude(()).unwrap();
    engine.inject_fn1("return_result", return_result).unwrap();
    let tokens = Token::tokenize(expr).unwrap();
    let program = Parser::new(tokens).parse_program().unwrap();
    let mut gas = GasMeter::unlimited(GasCosts::sensible_defaults());
    let result = engine.eval(program.expr.as_ref(), &mut gas).await.unwrap();

    let heap = engine.heap();
    assert_pointer_eq!(
        heap,
        result,
        heap.alloc_tuple(vec![
            heap.alloc_adt(sym("Ok"), vec![heap.alloc_i32(5).unwrap()])
                .unwrap(),
            heap.alloc_adt(
                sym("Err"),
                vec![heap.alloc_string("empty string".to_string()).unwrap()]
            )
            .unwrap(),
        ])
        .unwrap(),
    );
}

#[tokio::test]
async fn result_rex_type() {
    fn return_result(_state: &(), s: String) -> Result<i32, String> {
        if s.is_empty() {
            Err("empty string".to_string())
        } else {
            Ok(s.len() as i32)
        }
    }

    let expr = r#"return_result "hello""#;
    let mut engine = Engine::with_prelude(()).unwrap();
    engine.inject_fn1("return_result", return_result).unwrap();

    let costs = GasCosts::sensible_defaults();
    let mut gas = GasMeter::unlimited(costs);
    let (_, ty) = engine.infer_snippet(expr, &mut gas).unwrap();

    assert_eq!(
        ty,
        Type::app(
            Type::app(Type::con("Result", 2), Type::con("string", 0)),
            Type::con("i32", 0)
        )
    );
}

#[derive(Rex, Debug, PartialEq)]
struct Point {
    x: i32,
    y: i32,
}

#[derive(Rex, Debug, PartialEq)]
struct ErrorInfo {
    code: i32,
    message: String,
}

#[tokio::test]
async fn result_from_value_custom_types() {
    fn accept_result(_state: &(), res: Result<Point, ErrorInfo>) -> String {
        match res {
            Ok(p) => format!("Ok: Point({}, {})", p.x, p.y),
            Err(e) => format!("Err: {} (code {})", e.message, e.code),
        }
    }

    let mut engine = Engine::with_prelude(()).unwrap();
    Point::inject_rex(&mut engine).unwrap();
    ErrorInfo::inject_rex(&mut engine).unwrap();
    engine.inject_fn1("accept_result", accept_result).unwrap();

    let expr = r#"
        (
            accept_result (Ok (Point { x = 10, y = 20 })),
            accept_result (Err (ErrorInfo { code = 404, message = "not found" }))
        )
    "#;
    let tokens = Token::tokenize(expr).unwrap();
    let program = Parser::new(tokens).parse_program().unwrap();
    let mut gas = GasMeter::unlimited(GasCosts::sensible_defaults());
    let result = engine.eval(program.expr.as_ref(), &mut gas).await.unwrap();

    let heap = engine.heap();
    assert_pointer_eq!(
        heap,
        result,
        heap.alloc_tuple(vec![
            heap.alloc_string("Ok: Point(10, 20)".to_string()).unwrap(),
            heap.alloc_string("Err: not found (code 404)".to_string())
                .unwrap(),
        ])
        .unwrap(),
    );
}

#[tokio::test]
async fn result_into_value_custom_types() {
    fn return_result(_state: &(), flag: bool) -> Result<Point, ErrorInfo> {
        if flag {
            Ok(Point { x: 100, y: 200 })
        } else {
            Err(ErrorInfo {
                code: 500,
                message: "server error".to_string(),
            })
        }
    }

    let mut engine = Engine::with_prelude(()).unwrap();
    Point::inject_rex(&mut engine).unwrap();
    ErrorInfo::inject_rex(&mut engine).unwrap();
    engine.inject_fn1("return_result", return_result).unwrap();

    let expr = r#"(return_result true, return_result false)"#;
    let tokens = Token::tokenize(expr).unwrap();
    let program = Parser::new(tokens).parse_program().unwrap();
    let mut gas = GasMeter::unlimited(GasCosts::sensible_defaults());
    let result = engine.eval(program.expr.as_ref(), &mut gas).await.unwrap();

    let heap = engine.heap();

    // Verify by converting back to Rust types
    let tuple_ptrs = heap.pointer_as_tuple(&result).unwrap();
    assert_eq!(tuple_ptrs.len(), 2);
    let ok_ptr = &tuple_ptrs[0];
    let err_ptr = &tuple_ptrs[1];

    let ok_result = <Result<Point, ErrorInfo>>::from_pointer(&heap, ok_ptr).unwrap();
    let err_result = <Result<Point, ErrorInfo>>::from_pointer(&heap, err_ptr).unwrap();

    assert_eq!(ok_result, Ok(Point { x: 100, y: 200 }));
    assert_eq!(
        err_result,
        Err(ErrorInfo {
            code: 500,
            message: "server error".to_string(),
        })
    );
}
