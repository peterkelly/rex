use rex::{
    Engine, EngineError, FromPointer, GasCosts, GasMeter, Parser, Pointer, Token, Type, sym,
};
use rex_engine::assert_pointer_eq;
use rex_proc_macro::Rex;
use serde_json::json;

/// Helper to evaluate a Rex expression and return the result pointer
async fn eval_expr(engine: &mut Engine<()>, expr: &str) -> Pointer {
    let tokens = Token::tokenize(expr).unwrap();
    let mut gas = GasMeter::default();
    let program = Parser::new(tokens).parse_program(&mut gas).unwrap();
    engine.eval(program.expr.as_ref(), &mut gas).await.unwrap()
}

/// Helper to infer the type of a Rex expression
fn infer_type(engine: &mut Engine<()>, expr: &str) -> Type {
    let costs = GasCosts::sensible_defaults();
    let mut gas = GasMeter::unlimited(costs);
    let (_, ty) = engine.infer_snippet(expr, &mut gas).unwrap();
    ty
}

#[tokio::test]
async fn vec_from_value() {
    fn accept_vec(_state: &(), items: Vec<i32>) -> Result<String, EngineError> {
        Ok(format!("accept_vec: {:?}", items))
    }

    let mut engine = Engine::with_prelude(()).unwrap();
    engine.export("accept_vec", accept_vec).unwrap();

    let result = eval_expr(
        &mut engine,
        r#"accept_vec (prim_array_from_list [1, 2, 3])"#,
    )
    .await;

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
    fn return_vec(_state: &(), input: String) -> Result<Vec<i32>, EngineError> {
        Ok((0..input.len()).map(|i| i as i32).collect())
    }

    let mut engine = Engine::with_prelude(()).unwrap();
    engine.export("return_vec", return_vec).unwrap();

    let result = eval_expr(&mut engine, r#"return_vec "hello""#).await;

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
    fn return_vec(_state: &(), input: String) -> Result<Vec<i32>, EngineError> {
        Ok((0..input.len()).map(|i| i as i32).collect())
    }

    let mut engine = Engine::with_prelude(()).unwrap();
    engine.export("return_vec", return_vec).unwrap();

    let ty = infer_type(&mut engine, r#"return_vec "hello""#);
    assert_eq!(ty, Type::app(Type::con("Array", 1), Type::con("i32", 0)));
}

#[tokio::test]
async fn option_prelude() {
    let mut engine = Engine::with_prelude(()).unwrap();
    let result = eval_expr(&mut engine, r#"(Some 4, None)"#).await;
    let heap = engine.heap();
    assert_pointer_eq!(
        heap,
        result,
        heap.alloc_tuple(vec![
            heap.alloc_adt(sym("Some"), vec![heap.alloc_i32(4).unwrap()])
                .unwrap(),
            heap.alloc_adt(sym("None"), vec![]).unwrap(),
        ])
        .unwrap()
    );
}

#[tokio::test]
async fn option_from_value() {
    fn accept_opt(_state: &(), opt: Option<i32>) -> Result<String, EngineError> {
        Ok(format!("accept_opt: {:?}", opt))
    }

    let mut engine = Engine::with_prelude(()).unwrap();
    engine.export("accept_opt", accept_opt).unwrap();
    let result = eval_expr(&mut engine, r#"(accept_opt (Some 4), accept_opt None)"#).await;

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
    fn return_opt(_state: &(), s: String) -> Result<Option<i32>, EngineError> {
        Ok(if s.is_empty() {
            None
        } else {
            Some(s.len() as i32)
        })
    }

    let mut engine = Engine::with_prelude(()).unwrap();
    engine.export("return_opt", return_opt).unwrap();
    let result = eval_expr(&mut engine, r#"(return_opt "hello", return_opt "")"#).await;

    let heap = engine.heap();
    assert_pointer_eq!(
        heap,
        result,
        heap.alloc_tuple(vec![
            heap.alloc_adt(sym("Some"), vec![heap.alloc_i32(5).unwrap()])
                .unwrap(),
            heap.alloc_adt(sym("None"), vec![]).unwrap(),
        ])
        .unwrap(),
    );
}

#[tokio::test]
async fn option_rex_type() {
    fn return_opt(_state: &(), s: String) -> Result<Option<i32>, EngineError> {
        Ok(if s.is_empty() {
            None
        } else {
            Some(s.len() as i32)
        })
    }

    let mut engine = Engine::with_prelude(()).unwrap();
    engine.export("return_opt", return_opt).unwrap();

    let ty = infer_type(&mut engine, r#"return_opt "hello""#);
    assert_eq!(ty, Type::app(Type::con("Option", 1), Type::con("i32", 0)));
}

#[tokio::test]
async fn result_prelude() {
    let mut engine = Engine::with_prelude(()).unwrap();
    let result = eval_expr(&mut engine, r#"(Ok 42, Err "error")"#).await;
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
    fn accept_result(_state: &(), res: Result<i32, String>) -> Result<String, EngineError> {
        Ok(format!("accept_result: {:?}", res))
    }

    let mut engine = Engine::with_prelude(()).unwrap();
    engine.export("accept_result", accept_result).unwrap();
    let result = eval_expr(
        &mut engine,
        r#"(accept_result (Ok 42), accept_result (Err "failed"))"#,
    )
    .await;

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
    fn accept_result(_state: &(), res: Result<f32, i32>) -> Result<String, EngineError> {
        Ok(format!("accept_result: {:?}", res))
    }

    let mut engine = Engine::with_prelude(()).unwrap();
    engine.export("accept_result", accept_result).unwrap();
    let result = eval_expr(
        &mut engine,
        r#"(accept_result (Ok 3.14), accept_result (Err 404))"#,
    )
    .await;

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
    fn return_result(_state: &(), s: String) -> Result<Result<i32, String>, EngineError> {
        Ok(if s.is_empty() {
            Err("empty string".to_string())
        } else {
            Ok(s.len() as i32)
        })
    }

    let mut engine = Engine::with_prelude(()).unwrap();
    engine.export("return_result", return_result).unwrap();
    let result = eval_expr(&mut engine, r#"(return_result "hello", return_result "")"#).await;

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
    fn return_result(_state: &(), s: String) -> Result<Result<i32, String>, EngineError> {
        Ok(if s.is_empty() {
            Err("empty string".to_string())
        } else {
            Ok(s.len() as i32)
        })
    }

    let mut engine = Engine::with_prelude(()).unwrap();
    engine.export("return_result", return_result).unwrap();

    let ty = infer_type(&mut engine, r#"return_result "hello""#);
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
    fn accept_result(_state: &(), res: Result<Point, ErrorInfo>) -> Result<String, EngineError> {
        Ok(match res {
            Ok(p) => format!("Ok: Point({}, {})", p.x, p.y),
            Err(e) => format!("Err: {} (code {})", e.message, e.code),
        })
    }

    let mut engine = Engine::with_prelude(()).unwrap();
    Point::inject_rex(&mut engine).unwrap();
    ErrorInfo::inject_rex(&mut engine).unwrap();
    engine.export("accept_result", accept_result).unwrap();

    let result = eval_expr(
        &mut engine,
        r#"(
            accept_result (Ok (Point { x = 10, y = 20 })),
            accept_result (Err (ErrorInfo { code = 404, message = "not found" }))
        )"#,
    )
    .await;

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
    fn return_result(_state: &(), flag: bool) -> Result<Result<Point, ErrorInfo>, EngineError> {
        Ok(if flag {
            Ok(Point { x: 100, y: 200 })
        } else {
            Err(ErrorInfo {
                code: 500,
                message: "server error".to_string(),
            })
        })
    }

    let mut engine = Engine::with_prelude(()).unwrap();
    Point::inject_rex(&mut engine).unwrap();
    ErrorInfo::inject_rex(&mut engine).unwrap();
    engine.export("return_result", return_result).unwrap();

    let result = eval_expr(&mut engine, r#"(return_result true, return_result false)"#).await;

    let heap = engine.heap();
    let tuple_ptrs = heap.pointer_as_tuple(&result).unwrap();
    assert_eq!(tuple_ptrs.len(), 2);

    let ok_result = <Result<Point, ErrorInfo>>::from_pointer(heap, &tuple_ptrs[0]).unwrap();
    let err_result = <Result<Point, ErrorInfo>>::from_pointer(heap, &tuple_ptrs[1]).unwrap();

    assert_eq!(ok_result, Ok(Point { x: 100, y: 200 }));
    assert_eq!(
        err_result,
        Err(ErrorInfo {
            code: 500,
            message: "server error".to_string(),
        })
    );
}

#[tokio::test]
async fn serde_json_value_into_pointer() {
    fn return_json(_state: &(), key: String) -> Result<serde_json::Value, EngineError> {
        Ok(json!({
            "key": key,
            "count": 42,
            "nested": {
                "array": [1, 2, 3],
                "flag": true
            }
        }))
    }

    let mut engine = Engine::with_prelude(()).unwrap();
    engine.export("return_json", return_json).unwrap();

    let result = eval_expr(&mut engine, r#"return_json "test_key""#).await;

    let heap = engine.heap();
    let (tag, args) = heap.pointer_as_adt(&result).unwrap();
    assert_eq!(tag.as_ref(), "serde_json::Value");
    assert_eq!(args.len(), 1);

    let json_string = heap.pointer_as_string(&args[0]).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_string).unwrap();
    assert_eq!(parsed["key"], "test_key");
    assert_eq!(parsed["count"], 42);
}

#[tokio::test]
async fn serde_json_value_from_pointer() {
    fn accept_json(_state: &(), value: serde_json::Value) -> Result<String, EngineError> {
        Ok(format!(
            "key={}, count={}",
            value["key"].as_str().unwrap_or(""),
            value["count"].as_i64().unwrap_or(0)
        ))
    }

    let mut engine = Engine::with_prelude(()).unwrap();
    engine.export("accept_json", accept_json).unwrap();
    engine
        .export_value("test_json", json!({"key": "manual_key", "count": 99}))
        .unwrap();

    let result = eval_expr(&mut engine, r#"accept_json test_json"#).await;

    let heap = engine.heap();
    assert_pointer_eq!(
        heap,
        result,
        heap.alloc_string("key=manual_key, count=99".to_string())
            .unwrap(),
    );
}

#[tokio::test]
async fn serde_json_value_roundtrip() {
    fn roundtrip(_state: &(), value: serde_json::Value) -> Result<serde_json::Value, EngineError> {
        Ok(value)
    }

    let mut engine = Engine::with_prelude(()).unwrap();
    engine.export("roundtrip", roundtrip).unwrap();

    let original = json!({
        "string": "hello",
        "number": 123,
        "float": 2.5,
        "bool": true,
        "null": null,
        "array": [1, 2, 3],
        "object": {"nested": "value"}
    });
    engine.export_value("test_json", original.clone()).unwrap();

    let result = eval_expr(&mut engine, r#"roundtrip test_json"#).await;

    let heap = engine.heap();
    let result_value = serde_json::Value::from_pointer(heap, &result).unwrap();
    assert_eq!(result_value, original);
}

#[tokio::test]
async fn serde_json_value_rex_type() {
    fn return_json(_state: &(), _input: String) -> Result<serde_json::Value, EngineError> {
        Ok(json!({"test": "value"}))
    }

    let mut engine = Engine::with_prelude(()).unwrap();
    engine.export("return_json", return_json).unwrap();

    let ty = infer_type(&mut engine, r#"return_json "test""#);
    assert_eq!(ty, Type::con("serde_json::Value", 0));
}

#[tokio::test]
async fn serde_json_value_primitives() {
    fn accept_primitives(
        _state: &(),
        null: serde_json::Value,
        boolean: serde_json::Value,
        number: serde_json::Value,
        string: serde_json::Value,
    ) -> Result<String, EngineError> {
        Ok(format!(
            "null={}, bool={}, num={}, str={}",
            null.is_null(),
            boolean.as_bool().unwrap(),
            number.as_i64().unwrap(),
            string.as_str().unwrap()
        ))
    }

    let mut engine = Engine::with_prelude(()).unwrap();
    engine
        .export("accept_primitives", accept_primitives)
        .unwrap();

    engine.export_value("null_val", json!(null)).unwrap();
    engine.export_value("bool_val", json!(true)).unwrap();
    engine.export_value("num_val", json!(42)).unwrap();
    engine.export_value("str_val", json!("hello")).unwrap();

    let result = eval_expr(
        &mut engine,
        r#"accept_primitives null_val bool_val num_val str_val"#,
    )
    .await;

    let heap = engine.heap();
    assert_pointer_eq!(
        heap,
        result,
        heap.alloc_string("null=true, bool=true, num=42, str=hello".to_string())
            .unwrap(),
    );
}
