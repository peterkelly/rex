use futures::FutureExt;
use rexlang_ast::expr::{sym, sym_eq};
use rexlang_engine::{Engine, EngineError, EvaluatorRef, Value, assert_pointer_eq};
use rexlang_lexer::Token;
use rexlang_parser::Parser;
use rexlang_typesystem::{BuiltinTypeId, Scheme, Type, TypeError};
use rexlang_util::{GasCosts, GasMeter};
use std::sync::Arc;

fn parse(code: &str) -> Arc<rexlang_ast::expr::Expr> {
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    parser.parse_program(&mut GasMeter::default()).unwrap().expr
}

fn parse_program(code: &str) -> rexlang_ast::expr::Program {
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    parser.parse_program(&mut GasMeter::default()).unwrap()
}

fn strip_span(mut err: TypeError) -> TypeError {
    while let TypeError::Spanned { error, .. } = err {
        err = *error;
    }
    err
}

fn engine_with_arith() -> Engine {
    Engine::with_prelude(()).unwrap()
}

fn unlimited_gas() -> GasMeter {
    GasMeter::default()
}

async fn eval_expr(
    engine: &mut Engine,
    expr: &rexlang_ast::expr::Expr,
) -> Result<rexlang_engine::Pointer, EngineError> {
    let mut gas = unlimited_gas();
    engine
        .evaluator()
        .eval(expr, &mut gas)
        .await
        .map_err(|err| err.into_engine_error())
        .map(|(value, _)| value)
}

macro_rules! pval {
    ($engine:expr, $ptr:expr) => {
        $engine
            .heap
            .get(&$ptr)
            .map(|value| value.as_ref().clone())
            .unwrap()
    };
}

macro_rules! pvals {
    ($engine:expr, $vals:expr) => {
        $vals
            .iter()
            .map(|value| {
                $engine
                    .heap
                    .get(&value)
                    .map(|value| value.as_ref().clone())
                    .unwrap()
            })
            .collect::<Vec<_>>()
    };
}

fn list_values(engine: &Engine, value: &Value) -> Vec<rexlang_engine::Pointer> {
    let mut out = Vec::new();
    let mut cur = value.clone();
    loop {
        match &cur {
            Value::Adt(tag, args) if sym_eq(tag, "Empty") && args.is_empty() => return out,
            Value::Adt(tag, args) if sym_eq(tag, "Cons") && args.len() == 2 => {
                out.push(args[0]);
                cur = engine
                    .heap
                    .get(&args[1])
                    .map(|value| value.as_ref().clone())
                    .unwrap();
            }
            _ => panic!("expected list value"),
        }
    }
}

#[tokio::test]
async fn eval_let_lambda() {
    let expr = parse(
        r#"
        let
            id = \x -> x
        in
            id (id 1, id 2)
        "#,
    );
    let mut engine = Engine::with_prelude(()).unwrap();
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    let value = pval!(engine, value);
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(engine, xs);
            assert_eq!(xs.len(), 2);
            assert_pointer_eq!(
                &engine.heap,
                engine.heap.alloc_value(xs[0].clone()).unwrap(),
                engine.heap.alloc_i32(1).unwrap()
            );
            assert_pointer_eq!(
                &engine.heap,
                engine.heap.alloc_value(xs[1].clone()).unwrap(),
                engine.heap.alloc_i32(2).unwrap()
            );
        }
        _ => panic!("expected tuple"),
    }
}

#[tokio::test]
async fn eval_native_injection() {
    let expr = parse("inc 1");
    let mut engine = Engine::with_prelude(()).unwrap();
    engine
        .export_async("inc", |_: &(), x: i32| async move { Ok(x + 1) })
        .unwrap();

    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    assert_pointer_eq!(&engine.heap, value, engine.heap.alloc_i32(2).unwrap());
}

#[tokio::test]
async fn eval_sync_native_injection_supports_arities_0_to_8() {
    let expr = parse(
        r#"
        (
            f0,
            f1 1,
            f2 1 2,
            f3 1 2 3,
            f4 1 2 3 4,
            f5 1 2 3 4 5,
            f6 1 2 3 4 5 6,
            f7 1 2 3 4 5 6 7,
            f8 1 2 3 4 5 6 7 8
        )
        "#,
    );
    let mut engine = Engine::with_prelude(()).unwrap();
    engine.export("f0", |_: &()| Ok(0i32)).unwrap();
    engine.export("f1", |_: &(), a: i32| Ok(a)).unwrap();
    engine
        .export("f2", |_: &(), a: i32, b: i32| Ok(a + b))
        .unwrap();
    engine
        .export("f3", |_: &(), a: i32, b: i32, c: i32| Ok(a + b + c))
        .unwrap();
    engine
        .export("f4", |_: &(), a: i32, b: i32, c: i32, d: i32| {
            Ok(a + b + c + d)
        })
        .unwrap();
    engine
        .export("f5", |_: &(), a: i32, b: i32, c: i32, d: i32, e: i32| {
            Ok(a + b + c + d + e)
        })
        .unwrap();
    engine
        .export(
            "f6",
            |_: &(), a: i32, b: i32, c: i32, d: i32, e: i32, g: i32| Ok(a + b + c + d + e + g),
        )
        .unwrap();
    engine
        .export(
            "f7",
            |_: &(), a: i32, b: i32, c: i32, d: i32, e: i32, g: i32, h: i32| {
                Ok(a + b + c + d + e + g + h)
            },
        )
        .unwrap();
    engine
        .export(
            "f8",
            |_: &(), a: i32, b: i32, c: i32, d: i32, e: i32, g: i32, h: i32, i: i32| {
                Ok(a + b + c + d + e + g + h + i)
            },
        )
        .unwrap();

    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    let value = pval!(engine, value);
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(engine, xs);
            let expected = [0, 1, 3, 6, 10, 15, 21, 28, 36];
            assert_eq!(xs.len(), expected.len());
            for (idx, expected) in expected.iter().enumerate() {
                match &xs[idx] {
                    Value::I32(v) => assert_eq!(v, expected),
                    _ => panic!("expected i32 at index {idx}"),
                }
            }
        }
        _ => panic!("expected tuple"),
    }
}

