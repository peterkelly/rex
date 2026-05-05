use rex::{
    Rex,
    ast::Symbol,
    engine::{Engine, EngineError, FromRex, Handle, Heap, Module, Value},
    parser::{Parser, Token},
    typesystem::{BuiltinTypeId, RexType, Type},
};
fn inject_globals(
    engine: &mut Engine<()>,
    build: impl FnOnce(&mut Module<()>) -> Result<(), EngineError>,
) {
    let mut module = Module::global();
    build(&mut module).unwrap();
    engine.inject_module(module).unwrap();
}

/// Helper to evaluate a Rex expression and return the result handle.
async fn eval_expr(engine: Engine<()>, expr: &str) -> (Handle, Heap, Type) {
    let tokens = Token::tokenize(expr).unwrap();
    let program = Parser::new(tokens).parse_program().unwrap();
    let heap = engine.heap.clone();
    let (value, ty) = engine
        .into_evaluator()
        .eval(program.expr.as_ref())
        .await
        .unwrap();
    (value, heap, ty)
}

trait HandleRef {
    fn handle_ref(&self) -> &Handle;
}

impl HandleRef for Handle {
    fn handle_ref(&self) -> &Handle {
        self
    }
}

impl HandleRef for &Handle {
    fn handle_ref(&self) -> &Handle {
        self
    }
}

macro_rules! assert_handle_eq {
    ($lhs:expr, $rhs:expr $(,)?) => {{
        let lhs: Handle = HandleRef::handle_ref(&$lhs).clone();
        let rhs: Handle = HandleRef::handle_ref(&$rhs).clone();
        assert!(
            lhs.value_eq(&rhs).unwrap(),
            "left: {}, right: {}",
            lhs.display().unwrap(),
            rhs.display().unwrap()
        );
    }};
}

/// Helper to infer the type of a Rex expression
fn infer_type(engine: &mut Engine<()>, expr: &str) -> Type {
    let (_, ty) = engine.infer_snippet(expr).unwrap();
    ty
}

