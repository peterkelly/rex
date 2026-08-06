use futures::FutureExt;
use rex_ast::{CompilationUnit, Expr};
use rex_engine::{Builder, CompileOptions, Context, EngineError, FromRex, IntoRex, Module, Value};
use rex_parser::parse as parse_rex;
use rex_typesystem::{
    error::TypeError,
    types::{BuiltinTypeId, RexType, Scheme, Type},
};
use std::sync::Arc;

fn parse(code: &str) -> Arc<Expr> {
    parse_rex(code).unwrap().body.unwrap()
}

fn parse_program(code: &str) -> CompilationUnit {
    parse_rex(code).unwrap()
}

fn strip_span(mut err: TypeError) -> TypeError {
    while let TypeError::Spanned { error, .. } = err {
        err = *error;
    }
    err
}

fn builder_with_arith() -> Builder {
    Builder::with_prelude(()).unwrap()
}

fn compile_options() -> CompileOptions {
    CompileOptions::for_module("test.main").unwrap()
}

#[derive(Clone, Debug, PartialEq)]
struct HandleOnlyI32(i32);

impl RexType for HandleOnlyI32 {
    fn rex_type() -> Type {
        Type::builtin(BuiltinTypeId::I32)
    }
}

impl FromRex for HandleOnlyI32 {
    fn from_rex(value: Value) -> Result<Self, EngineError> {
        Ok(Self(i32::from_rex(value)?))
    }
}

impl IntoRex for HandleOnlyI32 {
    fn into_rex(self) -> Result<Value, EngineError> {
        self.0.into_rex()
    }
}

fn inject_globals(
    builder: &mut Builder,
    build: impl FnOnce(&mut Module<()>) -> Result<(), EngineError>,
) {
    let mut module = Module::global();
    build(&mut module).unwrap();
    builder.inject_module(module).unwrap();
}

async fn eval_expr(builder: Builder, expr: &Expr) -> Result<Value, EngineError> {
    let compiler = builder.build_compiler();
    let program = CompilationUnit {
        decls: Vec::new(),
        body: Some(Arc::new(expr.clone())),
    };
    let (compiled, evaluator) = compiler
        .compile_program(&program, compile_options())
        .await?;
    evaluator.run(compiled, Default::default()).await
}

async fn eval_program(builder: Builder, program: &CompilationUnit) -> Result<Value, EngineError> {
    let compiler = builder.build_compiler();
    let (compiled, evaluator) = compiler.compile_program(program, compile_options()).await?;
    evaluator.run(compiled, Default::default()).await
}

#[tokio::test]
async fn compile_program_returns_evaluator() {
    let builder = Builder::with_prelude(()).unwrap();
    let compiler = builder.build_compiler();
    let parsed = parse_program("1 + 2");
    let (program, evaluator) = compiler
        .compile_program(&parsed, compile_options())
        .await
        .unwrap();
    let ty = program.result_type().clone();
    let value = evaluator.run(program, Default::default()).await.unwrap();

    assert_eq!(ty.to_string(), "i32");
    assert_eq!(value.as_i32().unwrap(), 3);
}