#[tokio::test]
async fn runtime_env_validates_compiled_program_requirements_before_eval() {
    let mut gas = unlimited_gas();
    let mut compile_engine = Engine::with_prelude(()).unwrap();
    compile_engine
        .export_async("inc", |_: &(), x: i32| async move { Ok(x + 1) })
        .unwrap();

    let mut compiler = compile_engine.compiler();
    let program = compiler.compile_snippet("inc 1", &mut gas).unwrap();

    assert_eq!(program.externs().natives, vec![sym("inc")]);
    assert_ne!(program.link_fingerprint(), 0);

    let runtime = Engine::with_prelude(()).unwrap().runtime_env();
    let compatibility = runtime.compatibility_with(&program);
    assert_eq!(compatibility.missing_natives, vec![sym("inc")]);
    assert!(!compatibility.is_compatible());
    assert_ne!(runtime.fingerprint(), 0);

    let err = runtime.validate(&program).unwrap_err().into_engine_error();
    assert!(matches!(
        err,
        EngineError::Link {
            missing_natives,
            ..
        } if missing_natives == vec![sym("inc")]
    ));

    let mut evaluator = rexlang_engine::Evaluator::new(runtime);
    let err = evaluator
        .run(&program, &mut gas)
        .await
        .unwrap_err()
        .into_engine_error();
    assert!(matches!(
        err,
        EngineError::Link {
            missing_natives,
            ..
        } if missing_natives == vec![sym("inc")]
    ));
}

#[tokio::test]
async fn compiled_program_captures_rex_declarations_in_env_snapshot() {
    let mut gas = unlimited_gas();
    let compile_engine = Engine::with_prelude(()).unwrap();

    let mut compiler = compile_engine.compiler();
    let program = compiler
        .compile_snippet(
            r#"
            let answer = 41 in
                answer
            "#,
            &mut gas,
        )
        .unwrap();

    assert!(program.externs().is_empty(), "{:?}", program.externs());

    let runtime_engine = Engine::with_prelude(()).unwrap();
    let runtime = runtime_engine.runtime_env();
    assert!(runtime.compatibility_with(&program).is_compatible());
    runtime.validate(&program).unwrap();

    let mut evaluator = rexlang_engine::Evaluator::new(runtime);
    let value = evaluator.run(&program, &mut gas).await.unwrap();
    assert_pointer_eq!(
        &runtime_engine.heap,
        value,
        runtime_engine.heap.alloc_i32(41).unwrap()
    );
}

#[tokio::test]
async fn export_value_is_runtime_linked_like_other_host_exports() {
    let mut gas = unlimited_gas();
    let mut compile_engine = Engine::with_prelude(()).unwrap();
    compile_engine.export_value("answer", 41i32).unwrap();

    let mut compiler = compile_engine.compiler();
    let program = compiler.compile_snippet("answer + 1", &mut gas).unwrap();

    assert_eq!(program.externs().natives, vec![sym("answer")]);
    assert_eq!(program.externs().class_methods, vec![sym("+")]);

    let runtime_engine = Engine::with_prelude(()).unwrap();
    let runtime = runtime_engine.runtime_env();
    let compatibility = runtime.compatibility_with(&program);
    assert_eq!(compatibility.missing_natives, vec![sym("answer")]);
    assert!(!compatibility.is_compatible());

    let err = runtime.validate(&program).unwrap_err().into_engine_error();
    assert!(matches!(
        err,
        EngineError::Link {
            missing_natives,
            ..
        } if missing_natives == vec![sym("answer")]
    ));
}

#[tokio::test]
async fn runtime_env_reports_missing_class_method_bindings_before_eval() {
    let mut gas = unlimited_gas();
    let compile_engine = Engine::with_prelude(()).unwrap();
    let mut compiler = compile_engine.compiler();
    let program = compiler
        .compile_snippet(
            r#"
            class Pick a where
                pick : a -> a

            instance Pick i32 where
                pick = \x -> x

            pick 1
            "#,
            &mut gas,
        )
        .unwrap();

    assert_eq!(program.externs().class_methods, vec![sym("pick")]);

    let runtime_engine = Engine::with_prelude(()).unwrap();
    let runtime = runtime_engine.runtime_env();
    let compatibility = runtime.compatibility_with(&program);
    assert_eq!(compatibility.missing_class_methods, vec![sym("pick")]);
    assert!(!compatibility.is_compatible());

    let err = runtime.validate(&program).unwrap_err().into_engine_error();
    assert!(matches!(
        err,
        EngineError::Link {
            missing_class_methods,
            ..
        } if missing_class_methods == vec![sym("pick")]
    ));
}

