mod common;

use std::collections::{BTreeMap, HashMap};

use rex::{
    Rex,
    ast::Symbol,
    engine::{Builder, CompileOptions, EngineError, FromRex, Value},
    parser::parse as parse_rex,
    typesystem::{BuiltinTypeId, RexType, Type},
};

/// Helper to evaluate a Rex expression and return the result handle.
async fn eval_expr(builder: Builder<()>, expr: &str) -> (Value, (), Type) {
    let program = parse_rex(expr).unwrap();
    let (value, ty) = common::run_program(builder, &program).await.unwrap();
    (value, (), ty)
}

/// Helper to infer the type of a Rex expression
async fn infer_type(builder: Builder<()>, expr: &str) -> Type {
    let compiler = builder.build_compiler();
    let parsed = parse_rex(expr).unwrap();
    let (program, _evaluator) = compiler
        .compile_program(&parsed, CompileOptions::for_module("test.main").unwrap())
        .await
        .unwrap();
    program.result_type().clone()
}

#[tokio::test]
async fn vec_from_value() {
    fn accept_vec(_state: &(), items: Vec<i32>) -> Result<String, EngineError> {
        Ok(format!("accept_vec: {:?}", items))
    }

    let mut builder = Builder::with_prelude(()).unwrap();
    common::inject_globals(&mut builder, |module| {
        module.export("accept_vec", accept_vec)
    })
    .unwrap();

    let (result, _, ty) = eval_expr(builder, r#"accept_vec [1, 2, 3]"#).await;
    assert_eq!(ty, Type::builtin(BuiltinTypeId::String));
    common::assert_handles_eq(&result, &Value::String("accept_vec: [1, 2, 3]".to_string()));
}

#[tokio::test]
async fn vec_from_value_accepts_list_literal_without_conversion() {
    fn accept_vec(_state: &(), items: Vec<i32>) -> Result<String, EngineError> {
        Ok(format!("accept_vec: {:?}", items))
    }

    let mut builder = Builder::with_prelude(()).unwrap();
    common::inject_globals(&mut builder, |module| {
        module.export("accept_vec", accept_vec)
    })
    .unwrap();

    let (result, _, ty) = eval_expr(builder, r#"accept_vec [1, 2, 3]"#).await;
    assert_eq!(ty, Type::builtin(BuiltinTypeId::String));
    common::assert_handles_eq(&result, &Value::String("accept_vec: [1, 2, 3]".to_string()));
}

#[tokio::test]
async fn vec_to_value() {
    fn return_vec(_state: &(), input: String) -> Result<Vec<i32>, EngineError> {
        Ok((0..input.len()).map(|i| i as i32).collect())
    }

    let mut builder = Builder::with_prelude(()).unwrap();
    common::inject_globals(&mut builder, |module| {
        module.export("return_vec", return_vec)
    })
    .unwrap();

    let (result, _, ty) = eval_expr(builder, r#"return_vec "hello""#).await;
    assert_eq!(ty, Type::list(Type::builtin(BuiltinTypeId::I32)));
    common::assert_handles_eq(
        &result,
        &Value::List(vec![
            Value::I32(0),
            Value::I32(1),
            Value::I32(2),
            Value::I32(3),
            Value::I32(4),
        ]),
    );
}

#[tokio::test]
async fn vec_rex_type() {
    fn return_vec(_state: &(), input: String) -> Result<Vec<i32>, EngineError> {
        Ok((0..input.len()).map(|i| i as i32).collect())
    }

    let mut builder = Builder::with_prelude(()).unwrap();
    common::inject_globals(&mut builder, |module| {
        module.export("return_vec", return_vec)
    })
    .unwrap();

    let ty = infer_type(builder, r#"return_vec "hello""#).await;
    assert_eq!(ty, Type::list(Type::builtin(BuiltinTypeId::I32)));
}

#[test]
fn string_maps_rex_type() {
    let expected = Type::dict(Type::builtin(BuiltinTypeId::I32));

    assert_eq!(<BTreeMap<String, i32> as RexType>::rex_type(), expected);
    assert_eq!(<HashMap<String, i32> as RexType>::rex_type(), expected);
}

#[tokio::test]
async fn host_vecs_pattern_match_as_lists() {
    fn return_vec(_state: &(), input: String) -> Result<Vec<i32>, EngineError> {
        Ok((0..input.len()).map(|i| i as i32).collect())
    }

    let mut builder = Builder::with_prelude(()).unwrap();
    common::inject_globals(&mut builder, |module| {
        module.export("return_vec", return_vec)
    })
    .unwrap();

    let (result, _, ty) = eval_expr(
        builder,
        r#"match (return_vec "abc") with {
            case Cons x _ -> x;
            case Empty -> -1;
        }"#,
    )
    .await;
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    common::assert_handles_eq(&result, &Value::I32(0));
}

#[tokio::test]
async fn host_vec_u8_returns_canonical_bytes_value() {
    fn return_bytes(_state: &()) -> Result<Vec<u8>, EngineError> {
        Ok(vec![3, 4, 5])
    }

    let mut builder = Builder::with_prelude(()).unwrap();
    common::inject_globals(&mut builder, |module| {
        module.export("return_bytes", return_bytes)
    })
    .unwrap();

    let (result, _heap, ty) = eval_expr(builder, "return_bytes").await;
    assert_eq!(ty, Type::list(Type::builtin(BuiltinTypeId::U8)));
    assert_eq!(result, Value::Bytes(vec![3, 4, 5]));
}

#[tokio::test]
async fn host_vec_u8_arguments_decode_binary_and_hybrid_lists() {
    fn return_bytes(_state: &()) -> Result<Vec<u8>, EngineError> {
        Ok(vec![10, 11, 12, 13])
    }

    fn accept_bytes(_state: &(), bytes: Vec<u8>) -> Result<String, EngineError> {
        Ok(format!("{bytes:?}"))
    }

    let mut builder = Builder::with_prelude(()).unwrap();
    common::inject_globals(&mut builder, |module| {
        module.export("return_bytes", return_bytes)?;
        module.export("accept_bytes", accept_bytes)
    })
    .unwrap();

    let (result, _, ty) = eval_expr(
        builder,
        r#"
        (
          accept_bytes return_bytes,
          accept_bytes (slice 1 3 return_bytes),
          accept_bytes (Cons (1 is u8) (slice 1 4 return_bytes)),
          match return_bytes with {
            case Cons head tail -> (head, length tail);
            case Empty -> (0 is u8, 0);
          }
        )
        "#,
    )
    .await;

    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::String),
            Type::builtin(BuiltinTypeId::String),
            Type::builtin(BuiltinTypeId::String),
            Type::tuple(vec![
                Type::builtin(BuiltinTypeId::U8),
                Type::builtin(BuiltinTypeId::I32),
            ]),
        ])
    );
    common::assert_handles_eq(
        &result,
        &Value::Tuple(vec![
            Value::String("[10, 11, 12, 13]".to_string()),
            Value::String("[11, 12]".to_string()),
            Value::String("[1, 11, 12, 13]".to_string()),
            Value::Tuple(vec![Value::U8(10), Value::I32(3)]),
        ]),
    );
}