#[tokio::test]
async fn eval_hash_show_and_parse() {
    let expected = blake3::hash(b"rex hash conversion");
    let hex = expected.to_hex().to_string();
    let source = format!(r#"let value: Hash = unwrap (parse "{hex}") in (value, show value)"#);
    let expr = parse(&source);
    let value = eval_expr(Builder::with_prelude(()).unwrap(), expr.as_ref())
        .await
        .unwrap();

    let Value::Tuple(values) = value else {
        panic!("expected tuple");
    };
    assert_eq!(values[0].to_rust::<blake3::Hash>().unwrap(), expected);
    assert_eq!(values[1].to_rust::<String>().unwrap(), hex);

    let invalid = parse(r#"let value: Option Hash = parse "not-a-hash" in value"#);
    let value = eval_expr(Builder::with_prelude(()).unwrap(), invalid.as_ref())
        .await
        .unwrap();
    assert!(
        matches!(value, Value::Adt(ref tag, ref args) if tag.as_ref() == "None" && args.is_empty())
    );
}

#[tokio::test]
async fn compiler_is_consumed_by_compile_program() {
    let builder = Builder::with_prelude(()).unwrap();
    let compiler = builder.build_compiler();
    let parsed = parse_program("let answer = 40 + 2 in answer");
    let (program, evaluator) = compiler
        .compile_program(&parsed, compile_options())
        .await
        .unwrap();
    let value = evaluator.run(program, Default::default()).await.unwrap();

    assert_eq!(value.as_i32().unwrap(), 42);
}

macro_rules! pvals {
    ($builder:expr, $vals:expr) => {
        $vals.iter().cloned().collect::<Vec<_>>()
    };
}

macro_rules! assert_pointer_eq {
    ($ignored:expr, $lhs:expr, $rhs:expr) => {
        assert_eq!($lhs, $rhs);
    };
}

fn list_values(value: &Value) -> Vec<Value> {
    match value {
        Value::List(values) => values.clone(),
        Value::Bytes(values) => values.iter().copied().map(Value::U8).collect(),
        _ => panic!("expected list value"),
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
    let builder = Builder::with_prelude(()).unwrap();
    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(builder, xs);
            assert_eq!(xs.len(), 2);
            assert!(matches!(xs[0], Value::I32(1)));
            assert!(matches!(xs[1], Value::I32(2)));
        }
        _ => panic!("expected tuple"),
    }
}

#[tokio::test]
async fn eval_native_injection() {
    let expr = parse("inc 1");
    let mut builder = Builder::with_prelude(()).unwrap();
    inject_globals(&mut builder, |module| {
        module.export_async("inc", |_: &(), x: i32| async move { Ok(x + 1) })
    });

    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    assert_pointer_eq!((), value, Value::I32(2));
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
    let mut builder = Builder::with_prelude(()).unwrap();
    inject_globals(&mut builder, |module| {
        module.export("f0", |_: &()| Ok(0i32))?;
        module.export("f1", |_: &(), a: i32| Ok(a))?;
        module.export("f2", |_: &(), a: i32, b: i32| Ok(a + b))?;
        module.export("f3", |_: &(), a: i32, b: i32, c: i32| Ok(a + b + c))?;
        module.export("f4", |_: &(), a: i32, b: i32, c: i32, d: i32| {
            Ok(a + b + c + d)
        })?;
        module.export("f5", |_: &(), a: i32, b: i32, c: i32, d: i32, e: i32| {
            Ok(a + b + c + d + e)
        })?;
        module.export(
            "f6",
            |_: &(), a: i32, b: i32, c: i32, d: i32, e: i32, g: i32| Ok(a + b + c + d + e + g),
        )?;
        module.export(
            "f7",
            |_: &(), a: i32, b: i32, c: i32, d: i32, e: i32, g: i32, h: i32| {
                Ok(a + b + c + d + e + g + h)
            },
        )?;
        module.export(
            "f8",
            |_: &(), a: i32, b: i32, c: i32, d: i32, e: i32, g: i32, h: i32, i: i32| {
                Ok(a + b + c + d + e + g + h + i)
            },
        )?;
        Ok(())
    });

    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(builder, xs);
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
async fn compiled_program_captures_rex_declarations_in_env_snapshot() {
    let compile_builder = Builder::with_prelude(()).unwrap();

    let compiler = compile_builder.build_compiler();
    let parsed = parse_program(
        r#"
            let answer = 41 in
                answer
            "#,
    );
    let (program, evaluator) = compiler
        .compile_program(&parsed, compile_options())
        .await
        .unwrap();
    let value = evaluator.run(program, Default::default()).await.unwrap();
    assert_eq!(value.as_i32().unwrap(), 41);
}

#[tokio::test]
async fn exported_value_resolves_at_runtime() {
    let mut compile_builder = Builder::with_prelude(()).unwrap();
    inject_globals(&mut compile_builder, |module| {
        module.export_value("answer", 41i32)
    });

    let compiler = compile_builder.build_compiler();
    let parsed = parse_program("answer + 1");
    let (program, evaluator) = compiler
        .compile_program(&parsed, compile_options())
        .await
        .unwrap();
    let value = evaluator.run(program, Default::default()).await.unwrap();
    assert_eq!(value.as_i32().unwrap(), 42);
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
    let mut builder = Builder::with_prelude(()).unwrap();
    inject_globals(&mut builder, |module| {
        module.export_async("af0", |_: &()| async { Ok(0i32) })?;
        module.export_async("af1", |_: &(), a: i32| async move { Ok(a) })?;
        module.export_async("af2", |_: &(), a: i32, b: i32| async move { Ok(a + b) })?;
        module.export_async("af3", |_: &(), a: i32, b: i32, c: i32| async move {
            Ok(a + b + c)
        })?;
        module.export_async("af4", |_: &(), a: i32, b: i32, c: i32, d: i32| async move {
            Ok(a + b + c + d)
        })?;
        module.export_async(
            "af5",
            |_: &(), a: i32, b: i32, c: i32, d: i32, e: i32| async move { Ok(a + b + c + d + e) },
        )?;
        module.export_async(
            "af6",
            |_: &(), a: i32, b: i32, c: i32, d: i32, e: i32, g: i32| async move {
                Ok(a + b + c + d + e + g)
            },
        )?;
        module.export_async(
            "af7",
            |_: &(), a: i32, b: i32, c: i32, d: i32, e: i32, g: i32, h: i32| async move {
                Ok(a + b + c + d + e + g + h)
            },
        )?;
        module.export_async(
            "af8",
            |_: &(), a: i32, b: i32, c: i32, d: i32, e: i32, g: i32, h: i32, i: i32| async move {
                Ok(a + b + c + d + e + g + h + i)
            },
        )?;
        Ok(())
    });

    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(builder, xs);
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
async fn eval_deep_list_does_not_overflow() {
    // Regression test: large runtime lists must not overflow the default Rust stack.
    const N: usize = 5_000;
    let mut code = String::new();
    code.push_str("let xs = [");
    for i in 0..N {
        if i > 0 {
            code.push_str(", ");
        }
        code.push('0');
    }
    code.push_str("] in xs");

    let program = parse_rex(&code).unwrap();
    let expr = program.body.unwrap();
    let builder = Builder::with_prelude(()).unwrap();
    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    let xs = list_values(&value);
    assert_eq!(xs.len(), N);
    assert_eq!(xs.first(), Some(&Value::I32(0)));
    assert_eq!(xs.last(), Some(&Value::I32(0)));
}

#[tokio::test]
async fn eval_type_annotation_let() {
    let expr = parse("let x: i32 = 42 in x");
    let builder = builder_with_arith();
    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    assert_pointer_eq!((), value, Value::I32(42));
}

#[tokio::test]
async fn eval_type_annotation_is() {
    let expr = parse("\"hi\" is str");
    let builder = builder_with_arith();
    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    assert_pointer_eq!((), value, Value::String("hi".into()));
}

#[tokio::test]
async fn eval_type_annotation_lambda_param() {
    let expr = parse("let f = \\ (a : f32) -> a in f 1.5");
    let builder = builder_with_arith();
    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    assert!(matches!(value, Value::F32(v) if (v - 1.5).abs() < f32::EPSILON));
}

#[tokio::test]
async fn eval_record_update_single_variant_adt() {
    let program = parse_program(
        r#"
        type Foo = Bar { x: i32, y: i32, z: i32 };
        let
          foo: Foo = Bar { x = 1, y = 2, z = 3 },
          bar: Foo = { foo with { x = 6 } }
        in
          bar.x
        "#,
    );
    let builder = builder_with_arith();
    let value = eval_program(builder, &program).await.unwrap();
    assert_pointer_eq!((), value, Value::I32(6));
}

#[tokio::test]
async fn eval_record_update_refined_by_match() {
    let program = parse_program(
        r#"
        type Foo = Bar { x: i32 } | Baz { x: i32 };
        let
          foo: Foo = Bar { x = 1 }
        in
          match foo with {
            case Bar {x} -> (match ({ foo with { x = x + 1 } }) with { case Bar {x} -> x; case Baz {x} -> x; });
            case Baz {x} -> (match ({ foo with { x = x + 2 } }) with { case Bar {x} -> x; case Baz {x} -> x; });
          }
        "#,
    );
    let builder = builder_with_arith();
    let value = eval_program(builder, &program).await.unwrap();
    assert_pointer_eq!((), value, Value::I32(2));
}

#[tokio::test]
async fn eval_record_update_plain_record_type() {
    let program = parse_program(
        r#"
        let
          f = \ (r : { x: i32, y: i32 }) -> { r with { y = 9 } }
        in
          match (f { x = 1, y = 2 }) with { case {y} -> y; }
        "#,
    );
    let builder = builder_with_arith();
    let value = eval_program(builder, &program).await.unwrap();
    assert_pointer_eq!((), value, Value::I32(9));
}

#[tokio::test]
async fn eval_type_annotation_mismatch() {
    let expr = parse("let x: i32 = 3.14 in x");
    let builder = builder_with_arith();
    match eval_expr(builder, expr.as_ref()).await {
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
    let builder = builder_with_arith();
    match eval_expr(builder, expr.as_ref()).await {
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
    fn builder_with_natives() -> Builder {
        let mut builder = Builder::new(());
        inject_globals(&mut builder, |module| {
            module.export("zero", |_: &()| Ok(0u32))?;
            module.export("(+)", |_: &(), x: u32, y: u32| Ok(x + y))?;
            module.export_value("one", 1u32)?;
            Ok(())
        });
        builder
    }

    let expr = parse("one + one");
    let value = eval_expr(builder_with_natives(), expr.as_ref())
        .await
        .unwrap();
    assert_eq!(value.as_u32().unwrap(), 2);

    let expr = parse("zero");
    let value = eval_expr(builder_with_natives(), expr.as_ref())
        .await
        .unwrap();
    assert_eq!(value.as_u32().unwrap(), 0);
}

#[tokio::test]
async fn typed_native_injection_uses_owned_value_conversions() {
    let mut builder = builder_with_arith();
    inject_globals(&mut builder, |module| {
        module.export("bump_handle_only", |_: &(), value: HandleOnlyI32| {
            Ok(HandleOnlyI32(value.0 + 1))
        })?;
        module.export(
            "shift_handle_only_list",
            |_: &(), values: Vec<HandleOnlyI32>| {
                Ok(values
                    .into_iter()
                    .map(|value| HandleOnlyI32(value.0 + 10))
                    .collect::<Vec<_>>())
            },
        )?;
        Ok(())
    });

    let expr = parse("(bump_handle_only 41, shift_handle_only_list [1, 2, 3])");
    let compiler = builder.build_compiler();
    let program = CompilationUnit {
        decls: Vec::new(),
        body: Some(expr),
    };
    let (compiled, evaluator) = compiler
        .compile_program(&program, compile_options())
        .await
        .unwrap();
    let ty = compiled.result_type().clone();
    let ptr = evaluator.run(compiled, Default::default()).await.unwrap();

    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::I32),
            Type::list(Type::builtin(BuiltinTypeId::I32)),
        ])
    );

    let Value::Tuple(items) = ptr else {
        panic!("expected tuple");
    };
    assert_eq!(items[0].to_rust::<i32>().unwrap(), 42);

    let shifted = items[1]
        .as_list()
        .unwrap()
        .iter()
        .map(|item| item.to_rust::<i32>().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(shifted, vec![11, 12, 13]);
}

#[tokio::test]
async fn eval_export_err_is_evaluation_failure() {
    let mut builder = Builder::new(());
    inject_globals(&mut builder, |module| {
        module.export("fail", |_: &()| {
            Err::<i32, _>(EngineError::Custom("boom".into()))
        })
    });

    let expr = parse("fail");
    match eval_expr(builder, expr.as_ref()).await {
        Err(EngineError::Custom(msg)) => assert_eq!(msg, "boom"),
        Err(other) => panic!("expected custom error, got {other:?}"),
        Ok(_) => panic!("expected evaluation failure"),
    }
}

#[test]
fn engine_export_native_rejects_invalid_arity_scheme_pair() {
    let mut module = Module::global();
    let unary_scheme = Scheme::new(
        vec![],
        vec![],
        Type::fun(
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32),
        ),
    );

    let err = module
        .export_native(
            "bad",
            unary_scheme,
            2,
            |_ctx: Context<()>, _: &Type, _args| Err(EngineError::Internal("unused".into())),
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
    let mut module = Module::global();
    let unary_scheme = Scheme::new(
        vec![],
        vec![],
        Type::fun(
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32),
        ),
    );

    let err = module
        .export_native_async(
            "bad_async",
            unary_scheme,
            2,
            |_ctx: Context<()>, _: Type, _args| {
                async { Err(EngineError::Internal("unused".into())) }.boxed()
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
    let builder = builder_with_arith();

    let expr = parse(
        r#"
        match [1, 2, 3] with {
            case [] -> 0;
            case x::xs -> x;
        }
        "#,
    );
    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    assert_pointer_eq!((), value, Value::I32(1));
}

#[tokio::test]
async fn eval_cons_constructor_form_for_lists() {
    let builder = builder_with_arith();

    let expr = parse(
        r#"
        let
            from_sugar = 1::2::[],
            from_ctor = Cons 1 (Cons 2 Empty)
        in
            (from_sugar, from_ctor, match from_ctor with { case Cons h _t -> h; case [] -> 0; })
        "#,
    );
    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    let Value::Tuple(xs) = value else {
        panic!("expected tuple result");
    };
    assert_eq!(xs.len(), 3);

    let sugar = xs[0].clone();
    let ctor = xs[1].clone();
    let sugar_items = list_values(&sugar);
    let ctor_items = list_values(&ctor);
    assert_eq!(sugar_items.len(), 2);
    assert_eq!(ctor_items.len(), 2);
    assert_pointer_eq!((), sugar_items[0], Value::I32(1));
    assert_pointer_eq!((), sugar_items[1], Value::I32(2));
    assert_pointer_eq!((), ctor_items[0], Value::I32(1));
    assert_pointer_eq!((), ctor_items[1], Value::I32(2));
    assert_pointer_eq!((), xs[2], Value::I32(1));
}

#[tokio::test]
async fn eval_simple_addition() {
    let expr = parse("420 + 69");
    let builder = builder_with_arith();
    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    assert_pointer_eq!((), value, Value::I32(489));
}

#[tokio::test]
async fn eval_simple_mod() {
    let expr = parse("10 % 3");
    let builder = builder_with_arith();
    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    assert_pointer_eq!((), value, Value::I32(1));
}

#[tokio::test]
async fn eval_list_get_and_tuple_projection() {
    let expr = parse("unwrap (list_get (1 is u64) (([1, 2, 3]) is List i32))");
    let value = eval_expr(builder_with_arith(), expr.as_ref())
        .await
        .unwrap();
    assert_eq!(value.as_i32().unwrap(), 2);

    let expr = parse("(1, 2, 3).2");
    let value = eval_expr(builder_with_arith(), expr.as_ref())
        .await
        .unwrap();
    assert_eq!(value.as_i32().unwrap(), 3);
}

#[tokio::test]
async fn eval_simple_multiplication_float() {
    let expr = parse("420.0 * 6.9");
    let builder = builder_with_arith();
    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
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
    let builder = builder_with_arith();
    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    assert_pointer_eq!((), value, Value::I32(489));
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
    let builder = builder_with_arith();
    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    assert_pointer_eq!((), value, Value::I32(42));
}

#[tokio::test]
async fn eval_match_dict_and_tuple() {
    let expr = parse(
        r#"
        let
            inc = \x -> x + 1
        in
            match { foo = 1, bar = 2 } with {
                case {foo, bar} -> (inc foo, inc bar);
            }
        "#,
    );
    let builder = builder_with_arith();
    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(builder, xs);
            assert_eq!(xs.len(), 2);
            assert!(matches!(xs[0], Value::I32(2)));
            assert!(matches!(xs[1], Value::I32(3)));
        }
        _ => panic!("expected tuple result"),
    }
}

#[tokio::test]
async fn eval_match_missing_arm_errors() {
    let expr = parse("match (Err 1) with { case Ok x -> x; }");
    let builder = Builder::with_prelude(()).unwrap();
    let result = eval_expr(builder, expr.as_ref()).await;
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
    let expr = parse("match (Ok 1) with { case [] -> 0; case x::xs -> 1; }");
    let builder = Builder::with_prelude(()).unwrap();
    let result = eval_expr(builder, expr.as_ref()).await;
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
        match [1, 2, 3] with {
            case x::xs ->
                (match xs with {
                    case [] -> x;
                    case y::ys -> x + y;
                });
            case [] -> 0;
        }
        "#,
    );
    let builder = builder_with_arith();
    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    assert_pointer_eq!((), value, Value::I32(3));
}