#[tokio::test]
async fn eval_async_native_injection_supports_arities_0_to_8() {
    let expr = parse(
        r#"
        (
            af0,
            af1 1,
            af2 1 2,
            af3 1 2 3,
            af4 1 2 3 4,
            af5 1 2 3 4 5,
            af6 1 2 3 4 5 6,
            af7 1 2 3 4 5 6 7,
            af8 1 2 3 4 5 6 7 8
        )
        "#,
    );
    let mut engine = Engine::with_prelude(()).unwrap();
    engine
        .export_async("af0", |_: &()| async { Ok(0i32) })
        .unwrap();
    engine
        .export_async("af1", |_: &(), a: i32| async move { Ok(a) })
        .unwrap();
    engine
        .export_async("af2", |_: &(), a: i32, b: i32| async move { Ok(a + b) })
        .unwrap();
    engine
        .export_async("af3", |_: &(), a: i32, b: i32, c: i32| async move {
            Ok(a + b + c)
        })
        .unwrap();
    engine
        .export_async("af4", |_: &(), a: i32, b: i32, c: i32, d: i32| async move {
            Ok(a + b + c + d)
        })
        .unwrap();
    engine
        .export_async(
            "af5",
            |_: &(), a: i32, b: i32, c: i32, d: i32, e: i32| async move { Ok(a + b + c + d + e) },
        )
        .unwrap();
    engine
        .export_async(
            "af6",
            |_: &(), a: i32, b: i32, c: i32, d: i32, e: i32, g: i32| async move {
                Ok(a + b + c + d + e + g)
            },
        )
        .unwrap();
    engine
        .export_async(
            "af7",
            |_: &(), a: i32, b: i32, c: i32, d: i32, e: i32, g: i32, h: i32| async move {
                Ok(a + b + c + d + e + g + h)
            },
        )
        .unwrap();
    engine
        .export_async(
            "af8",
            |_: &(), a: i32, b: i32, c: i32, d: i32, e: i32, g: i32, h: i32, i: i32| async move {
                Ok(a + b + c + d + e + g + h + i)
            },
        )
        .unwrap();

    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    let value = pval!(engine, value);
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(engine, xs);
            let expected = [0, 1, 3, 6, 10, 15, 21, 28, 36];
            assert_eq!(xs.len(), expected.len());
            for (idx, expected) in expected.iter().enumerate() {
                match &xs[idx] {
                    Value::I32(v) => assert_eq!(v, expected),
                    _ => panic!("expected i32 at index {idx}"),
                }
            }
        }
        _ => panic!("expected tuple"),
    }
}

#[tokio::test]
async fn eval_with_gas_rejects_out_of_budget() {
    let expr = parse("1");
    let engine = Engine::with_prelude(()).unwrap();
    let mut gas = GasMeter::new(
        Some(0),
        GasCosts {
            eval_node: 1,
            ..GasCosts::sensible_defaults()
        },
    );
    let err = match engine.evaluator().eval(expr.as_ref(), &mut gas).await {
        Ok(_) => panic!("expected out of gas"),
        Err(e) => e,
    };
    assert!(matches!(err.as_engine_error(), EngineError::OutOfGas(..)));
}

#[test]
fn eval_deep_list_does_not_overflow() {
    // Regression test: deeply nested terms (right-nested arguments) can overflow the default
    // Rust stack during typechecking/evaluation unless callers opt into a larger stack.
    const N: usize = 2_000;
    let mut code = String::new();
    code.push_str("let xs = ");
    for _ in 0..N {
        code.push_str("Cons 0 (");
    }
    code.push_str("Empty");
    for _ in 0..N {
        code.push(')');
    }
    code.push_str(" in xs");

    let handle = std::thread::Builder::new()
        .name("eval_deep_list_large_stack".into())
        .stack_size(128 * 1024 * 1024)
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let tokens = Token::tokenize(&code).unwrap();
                let mut parser = Parser::new(tokens);
                let program = parser.parse_program(&mut GasMeter::default()).unwrap();
                let expr = program.expr;
                let mut engine = Engine::with_prelude(()).unwrap();
                let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
                let xs = engine
                    .heap
                    .get(&value)
                    .map(|value| list_values(&engine, value.as_ref()))
                    .unwrap();
                assert_eq!(xs.len(), N);
                let expected = engine.heap.alloc_i32(0).unwrap();
                assert_pointer_eq!(
                    &engine.heap,
                    xs.first().expect("list should be non-empty"),
                    expected
                );
                assert_pointer_eq!(
                    &engine.heap,
                    xs.last().expect("list should be non-empty"),
                    expected
                );
            });
        })
        .unwrap();

    handle.join().unwrap();
}

#[tokio::test]
async fn eval_type_annotation_let() {
    let expr = parse("let x: i32 = 42 in x");
    let mut engine = engine_with_arith();
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    assert_pointer_eq!(&engine.heap, value, engine.heap.alloc_i32(42).unwrap());
}

#[tokio::test]
async fn eval_type_annotation_is() {
    let expr = parse("\"hi\" is str");
    let mut engine = engine_with_arith();
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    assert_pointer_eq!(
        &engine.heap,
        value,
        engine.heap.alloc_string("hi".into()).unwrap()
    );
}

#[tokio::test]
async fn eval_type_annotation_lambda_param() {
    let expr = parse("let f = \\ (a : f32) -> a in f 1.5");
    let mut engine = engine_with_arith();
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    let value = pval!(engine, value);
    assert!(matches!(value, Value::F32(v) if (v - 1.5).abs() < f32::EPSILON));
}

#[tokio::test]
async fn eval_record_update_single_variant_adt() {
    let program = parse_program(
        r#"
        type Foo = Bar { x: i32, y: i32, z: i32 }
        let
          foo: Foo = Bar { x = 1, y = 2, z = 3 },
          bar: Foo = { foo with { x = 6 } }
        in
          bar.x
        "#,
    );
    let mut engine = engine_with_arith();
    engine.inject_decls(&program.decls).unwrap();
    let value = eval_expr(&mut engine, program.expr.as_ref()).await.unwrap();
    assert_pointer_eq!(&engine.heap, value, engine.heap.alloc_i32(6).unwrap());
}