#[tokio::test]
async fn option_prelude() {
    let builder = Builder::with_prelude(()).unwrap();
    let (result, _, ty) = eval_expr(
        builder,
        r#"(((Some 4) is Option i32), (None is Option i32))"#,
    )
    .await;
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::option(Type::builtin(BuiltinTypeId::I32)),
            Type::option(Type::builtin(BuiltinTypeId::I32)),
        ])
    );
    common::assert_handles_eq(
        &result,
        &Value::Tuple(vec![
            Value::Adt(Symbol::intern("Some"), vec![Value::I32(4)]),
            Value::Adt(Symbol::intern("None"), vec![]),
        ]),
    );
}

#[tokio::test]
async fn option_from_value() {
    fn accept_opt(_state: &(), opt: Option<i32>) -> Result<String, EngineError> {
        Ok(format!("accept_opt: {:?}", opt))
    }

    let mut builder = Builder::with_prelude(()).unwrap();
    common::inject_globals(&mut builder, |module| {
        module.export("accept_opt", accept_opt)
    })
    .unwrap();
    let (result, _, ty) = eval_expr(builder, r#"(accept_opt (Some 4), accept_opt None)"#).await;
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::String),
            Type::builtin(BuiltinTypeId::String)
        ])
    );
    common::assert_handles_eq(
        &result,
        &Value::Tuple(vec![
            Value::String("accept_opt: Some(4)".to_string()),
            Value::String("accept_opt: None".to_string()),
        ]),
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

    let mut builder = Builder::with_prelude(()).unwrap();
    common::inject_globals(&mut builder, |module| {
        module.export("return_opt", return_opt)
    })
    .unwrap();
    let (result, _, ty) = eval_expr(builder, r#"(return_opt "hello", return_opt "")"#).await;
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::option(Type::builtin(BuiltinTypeId::I32)),
            Type::option(Type::builtin(BuiltinTypeId::I32)),
        ])
    );
    common::assert_handles_eq(
        &result,
        &Value::Tuple(vec![
            Value::Adt(Symbol::intern("Some"), vec![Value::I32(5)]),
            Value::Adt(Symbol::intern("None"), vec![]),
        ]),
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

    let mut builder = Builder::with_prelude(()).unwrap();
    common::inject_globals(&mut builder, |module| {
        module.export("return_opt", return_opt)
    })
    .unwrap();

    let ty = infer_type(builder, r#"return_opt "hello""#).await;
    assert_eq!(
        ty,
        Type::app(
            Type::builtin(BuiltinTypeId::Option),
            Type::builtin(BuiltinTypeId::I32)
        )
    );
}

#[tokio::test]
async fn result_prelude() {
    let builder = Builder::with_prelude(()).unwrap();
    let (result, _, ty) = eval_expr(
        builder,
        r#"(((Ok 42) is Result i32 String), ((Err "error") is Result i32 String))"#,
    )
    .await;
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::result(
                Type::builtin(BuiltinTypeId::I32),
                Type::builtin(BuiltinTypeId::String)
            ),
            Type::result(
                Type::builtin(BuiltinTypeId::I32),
                Type::builtin(BuiltinTypeId::String)
            ),
        ])
    );
    common::assert_handles_eq(
        &result,
        &Value::Tuple(vec![
            Value::Adt(Symbol::intern("Ok"), vec![Value::I32(42)]),
            Value::Adt(
                Symbol::intern("Err"),
                vec![Value::String("error".to_string())],
            ),
        ]),
    );
}