#[tokio::test]
async fn vec_from_value() {
    fn accept_vec(_state: &(), items: Vec<i32>) -> Result<String, EngineError> {
        Ok(format!("accept_vec: {:?}", items))
    }

    let mut engine = Engine::with_prelude(()).unwrap();
    inject_globals(&mut engine, |module| {
        module.export("accept_vec", accept_vec)
    });

    let (result, heap, ty) =
        eval_expr(engine, r#"accept_vec (prim_array_from_list [1, 2, 3])"#).await;
    assert_eq!(ty, Type::builtin(BuiltinTypeId::String));
    assert_handle_eq!(
        result,
        heap.alloc_string("accept_vec: [1, 2, 3]".to_string())
            .unwrap(),
    );
}

#[tokio::test]
async fn vec_from_value_accepts_list_literal_without_conversion() {
    fn accept_vec(_state: &(), items: Vec<i32>) -> Result<String, EngineError> {
        Ok(format!("accept_vec: {:?}", items))
    }

    let mut engine = Engine::with_prelude(()).unwrap();
    inject_globals(&mut engine, |module| {
        module.export("accept_vec", accept_vec)
    });

    let (result, heap, ty) = eval_expr(engine, r#"accept_vec [1, 2, 3]"#).await;
    assert_eq!(ty, Type::builtin(BuiltinTypeId::String));
    assert_handle_eq!(
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
    inject_globals(&mut engine, |module| {
        module.export("return_vec", return_vec)
    });

    let (result, heap, ty) = eval_expr(engine, r#"return_vec "hello""#).await;
    assert_eq!(ty, Type::array(Type::builtin(BuiltinTypeId::I32)));
    assert_handle_eq!(
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
    inject_globals(&mut engine, |module| {
        module.export("return_vec", return_vec)
    });

    let ty = infer_type(&mut engine, r#"return_vec "hello""#);
    assert_eq!(
        ty,
        Type::app(
            Type::builtin(BuiltinTypeId::Array),
            Type::builtin(BuiltinTypeId::I32)
        )
    );
}

#[tokio::test]
async fn to_list_allows_pattern_matching_host_arrays() {
    fn return_vec(_state: &(), input: String) -> Result<Vec<i32>, EngineError> {
        Ok((0..input.len()).map(|i| i as i32).collect())
    }

    let mut engine = Engine::with_prelude(()).unwrap();
    inject_globals(&mut engine, |module| {
        module.export("return_vec", return_vec)
    });

    let (result, heap, ty) = eval_expr(
        engine,
        r#"match (to_list (return_vec "abc")) with {
            when Cons x _ -> x;
            when Empty -> -1;
        }"#,
    )
    .await;
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_handle_eq!(result, heap.alloc_i32(0).unwrap());
}

#[tokio::test]
async fn option_prelude() {
    let engine = Engine::with_prelude(()).unwrap();
    let (result, heap, ty) = eval_expr(
        engine,
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
    assert_handle_eq!(
        result,
        heap.alloc_tuple(vec![
            heap.alloc_adt(Symbol::intern("Some"), vec![heap.alloc_i32(4).unwrap()])
                .unwrap(),
            heap.alloc_adt(Symbol::intern("None"), vec![]).unwrap(),
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
    inject_globals(&mut engine, |module| {
        module.export("accept_opt", accept_opt)
    });
    let (result, heap, ty) = eval_expr(engine, r#"(accept_opt (Some 4), accept_opt None)"#).await;
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::String),
            Type::builtin(BuiltinTypeId::String)
        ])
    );
    assert_handle_eq!(
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
    inject_globals(&mut engine, |module| {
        module.export("return_opt", return_opt)
    });
    let (result, heap, ty) = eval_expr(engine, r#"(return_opt "hello", return_opt "")"#).await;
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::option(Type::builtin(BuiltinTypeId::I32)),
            Type::option(Type::builtin(BuiltinTypeId::I32)),
        ])
    );
    assert_handle_eq!(
        result,
        heap.alloc_tuple(vec![
            heap.alloc_adt(Symbol::intern("Some"), vec![heap.alloc_i32(5).unwrap()])
                .unwrap(),
            heap.alloc_adt(Symbol::intern("None"), vec![]).unwrap(),
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
    inject_globals(&mut engine, |module| {
        module.export("return_opt", return_opt)
    });

    let ty = infer_type(&mut engine, r#"return_opt "hello""#);
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
    let engine = Engine::with_prelude(()).unwrap();
    let (result, heap, ty) = eval_expr(
        engine,
        r#"(((Ok 42) is Result i32 string), ((Err "error") is Result i32 string))"#,
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
    assert_handle_eq!(
        result,
        heap.alloc_tuple(vec![
            heap.alloc_adt(Symbol::intern("Ok"), vec![heap.alloc_i32(42).unwrap()])
                .unwrap(),
            heap.alloc_adt(
                Symbol::intern("Err"),
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
    inject_globals(&mut engine, |module| {
        module.export("accept_result", accept_result)
    });
    let (result, heap, ty) = eval_expr(
        engine,
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
    assert_handle_eq!(
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
    inject_globals(&mut engine, |module| {
        module.export("accept_result", accept_result)
    });
    let (result, heap, ty) = eval_expr(
        engine,
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
    assert_handle_eq!(
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
    inject_globals(&mut engine, |module| {
        module.export("return_result", return_result)
    });
    let (result, heap, ty) =
        eval_expr(engine, r#"(return_result "hello", return_result "")"#).await;
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
    assert_handle_eq!(
        result,
        heap.alloc_tuple(vec![
            heap.alloc_adt(Symbol::intern("Ok"), vec![heap.alloc_i32(5).unwrap()])
                .unwrap(),
            heap.alloc_adt(
                Symbol::intern("Err"),
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
    inject_globals(&mut engine, |module| {
        module.export("return_result", return_result)
    });

    let ty = infer_type(&mut engine, r#"return_result "hello""#);
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

    let mut engine = Engine::with_prelude(()).unwrap();
    Point::inject_rex(&mut engine).unwrap();
    ErrorInfo::inject_rex(&mut engine).unwrap();
    inject_globals(&mut engine, |module| {
        module.export("accept_result", accept_result)
    });

    let (result, heap, ty) = eval_expr(
        engine,
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

    assert_handle_eq!(
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
    inject_globals(&mut engine, |module| {
        module.export("return_result", return_result)
    });

    let (result, _heap, ty) =
        eval_expr(engine, r#"(return_result true, return_result false)"#).await;
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::result(Point::rex_type(), ErrorInfo::rex_type()),
            Type::result(Point::rex_type(), ErrorInfo::rex_type()),
        ])
    );

    let Value::Tuple(tuple_values) = result.value().unwrap() else {
        panic!("expected tuple");
    };
    assert_eq!(tuple_values.len(), 2);

    let ok_result = <Result<Point, ErrorInfo>>::from_rex(&tuple_values[0]).unwrap();
    let err_result = <Result<Point, ErrorInfo>>::from_rex(&tuple_values[1]).unwrap();

    assert_eq!(ok_result, Ok(Point { x: 100, y: 200 }));
    assert_eq!(
        err_result,
        Err(ErrorInfo {
            code: 500,
            message: "server error".to_string(),
        })
    );
}