#[tokio::test]
async fn eval_record_update_refined_by_match() {
    let program = parse_program(
        r#"
        type Foo = Bar { x: i32 } | Baz { x: i32 }
        let
          foo: Foo = Bar { x = 1 }
        in
          match foo
            when Bar {x} -> (match { foo with { x = x + 1 } } when Bar {x} -> x when Baz {x} -> x)
            when Baz {x} -> (match { foo with { x = x + 2 } } when Bar {x} -> x when Baz {x} -> x)
        "#,
    );
    let mut engine = engine_with_arith();
    engine.inject_decls(&program.decls).unwrap();
    let value = eval_expr(&mut engine, program.expr.as_ref()).await.unwrap();
    assert_pointer_eq!(&engine.heap, value, engine.heap.alloc_i32(2).unwrap());
}

#[tokio::test]
async fn eval_record_update_plain_record_type() {
    let program = parse_program(
        r#"
        let
          f = \ (r : { x: i32, y: i32 }) -> { r with { y = 9 } }
        in
          match (f { x = 1, y = 2 }) when {y} -> y
        "#,
    );
    let mut engine = engine_with_arith();
    engine.inject_decls(&program.decls).unwrap();
    let value = eval_expr(&mut engine, program.expr.as_ref()).await.unwrap();
    assert_pointer_eq!(&engine.heap, value, engine.heap.alloc_i32(9).unwrap());
}

#[tokio::test]
async fn eval_type_annotation_mismatch() {
    let expr = parse("let x: i32 = 3.14 in x");
    let mut engine = engine_with_arith();
    match eval_expr(&mut engine, expr.as_ref()).await {
        Err(EngineError::Type(err)) => {
            let err = strip_span(err);
            assert!(matches!(err, TypeError::Unification(_, _)));
        }
        Err(other) => panic!("expected type error, got {other:?}"),
        Ok(_) => panic!("expected type error, got Ok"),
    }
}

#[tokio::test]
async fn eval_typed_hole_reports_type_error_not_runtime_error() {
    let expr = parse("let y : i32 = ? in y");
    let mut engine = engine_with_arith();
    match eval_expr(&mut engine, expr.as_ref()).await {
        Err(EngineError::Type(err)) => {
            let err = strip_span(err);
            match err {
                TypeError::UnsupportedExpr(msg) => {
                    assert!(
                        msg.contains("typed hole `?` must be filled before evaluation"),
                        "msg={msg}"
                    );
                }
                other => panic!("expected hole type error, got {other:?}"),
            }
        }
        Err(other) => panic!("expected type error, got {other:?}"),
        Ok(_) => panic!("expected type error, got Ok"),
    }
}

#[tokio::test]
async fn eval_sync_native_injection() {
    let mut engine = Engine::new(());
    engine.export("zero", |_: &()| Ok(0u32)).unwrap();
    engine
        .export("(+)", |_: &(), x: u32, y: u32| Ok(x + y))
        .unwrap();
    engine.export_value("one", 1u32).unwrap();

    let expr = parse("one + one");
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    assert_pointer_eq!(&engine.heap, value, engine.heap.alloc_u32(2).unwrap());

    let expr = parse("zero");
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    assert_pointer_eq!(&engine.heap, value, engine.heap.alloc_u32(0).unwrap());
}

#[tokio::test]
async fn eval_export_err_is_evaluation_failure() {
    let mut engine = Engine::new(());
    engine
        .export("fail", |_: &()| {
            Err::<i32, _>(EngineError::Custom("boom".into()))
        })
        .unwrap();

    let expr = parse("fail");
    match eval_expr(&mut engine, expr.as_ref()).await {
        Err(EngineError::Custom(msg)) => assert_eq!(msg, "boom"),
        Err(other) => panic!("expected custom error, got {other:?}"),
        Ok(_) => panic!("expected evaluation failure"),
    }
}

#[test]
fn engine_export_native_rejects_invalid_arity_scheme_pair() {
    let mut engine = Engine::new(());
    let unary_scheme = Scheme::new(
        vec![],
        vec![],
        Type::fun(
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32),
        ),
    );

    let err = engine
        .export_native(
            "bad",
            unary_scheme,
            2,
            |_engine: EvaluatorRef<'_, ()>, _: &Type, _args| {
                Err(rexlang_engine::EngineError::Internal("unused".into()))
            },
        )
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("does not accept 2 argument(s)"),
        "unexpected error: {msg}"
    );
}

#[test]
fn engine_export_native_async_rejects_invalid_arity_scheme_pair() {
    let mut engine = Engine::new(());
    let unary_scheme = Scheme::new(
        vec![],
        vec![],
        Type::fun(
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32),
        ),
    );

    let err = engine
        .export_native_async(
            "bad_async",
            unary_scheme,
            2,
            |_engine: EvaluatorRef<'_, ()>, _: Type, _args| {
                async { Err(rexlang_engine::EngineError::Internal("unused".into())) }.boxed()
            },
        )
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("does not accept 2 argument(s)"),
        "unexpected error: {msg}"
    );
}

#[tokio::test]
async fn eval_match_list() {
    let mut engine = engine_with_arith();

    let expr = parse(
        r#"
        match [1, 2, 3]
            when [] -> 0
            when x::xs -> x
        "#,
    );
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    assert_pointer_eq!(&engine.heap, value, engine.heap.alloc_i32(1).unwrap());
}