#[tokio::test]
async fn result_from_value_primitives() {
    fn accept_result(_state: &(), res: Result<i32, String>) -> Result<String, EngineError> {
        Ok(format!("accept_result: {:?}", res))
    }

    let mut builder = Builder::with_prelude(()).unwrap();
    common::inject_globals(&mut builder, |module| {
        module.export("accept_result", accept_result)
    })
    .unwrap();
    let (result, _, ty) = eval_expr(
        builder,
        r#"(accept_result (Ok 42), accept_result (Err "failed"))"#,
    )
    .await;
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::String),
            Type::builtin(BuiltinTypeId::String)
        ])
    );
    common::assert_handles_eq(
        &result,
        &Value::Tuple(vec![
            Value::String("accept_result: Ok(42)".to_string()),
            Value::String("accept_result: Err(\"failed\")".to_string()),
        ]),
    );
}

#[tokio::test]
async fn result_from_value_different_primitives() {
    fn accept_result(_state: &(), res: Result<f32, i32>) -> Result<String, EngineError> {
        Ok(format!("accept_result: {:?}", res))
    }

    let mut builder = Builder::with_prelude(()).unwrap();
    common::inject_globals(&mut builder, |module| {
        module.export("accept_result", accept_result)
    })
    .unwrap();
    let (result, _, ty) = eval_expr(
        builder,
        r#"(accept_result (Ok 3.14), accept_result (Err 404))"#,
    )
    .await;
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::String),
            Type::builtin(BuiltinTypeId::String)
        ])
    );
    common::assert_handles_eq(
        &result,
        &Value::Tuple(vec![
            Value::String("accept_result: Ok(3.14)".to_string()),
            Value::String("accept_result: Err(404)".to_string()),
        ]),
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

    let mut builder = Builder::with_prelude(()).unwrap();
    common::inject_globals(&mut builder, |module| {
        module.export("return_result", return_result)
    })
    .unwrap();
    let (result, _, ty) = eval_expr(builder, r#"(return_result "hello", return_result "")"#).await;
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::result(
                Type::builtin(BuiltinTypeId::I32),
                Type::builtin(BuiltinTypeId::String)
            ),
            Type::result(
                Type::builtin(BuiltinTypeId::I32),
                Type::builtin(BuiltinTypeId::String)
            ),
        ])
    );
    common::assert_handles_eq(
        &result,
        &Value::Tuple(vec![
            Value::Adt(Symbol::intern("Ok"), vec![Value::I32(5)]),
            Value::Adt(
                Symbol::intern("Err"),
                vec![Value::String("empty string".to_string())],
            ),
        ]),
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

    let mut builder = Builder::with_prelude(()).unwrap();
    common::inject_globals(&mut builder, |module| {
        module.export("return_result", return_result)
    })
    .unwrap();

    let ty = infer_type(builder, r#"return_result "hello""#).await;
    assert_eq!(
        ty,
        Type::app(
            Type::app(
                Type::builtin(BuiltinTypeId::Result),
                Type::builtin(BuiltinTypeId::String)
            ),
            Type::builtin(BuiltinTypeId::I32)
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

    let mut builder = Builder::with_prelude(()).unwrap();
    Point::inject_rex(&mut builder).unwrap();
    ErrorInfo::inject_rex(&mut builder).unwrap();
    common::inject_globals(&mut builder, |module| {
        module.export("accept_result", accept_result)
    })
    .unwrap();

    let (result, _, ty) = eval_expr(
        builder,
        r#"(
            accept_result (Ok (Point { x = 10, y = 20 })),
            accept_result (Err (ErrorInfo { code = 404, message = "not found" }))
        )"#,
    )
    .await;
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::String),
            Type::builtin(BuiltinTypeId::String)
        ])
    );

    common::assert_handles_eq(
        &result,
        &Value::Tuple(vec![
            Value::String("Ok: Point(10, 20)".to_string()),
            Value::String("Err: not found (code 404)".to_string()),
        ]),
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

    let mut builder = Builder::with_prelude(()).unwrap();
    Point::inject_rex(&mut builder).unwrap();
    ErrorInfo::inject_rex(&mut builder).unwrap();
    common::inject_globals(&mut builder, |module| {
        module.export("return_result", return_result)
    })
    .unwrap();

    let (result, _heap, ty) =
        eval_expr(builder, r#"(return_result true, return_result false)"#).await;
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::result(Point::rex_type(), ErrorInfo::rex_type()),
            Type::result(Point::rex_type(), ErrorInfo::rex_type()),
        ])
    );

    let Value::Tuple(tuple_values) = result else {
        panic!("expected tuple");
    };
    assert_eq!(tuple_values.len(), 2);

    let ok_result = <Result<Point, ErrorInfo>>::from_rex(tuple_values[0].clone()).unwrap();
    let err_result = <Result<Point, ErrorInfo>>::from_rex(tuple_values[1].clone()).unwrap();

    assert_eq!(ok_result, Ok(Point { x: 100, y: 200 }));
    assert_eq!(
        err_result,
        Err(ErrorInfo {
            code: 500,
            message: "server error".to_string(),
        })
    );
}