#[tokio::test]
async fn eval_safe_div_pipeline() {
    let expr = parse(
        r#"
        let
            id = \x -> x,
            safeDiv = \a b -> if b == 0.0 then None else Some (a / b),
            noneToZero = \x -> match x with { case None -> zero; case Some y -> y; },
            someToOne = \x -> match x with { case Some _ -> one; case None -> zero; }
        in
            (
                someToOne ((id safeDiv) (id 420.0) (id 6.9)),
                someToOne (safeDiv 420.0 6.9),
                noneToZero (safeDiv 420.0 0.0)
            )
        "#,
    );
    let builder = Builder::with_prelude(()).unwrap();
    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(builder, xs);
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
        type Boxed a = Box a;
        let
            value = Box 42
        in
            match value with {
                case Box x -> x;
            }
        "#,
    );
    let builder = Builder::with_prelude(()).unwrap();
    let value = eval_program(builder, &program).await.unwrap();
    assert_pointer_eq!((), value, Value::I32(42));
}

#[tokio::test]
async fn eval_fn_decl_simple() {
    let program = parse_program(
        r#"
        fn add (x: i32, y: i32) -> i32 = x + y;
        add 1 2
        "#,
    );
    let builder = Builder::with_prelude(()).unwrap();
    let expr = program.body_with_fns().unwrap();
    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    assert_pointer_eq!((), value, Value::I32(3));
}