#[tokio::test]
async fn eval_cons_constructor_form_for_lists() {
    let mut engine = engine_with_arith();

    let expr = parse(
        r#"
        let
            from_sugar = 1::2::[],
            from_ctor = Cons 1 (Cons 2 Empty)
        in
            (from_sugar, from_ctor, match from_ctor when Cons h _t -> h when [] -> 0)
        "#,
    );
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    let value = pval!(engine, value);
    let Value::Tuple(xs) = value else {
        panic!("expected tuple result");
    };
    assert_eq!(xs.len(), 3);

    let sugar = engine.heap.get(&xs[0]).unwrap();
    let ctor = engine.heap.get(&xs[1]).unwrap();
    let sugar_items = list_values(&engine, sugar.as_ref());
    let ctor_items = list_values(&engine, ctor.as_ref());
    assert_eq!(sugar_items.len(), 2);
    assert_eq!(ctor_items.len(), 2);
    assert_pointer_eq!(
        &engine.heap,
        sugar_items[0],
        engine.heap.alloc_i32(1).unwrap()
    );
    assert_pointer_eq!(
        &engine.heap,
        sugar_items[1],
        engine.heap.alloc_i32(2).unwrap()
    );
    assert_pointer_eq!(
        &engine.heap,
        ctor_items[0],
        engine.heap.alloc_i32(1).unwrap()
    );
    assert_pointer_eq!(
        &engine.heap,
        ctor_items[1],
        engine.heap.alloc_i32(2).unwrap()
    );
    assert_pointer_eq!(&engine.heap, xs[2], engine.heap.alloc_i32(1).unwrap());
}

#[tokio::test]
async fn eval_simple_addition() {
    let expr = parse("420 + 69");
    let mut engine = engine_with_arith();
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    assert_pointer_eq!(&engine.heap, value, engine.heap.alloc_i32(489).unwrap());
}

#[tokio::test]
async fn eval_simple_mod() {
    let expr = parse("10 % 3");
    let mut engine = engine_with_arith();
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    assert_pointer_eq!(&engine.heap, value, engine.heap.alloc_i32(1).unwrap());
}

#[tokio::test]
async fn eval_get_list_and_tuple() {
    let mut engine = engine_with_arith();

    let expr = parse("get 1 [1, 2, 3]");
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    assert_pointer_eq!(&engine.heap, value, engine.heap.alloc_i32(2).unwrap());

    let expr = parse("(1, 2, 3).2");
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    assert_pointer_eq!(&engine.heap, value, engine.heap.alloc_i32(3).unwrap());
}

#[tokio::test]
async fn eval_simple_multiplication_float() {
    let expr = parse("420.0 * 6.9");
    let mut engine = engine_with_arith();
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    let value = pval!(engine, value);
    match value {
        Value::F32(v) => assert!((v - 2898.0).abs() < 1e-3),
        _ => panic!("expected f32 result"),
    }
}

#[tokio::test]
async fn eval_let_id_nested() {
    let expr = parse(
        r#"
        let
            id = \x -> x
        in
            id (id 420 + id 69)
        "#,
    );
    let mut engine = engine_with_arith();
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    assert_pointer_eq!(&engine.heap, value, engine.heap.alloc_i32(489).unwrap());
}

#[tokio::test]
async fn eval_higher_order_add() {
    let expr = parse(
        r#"
        let
            add = \x -> \y -> x + y
        in
            add 40 2
        "#,
    );
    let mut engine = engine_with_arith();
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    assert_pointer_eq!(&engine.heap, value, engine.heap.alloc_i32(42).unwrap());
}

#[tokio::test]
async fn eval_match_dict_and_tuple() {
    let expr = parse(
        r#"
        let
            inc = \x -> x + 1
        in
            match { foo = 1, bar = 2 }
                when {foo, bar} -> (inc foo, inc bar)
        "#,
    );
    let mut engine = engine_with_arith();
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    let value = pval!(engine, value);
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(engine, xs);
            assert_eq!(xs.len(), 2);
            assert_pointer_eq!(
                &engine.heap,
                engine.heap.alloc_value(xs[0].clone()).unwrap(),
                engine.heap.alloc_i32(2).unwrap()
            );
            assert_pointer_eq!(
                &engine.heap,
                engine.heap.alloc_value(xs[1].clone()).unwrap(),
                engine.heap.alloc_i32(3).unwrap()
            );
        }
        _ => panic!("expected tuple result"),
    }
}

#[tokio::test]
async fn eval_match_missing_arm_errors() {
    let expr = parse("match (Err 1) when Ok x -> x");
    let mut engine = Engine::with_prelude(()).unwrap();
    let result = eval_expr(&mut engine, expr.as_ref()).await;
    match result {
        Err(EngineError::Type(err)) => {
            let err = strip_span(err);
            assert!(matches!(err, TypeError::NonExhaustiveMatch { .. }));
        }
        _ => panic!("expected non-exhaustive match type error"),
    }
}

#[tokio::test]
async fn eval_match_invalid_pattern_type_error() {
    let expr = parse("match (Ok 1) when [] -> 0 when x::xs -> 1");
    let mut engine = Engine::with_prelude(()).unwrap();
    let result = eval_expr(&mut engine, expr.as_ref()).await;
    match result {
        Err(EngineError::Type(err)) => {
            let err = strip_span(err);
            assert!(matches!(err, TypeError::Unification(_, _)));
        }
        _ => panic!("expected unification type error"),
    }
}

#[tokio::test]
async fn eval_nested_match_list_sum() {
    let expr = parse(
        r#"
        match [1, 2, 3]
            when x::xs ->
                (match xs
                    when [] -> x
                    when y::ys -> x + y)
            when [] -> 0
        "#,
    );
    let mut engine = engine_with_arith();
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    assert_pointer_eq!(&engine.heap, value, engine.heap.alloc_i32(3).unwrap());
}

#[tokio::test]
async fn eval_safe_div_pipeline() {
    let expr = parse(
        r#"
        let
            id = \x -> x,
            safeDiv = \a b -> if b == 0.0 then None else Some (a / b),
            noneToZero = \x -> match x when None -> zero when Some y -> y,
            someToOne = \x -> match x when Some _ -> one when None -> zero
        in
            (
                someToOne ((id safeDiv) (id 420.0) (id 6.9)),
                someToOne (safeDiv 420.0 6.9),
                noneToZero (safeDiv 420.0 0.0)
            )
        "#,
    );
    let mut engine = Engine::with_prelude(()).unwrap();
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    let value = pval!(engine, value);
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(engine, xs);
            assert_eq!(xs.len(), 3);
            match xs[0] {
                Value::F32(v) => assert!((v - 1.0).abs() < 1e-3),
                _ => panic!("expected f32 one"),
            }
            match xs[1] {
                Value::F32(v) => assert!((v - 1.0).abs() < 1e-3),
                _ => panic!("expected f32 one"),
            }
            match xs[2] {
                Value::F32(v) => assert!((v - 0.0).abs() < 1e-3),
                _ => panic!("expected f32 zero"),
            }
        }
        _ => panic!("expected tuple result"),
    }
}

#[tokio::test]
async fn eval_user_adt_declaration() {
    let program = parse_program(
        r#"
        type Boxed a = Box a
        let
            value = Box 42
        in
            match value
                when Box x -> x
        "#,
    );
    let mut engine = Engine::with_prelude(()).unwrap();
    for decl in &program.decls {
        if let rexlang_ast::expr::Decl::Type(ty) = decl {
            engine.inject_type_decl(ty).unwrap();
        }
    }
    let value = eval_expr(&mut engine, program.expr.as_ref()).await.unwrap();
    assert_pointer_eq!(&engine.heap, value, engine.heap.alloc_i32(42).unwrap());
}

#[tokio::test]
async fn eval_fn_decl_simple() {
    let program = parse_program(
        r#"
        fn add (x: i32, y: i32) -> i32 = x + y
        add 1 2
        "#,
    );
    let mut engine = Engine::with_prelude(()).unwrap();
    for decl in &program.decls {
        if let rexlang_ast::expr::Decl::Type(ty) = decl {
            engine.inject_type_decl(ty).unwrap();
        }
    }
    let expr = program.expr_with_fns();
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    assert_pointer_eq!(&engine.heap, value, engine.heap.alloc_i32(3).unwrap());
}

#[tokio::test]
async fn eval_fn_decl_with_where_constraints() {
    let program = parse_program(
        r#"
        fn my_add (x: a, y: a) -> a where AdditiveMonoid a = x + y
        my_add 1 2
        "#,
    );
    let mut engine = Engine::with_prelude(()).unwrap();
    for decl in &program.decls {
        if let rexlang_ast::expr::Decl::Type(ty) = decl {
            engine.inject_type_decl(ty).unwrap();
        }
    }
    let expr = program.expr_with_fns();
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    assert_pointer_eq!(&engine.heap, value, engine.heap.alloc_i32(3).unwrap());
}

#[tokio::test]
async fn eval_adt_record_projection_single_variant() {
    let program = parse_program(
        r#"
        type MyADT = MyVariant1 { field1: i32, field2: f32 }
        let
            x = MyVariant1 { field1 = 1, field2 = 2.0 }
        in
            (x.field1, x.field2)
        "#,
    );
    let mut engine = Engine::with_prelude(()).unwrap();
    for decl in &program.decls {
        if let rexlang_ast::expr::Decl::Type(ty) = decl {
            engine.inject_type_decl(ty).unwrap();
        }
    }
    let value = eval_expr(&mut engine, program.expr.as_ref()).await.unwrap();
    let value = pval!(engine, value);
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(engine, xs);
            assert_pointer_eq!(
                &engine.heap,
                engine.heap.alloc_value(xs[0].clone()).unwrap(),
                engine.heap.alloc_i32(1).unwrap()
            );
            match xs[1] {
                Value::F32(v) => assert!((v - 2.0).abs() < 1e-3),
                _ => panic!("expected f32 field"),
            }
        }
        _ => panic!("expected tuple result"),
    }
}

#[tokio::test]
async fn eval_adt_record_projection_match_arm() {
    let program = parse_program(
        r#"
        type MyADT = MyVariant1 { field1: i32 } | MyVariant2 i32
        let
            x = MyVariant1 { field1 = 1 }
        in
            match x
                when MyVariant1 { field1 } -> x.field1
                when MyVariant2 _ -> 0
        "#,
    );
    let mut engine = Engine::with_prelude(()).unwrap();
    for decl in &program.decls {
        if let rexlang_ast::expr::Decl::Type(ty) = decl {
            engine.inject_type_decl(ty).unwrap();
        }
    }
    let value = eval_expr(&mut engine, program.expr.as_ref()).await.unwrap();
    assert_pointer_eq!(&engine.heap, value, engine.heap.alloc_i32(1).unwrap());
}

#[tokio::test]
async fn eval_list_map_fold_filter() {
    let expr = parse(
        r#"
        let
            xs = [1, 2, 3],
            ys = map (\x -> x + 1) xs,
            zs = filter (\x -> x == 2) xs,
            total = foldl (\acc x -> acc + x) 0 xs
        in
            (ys, zs, total)
        "#,
    );
    let mut engine = Engine::with_prelude(()).unwrap();
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    let value = pval!(engine, value);
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(engine, xs);
            assert_eq!(xs.len(), 3);
            let vals = list_values(&engine, &xs[0]);
            assert_eq!(vals.len(), 3);
            assert_pointer_eq!(&engine.heap, vals[0], engine.heap.alloc_i32(2).unwrap());
            assert_pointer_eq!(&engine.heap, vals[1], engine.heap.alloc_i32(3).unwrap());
            assert_pointer_eq!(&engine.heap, vals[2], engine.heap.alloc_i32(4).unwrap());
            let vals = list_values(&engine, &xs[1]);
            assert_eq!(vals.len(), 1);
            assert_pointer_eq!(&engine.heap, vals[0], engine.heap.alloc_i32(2).unwrap());
            assert_pointer_eq!(
                &engine.heap,
                engine.heap.alloc_value(xs[2].clone()).unwrap(),
                engine.heap.alloc_i32(6).unwrap()
            );
        }
        _ => panic!("expected tuple result"),
    }
}