#[tokio::test]
async fn eval_fn_decl_with_where_constraints() {
    let program = parse_program(
        r#"
        fn my_add<a> (x: a, y: a) -> a where AdditiveMonoid a = x + y;
        my_add 1 2
        "#,
    );
    let builder = Builder::with_prelude(()).unwrap();
    let expr = program.body_with_fns().unwrap();
    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    assert_pointer_eq!((), value, Value::I32(3));
}

#[tokio::test]
async fn eval_adt_record_projection_single_variant() {
    let program = parse_program(
        r#"
        type MyADT = MyVariant1 { field1: i32, field2: f32 };
        let
            x = MyVariant1 { field1 = 1, field2 = 2.0 }
        in
            (x.field1, x.field2)
        "#,
    );
    let builder = Builder::with_prelude(()).unwrap();
    let value = eval_program(builder, &program).await.unwrap();
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(builder, xs);
            assert!(matches!(xs[0], Value::I32(1)));
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
        type MyADT = MyVariant1 { field1: i32 } | MyVariant2 i32;
        let
            x = MyVariant1 { field1 = 1 }
        in
            match x with {
                case MyVariant1 { field1 } -> x.field1;
                case MyVariant2 _ -> 0;
            }
        "#,
    );
    let builder = Builder::with_prelude(()).unwrap();
    let value = eval_program(builder, &program).await.unwrap();
    assert_pointer_eq!((), value, Value::I32(1));
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
    let builder = Builder::with_prelude(()).unwrap();
    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(builder, xs);
            assert_eq!(xs.len(), 3);
            let vals = list_values(&xs[0]);
            assert_eq!(vals.len(), 3);
            assert_pointer_eq!((), vals[0], Value::I32(2));
            assert_pointer_eq!((), vals[1], Value::I32(3));
            assert_pointer_eq!((), vals[2], Value::I32(4));
            let vals = list_values(&xs[1]);
            assert_eq!(vals.len(), 1);
            assert_pointer_eq!((), vals[0], Value::I32(2));
            assert!(matches!(xs[2], Value::I32(6)));
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
    let builder = Builder::with_prelude(()).unwrap();
    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(builder, xs);
            assert_eq!(xs.len(), 2);
            let vals = list_values(&xs[0]);
            assert_eq!(vals.len(), 4);
            assert_pointer_eq!((), vals[0], Value::I32(1));
            assert_pointer_eq!((), vals[1], Value::I32(1));
            assert_pointer_eq!((), vals[2], Value::I32(2));
            assert_pointer_eq!((), vals[3], Value::I32(2));
            match &xs[1] {
                Value::Tuple(parts) => {
                    let parts = pvals!(builder, parts);
                    assert_eq!(parts.len(), 2);
                    list_values(&parts[0]);
                    list_values(&parts[1]);
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
    let builder = Builder::with_prelude(()).unwrap();
    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(builder, xs);
            assert_eq!(xs.len(), 4);
            assert!(matches!(xs[0], Value::I32(6)));
            match xs[1] {
                Value::F32(v) => assert!((v - 2.0).abs() < 1e-3),
                _ => panic!("expected mean f32"),
            }
            assert!(matches!(xs[2], Value::I32(1)));
            assert!(matches!(xs[3], Value::I32(3)));
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
            res = map (\x -> x + 1) ((Ok (1 is i32)) is Result i32 String),
            unwrapped_opt = unwrap opt2,
            unwrapped_res = unwrap res,
            ok = is_ok res,
            err = is_err (Err "nope")
        in
            (opt2, res, unwrapped_opt, unwrapped_res, ok, err)
        "#,
    );
    let builder = Builder::with_prelude(()).unwrap();
    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(builder, xs);
            assert_eq!(xs.len(), 6);
            assert!(matches!(xs[0], Value::Adt(ref n, _) if n.as_ref() == "Some"));
            assert!(matches!(xs[1], Value::Adt(ref n, _) if n.as_ref() == "Ok"));
            assert!(matches!(xs[2], Value::I32(3)));
            assert!(matches!(xs[3], Value::I32(2)));
            assert!(matches!(xs[4], Value::Bool(true)));
            assert!(matches!(xs[5], Value::Bool(true)));
        }
        _ => panic!("expected tuple result"),
    }
}