#[tokio::test]
async fn eval_list_flat_map_zip_unzip() {
    let expr = parse(
        r#"
        let
            xs = bind (\x -> [x, x]) [1, 2],
            pairs = zip [1, 2] [3, 4],
            unzipped = unzip pairs
        in
            (xs, unzipped)
        "#,
    );
    let mut engine = Engine::with_prelude(()).unwrap();
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    let value = pval!(engine, value);
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(engine, xs);
            assert_eq!(xs.len(), 2);
            let vals = list_values(&engine, &xs[0]);
            assert_eq!(vals.len(), 4);
            assert_pointer_eq!(&engine.heap, vals[0], engine.heap.alloc_i32(1).unwrap());
            assert_pointer_eq!(&engine.heap, vals[1], engine.heap.alloc_i32(1).unwrap());
            assert_pointer_eq!(&engine.heap, vals[2], engine.heap.alloc_i32(2).unwrap());
            assert_pointer_eq!(&engine.heap, vals[3], engine.heap.alloc_i32(2).unwrap());
            match &xs[1] {
                Value::Tuple(parts) => {
                    let parts = pvals!(engine, parts);
                    assert_eq!(parts.len(), 2);
                    list_values(&engine, &parts[0]);
                    list_values(&engine, &parts[1]);
                }
                _ => panic!("expected unzip tuple"),
            }
        }
        _ => panic!("expected tuple result"),
    }
}

#[tokio::test]
async fn eval_list_sum_mean_min_max() {
    let expr = parse(
        r#"
        let
            s = sum [1, 2, 3],
            m = mean [1.0, 2.0, 3.0],
            lo = min [3, 1, 2],
            hi = max [3, 1, 2]
        in
            (s, m, lo, hi)
        "#,
    );
    let mut engine = Engine::with_prelude(()).unwrap();
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    let value = pval!(engine, value);
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(engine, xs);
            assert_eq!(xs.len(), 4);
            assert_pointer_eq!(
                &engine.heap,
                engine.heap.alloc_value(xs[0].clone()).unwrap(),
                engine.heap.alloc_i32(6).unwrap()
            );
            match xs[1] {
                Value::F32(v) => assert!((v - 2.0).abs() < 1e-3),
                _ => panic!("expected mean f32"),
            }
            assert_pointer_eq!(
                &engine.heap,
                engine.heap.alloc_value(xs[2].clone()).unwrap(),
                engine.heap.alloc_i32(1).unwrap()
            );
            assert_pointer_eq!(
                &engine.heap,
                engine.heap.alloc_value(xs[3].clone()).unwrap(),
                engine.heap.alloc_i32(3).unwrap()
            );
        }
        _ => panic!("expected tuple result"),
    }
}

#[tokio::test]
async fn eval_option_result_helpers() {
    let expr = parse(
        r#"
        let
            opt = map (\x -> x + 1) (Some (1 is i32)),
            opt2 = bind (\x -> Some (x + 1)) opt,
            res = map (\x -> x + 1) ((Ok (1 is i32)) is Result i32 string),
            ok = is_ok res,
            err = is_err (Err "nope")
        in
            (opt2, res, ok, err)
        "#,
    );
    let mut engine = Engine::with_prelude(()).unwrap();
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    let value = pval!(engine, value);
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(engine, xs);
            assert_eq!(xs.len(), 4);
            assert!(matches!(xs[0], Value::Adt(ref n, _) if sym_eq(n, "Some")));
            assert!(matches!(xs[1], Value::Adt(ref n, _) if sym_eq(n, "Ok")));
            assert_pointer_eq!(
                &engine.heap,
                engine.heap.alloc_value(xs[2].clone()).unwrap(),
                engine.heap.alloc_bool(true).unwrap()
            );
            assert_pointer_eq!(
                &engine.heap,
                engine.heap.alloc_value(xs[3].clone()).unwrap(),
                engine.heap.alloc_bool(true).unwrap()
            );
        }
        _ => panic!("expected tuple result"),
    }
}

#[tokio::test]
async fn eval_order_ops() {
    let expr = parse(
        r#"
        let
            a = (1 is i32) < (2 is i32),
            b = (2 is i32) <= (2 is i32),
            c = (3 is i32) > (2 is i32),
            d = (2 is i32) >= (3 is i32),
            e = "a" < "b"
        in
            (a, b, c, d, e)
        "#,
    );
    let mut engine = Engine::with_prelude(()).unwrap();
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    let value = pval!(engine, value);
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(engine, xs);
            assert_eq!(xs.len(), 5);
            assert_pointer_eq!(
                &engine.heap,
                engine.heap.alloc_value(xs[0].clone()).unwrap(),
                engine.heap.alloc_bool(true).unwrap()
            );
            assert_pointer_eq!(
                &engine.heap,
                engine.heap.alloc_value(xs[1].clone()).unwrap(),
                engine.heap.alloc_bool(true).unwrap()
            );
            assert_pointer_eq!(
                &engine.heap,
                engine.heap.alloc_value(xs[2].clone()).unwrap(),
                engine.heap.alloc_bool(true).unwrap()
            );
            assert_pointer_eq!(
                &engine.heap,
                engine.heap.alloc_value(xs[3].clone()).unwrap(),
                engine.heap.alloc_bool(false).unwrap()
            );
            assert_pointer_eq!(
                &engine.heap,
                engine.heap.alloc_value(xs[4].clone()).unwrap(),
                engine.heap.alloc_bool(true).unwrap()
            );
        }
        _ => panic!("expected tuple result"),
    }
}