#[tokio::test]
async fn eval_option_filter() {
    let expr = parse(
        r#"
        let
            keep = filter (\x -> x > 1) (Some (2 is i32)),
            drop = filter (\x -> x > 1) (Some (1 is i32)),
            empty = filter (\x -> x > 1) (None is Option i32)
        in
            (keep, drop, empty)
        "#,
    );
    let builder = Builder::with_prelude(()).unwrap();
    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(builder, xs);
            assert_eq!(xs.len(), 3);
            match &xs[0] {
                Value::Adt(tag, args) if tag.as_ref() == "Some" => {
                    assert_eq!(args.len(), 1);
                    assert!(matches!(args[0], Value::I32(2)));
                }
                _ => panic!("expected Some 2"),
            }
            assert!(matches!(xs[1], Value::Adt(ref tag, _) if tag.as_ref() == "None"));
            assert!(matches!(xs[2], Value::Adt(ref tag, _) if tag.as_ref() == "None"));
        }
        _ => panic!("expected tuple result"),
    }
}

#[tokio::test]
async fn eval_option_filter_map() {
    let expr = parse(
        r#"
        let
            keep = filter_map (\x -> if x > 1 then Some (x + 1) else None) (Some (2 is i32)),
            drop = filter_map (\x -> if x > 1 then Some (x + 1) else None) (Some (1 is i32)),
            empty = filter_map (\x -> if x > 1 then Some (x + 1) else None) (None is Option i32)
        in
            (keep, drop, empty)
        "#,
    );
    let builder = Builder::with_prelude(()).unwrap();
    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(builder, xs);
            assert_eq!(xs.len(), 3);
            match &xs[0] {
                Value::Adt(tag, args) if tag.as_ref() == "Some" => {
                    assert_eq!(args.len(), 1);
                    assert!(matches!(args[0], Value::I32(3)));
                }
                _ => panic!("expected Some 3"),
            }
            assert!(matches!(xs[1], Value::Adt(ref tag, _) if tag.as_ref() == "None"));
            assert!(matches!(xs[2], Value::Adt(ref tag, _) if tag.as_ref() == "None"));
        }
        _ => panic!("expected tuple result"),
    }
}

#[tokio::test]
async fn eval_unwrap_errors_for_empty_option_and_err_result() {
    let none_expr = parse("(unwrap ((None is Option i32)))");
    match eval_expr(Builder::with_prelude(()).unwrap(), none_expr.as_ref()).await {
        Err(EngineError::Custom(msg)) => assert_eq!(msg, "called unwrap on None"),
        Err(other) => panic!("expected custom error, got {other:?}"),
        Ok(_) => panic!("expected evaluation failure"),
    }

    let err_expr = parse(r#"(unwrap ((Err "boom") is Result i32 String))"#);
    match eval_expr(Builder::with_prelude(()).unwrap(), err_expr.as_ref()).await {
        Err(EngineError::Custom(msg)) => assert_eq!(msg, "called unwrap on Err"),
        Err(other) => panic!("expected custom error, got {other:?}"),
        Ok(_) => panic!("expected evaluation failure"),
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
    let builder = Builder::with_prelude(()).unwrap();
    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(builder, xs);
            assert_eq!(xs.len(), 5);
            assert!(matches!(xs[0], Value::Bool(true)));
            assert!(matches!(xs[1], Value::Bool(true)));
            assert!(matches!(xs[2], Value::Bool(true)));
            assert!(matches!(xs[3], Value::Bool(false)));
            assert!(matches!(xs[4], Value::Bool(true)));
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
    let builder = Builder::with_prelude(()).unwrap();
    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(builder, xs);
            assert_eq!(xs.len(), 3);
            assert!(matches!(xs[0], Value::Adt(ref n, _) if n.as_ref() == "Some"));
            assert!(matches!(xs[1], Value::Adt(ref n, _) if n.as_ref() == "None"));
            assert!(matches!(xs[2], Value::Adt(ref n, _) if n.as_ref() == "Some"));
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
            xs: List i32 = [0, 2, 3],
            ys = map classify xs,
            zs = filter_map (\x -> match x with { case Ok v -> Some v; case Err _ -> None; }) ys,
            total = sum zs
        in
            (length ys, total)
        "#,
    );
    let builder = Builder::with_prelude(()).unwrap();
    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    match value {
        Value::Tuple(xs) => {
            let xs = pvals!(builder, xs);
            assert_eq!(xs.len(), 2);
            assert!(matches!(xs[0], Value::U64(3)));
            assert!(matches!(xs[1], Value::I32(5)));
        }
        _ => panic!("expected tuple result"),
    }
}

#[tokio::test]
async fn eval_list_combinators_for_host_vecs() {
    let mut builder = Builder::with_prelude(()).unwrap();
    inject_globals(&mut builder, |module| {
        module.export_value("arr", vec![1i32, 2i32, 3i32])
    });
    builder.set_extreme_gc_stress(true);
    let expr = parse(
        r#"
        let
            mapped = map (\x -> x + 1) arr,
            total = sum arr,
            taken = take (2 is u64) arr,
            skipped = skip (1 is u64) arr,
            pairs = zip arr mapped,
            unzipped = unzip pairs
        in
            (mapped, total, taken, skipped, unzipped)
        "#,
    );
    let value = eval_expr(builder, expr.as_ref()).await.unwrap();
    match value {
        Value::Tuple(items) => {
            let viewed = pvals!(builder, items);
            assert_eq!(viewed.len(), 5);
            match &viewed[0] {
                value if value.value_type_name() == "list" => {
                    let vals = items[0].as_list().unwrap();
                    let vals = pvals!(builder, vals);
                    assert_eq!(vals.len(), 3);
                    assert!(matches!(vals[0], Value::I32(2)));
                    assert!(matches!(vals[1], Value::I32(3)));
                    assert!(matches!(vals[2], Value::I32(4)));
                }
                _ => panic!("expected mapped list"),
            }
            assert!(matches!(viewed[1], Value::I32(6)));
            match &viewed[2] {
                value if value.value_type_name() == "list" => {
                    let vals = items[2].as_list().unwrap();
                    let vals = pvals!(builder, vals);
                    assert_eq!(vals.len(), 2);
                    assert!(matches!(vals[0], Value::I32(1)));
                    assert!(matches!(vals[1], Value::I32(2)));
                }
                _ => panic!("expected taken list"),
            }
            match &viewed[3] {
                value if value.value_type_name() == "list" => {
                    let vals = items[3].as_list().unwrap();
                    let vals = pvals!(builder, vals);
                    assert_eq!(vals.len(), 2);
                    assert!(matches!(vals[0], Value::I32(2)));
                    assert!(matches!(vals[1], Value::I32(3)));
                }
                _ => panic!("expected skipped list"),
            }
            match &viewed[4] {
                Value::Tuple(parts) => {
                    assert_eq!(parts.len(), 2);
                    assert_eq!(parts[0].value_type_name(), "list");
                    assert_eq!(parts[1].value_type_name(), "list");
                }
                _ => panic!("expected unzipped tuple"),
            }
        }
        _ => panic!("expected tuple result"),
    }
}