#[tokio::test]
async fn eval_option_and_then_or_else() {
    let expr = parse(
        r#"
        let
            inc_if_pos = \x -> if x > 0 then Some (x + 1) else None,
            a = bind inc_if_pos (Some 1),
            b = bind inc_if_pos (Some 0),
            c = or_else (\x -> Some 42) b
        in
            (a, b, c)
        "#,
    );
    let mut engine = Engine::with_prelude(()).unwrap();
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    let value = pval!(engine, value);
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(engine, xs);
            assert_eq!(xs.len(), 3);
            assert!(matches!(xs[0], Value::Adt(ref n, _) if sym_eq(n, "Some")));
            assert!(matches!(xs[1], Value::Adt(ref n, _) if sym_eq(n, "None")));
            assert!(matches!(xs[2], Value::Adt(ref n, _) if sym_eq(n, "Some")));
        }
        _ => panic!("expected tuple result"),
    }
}

#[tokio::test]
async fn eval_result_filter_pipeline() {
    let expr = parse(
        r#"
        let
            classify = \x -> if x < 2 then Err x else Ok x,
            xs = [0, 2, 3],
            ys = map classify xs,
            zs = filter_map (\x -> match x when Ok v -> Some v when Err _ -> None) ys,
            total = sum zs
        in
            (count ys, total)
        "#,
    );
    let mut engine = Engine::with_prelude(()).unwrap();
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    let value = pval!(engine, value);
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(engine, xs);
            assert_eq!(xs.len(), 2);
            assert_pointer_eq!(
                &engine.heap,
                engine.heap.alloc_value(xs[0].clone()).unwrap(),
                engine.heap.alloc_i32(3).unwrap()
            );
            assert_pointer_eq!(
                &engine.heap,
                engine.heap.alloc_value(xs[1].clone()).unwrap(),
                engine.heap.alloc_i32(5).unwrap()
            );
        }
        _ => panic!("expected tuple result"),
    }
}

#[tokio::test]
async fn eval_array_combinators() {
    let mut engine = Engine::with_prelude(()).unwrap();
    engine.export_value("arr", vec![1i32, 2i32, 3i32]).unwrap();
    let expr = parse(
        r#"
        let
            mapped = map (\x -> x + 1) arr,
            total = sum arr,
            taken = take 2 arr,
            skipped = skip 1 arr,
            pairs = zip arr mapped,
            unzipped = unzip pairs
        in
            (mapped, total, taken, skipped, unzipped)
        "#,
    );
    let value = eval_expr(&mut engine, expr.as_ref()).await.unwrap();
    let value = pval!(engine, value);
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(engine, xs);
            assert_eq!(xs.len(), 5);
            match &xs[0] {
                Value::Array(vals) => {
                    let vals = pvals!(engine, vals);
                    assert_eq!(vals.len(), 3);
                    assert_pointer_eq!(
                        &engine.heap,
                        engine.heap.alloc_value(vals[0].clone()).unwrap(),
                        engine.heap.alloc_i32(2).unwrap()
                    );
                    assert_pointer_eq!(
                        &engine.heap,
                        engine.heap.alloc_value(vals[1].clone()).unwrap(),
                        engine.heap.alloc_i32(3).unwrap()
                    );
                    assert_pointer_eq!(
                        &engine.heap,
                        engine.heap.alloc_value(vals[2].clone()).unwrap(),
                        engine.heap.alloc_i32(4).unwrap()
                    );
                }
                _ => panic!("expected mapped array"),
            }
            assert_pointer_eq!(
                &engine.heap,
                engine.heap.alloc_value(xs[1].clone()).unwrap(),
                engine.heap.alloc_i32(6).unwrap()
            );
            match &xs[2] {
                Value::Array(vals) => {
                    let vals = pvals!(engine, vals);
                    assert_eq!(vals.len(), 2);
                    assert_pointer_eq!(
                        &engine.heap,
                        engine.heap.alloc_value(vals[0].clone()).unwrap(),
                        engine.heap.alloc_i32(1).unwrap()
                    );
                    assert_pointer_eq!(
                        &engine.heap,
                        engine.heap.alloc_value(vals[1].clone()).unwrap(),
                        engine.heap.alloc_i32(2).unwrap()
                    );
                }
                _ => panic!("expected taken array"),
            }
            match &xs[3] {
                Value::Array(vals) => {
                    let vals = pvals!(engine, vals);
                    assert_eq!(vals.len(), 2);
                    assert_pointer_eq!(
                        &engine.heap,
                        engine.heap.alloc_value(vals[0].clone()).unwrap(),
                        engine.heap.alloc_i32(2).unwrap()
                    );
                    assert_pointer_eq!(
                        &engine.heap,
                        engine.heap.alloc_value(vals[1].clone()).unwrap(),
                        engine.heap.alloc_i32(3).unwrap()
                    );
                }
                _ => panic!("expected skipped array"),
            }
            match &xs[4] {
                Value::Tuple(parts) => {
                    let parts = pvals!(engine, parts);
                    assert_eq!(parts.len(), 2);
                    match &parts[0] {
                        Value::Array(vals) => assert_eq!(vals.len(), 3),
                        _ => panic!("expected unzipped left array"),
                    }
                    match &parts[1] {
                        Value::Array(vals) => assert_eq!(vals.len(), 3),
                        _ => panic!("expected unzipped right array"),
                    }
                }
                _ => panic!("expected unzipped tuple"),
            }
        }
        _ => panic!("expected tuple result"),
    }
}
