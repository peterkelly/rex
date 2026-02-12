use futures::executor::block_on;
use futures::future;
use rex_ast::expr::sym_eq;
use rex_engine::{CancellationToken, Engine, EngineError, Value};
use rex_ts::TypeError;
use rex_util::{GasCosts, GasMeter};
use std::sync::Arc;

fn parse(code: &str) -> Arc<rex_ast::expr::Expr> {
    let mut parser = rex_parser::Parser::new(rex_lexer::Token::tokenize(code).unwrap());
    parser.parse_program().unwrap().expr
}

fn parse_program(code: &str) -> rex_ast::expr::Program {
    let mut parser = rex_parser::Parser::new(rex_lexer::Token::tokenize(code).unwrap());
    parser.parse_program().unwrap()
}

fn strip_span(mut err: TypeError) -> TypeError {
    while let TypeError::Spanned { error, .. } = err {
        err = *error;
    }
    err
}

fn engine_with_arith() -> Engine {
    Engine::with_prelude().unwrap()
}

fn list_values(value: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    let mut cur = value;
    loop {
        match cur {
            Value::Adt(tag, args) if sym_eq(tag, "Empty") && args.is_empty() => return out,
            Value::Adt(tag, args) if sym_eq(tag, "Cons") && args.len() == 2 => {
                out.push(args[0].clone());
                cur = &args[1];
            }
            _ => panic!("expected list value"),
        }
    }
}

#[test]
fn eval_let_lambda() {
    let expr = parse(
        r#"
        let
            id = \x -> x
        in
            id (id 1, id 2)
        "#,
    );
    let mut engine = Engine::new();
    let value = engine.eval(expr.as_ref()).unwrap();
    match value {
        Value::Tuple(xs) => {
            assert_eq!(xs.len(), 2);
            assert_eq!(
                xs[0],
                engine
                    .heap()
                    .alloc_i32(1)
                    .unwrap()
                    .get_value(engine.heap())
                    .unwrap()
            );
            assert_eq!(
                xs[1],
                engine
                    .heap()
                    .alloc_i32(2)
                    .unwrap()
                    .get_value(engine.heap())
                    .unwrap()
            );
        }
        _ => panic!("expected tuple"),
    }
}

#[test]
fn eval_async_native_injection() {
    let expr = parse("inc 1");
    let mut engine = Engine::with_prelude().unwrap();
    engine
        .inject_async_fn1("inc", |x: i32| async move { x + 1 })
        .unwrap();

    let v_async = block_on(engine.eval_async(expr.as_ref())).unwrap();
    assert_eq!(
        v_async,
        engine
            .heap()
            .alloc_i32(2)
            .unwrap()
            .get_value(engine.heap())
            .unwrap()
    );

    let v_sync = engine.eval(expr.as_ref()).unwrap();
    assert_eq!(
        v_sync,
        engine
            .heap()
            .alloc_i32(2)
            .unwrap()
            .get_value(engine.heap())
            .unwrap()
    );
}

#[test]
fn eval_async_can_be_cancelled() {
    let expr = parse("stall");
    let mut engine = Engine::with_prelude().unwrap();

    let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
    engine
        .inject_async_fn0_cancellable("stall", move |token: CancellationToken| {
            let started_tx = started_tx.clone();
            async move {
                let _ = started_tx.send(());
                future::pending::<()>().await;
                let _ = token;
                0i32
            }
        })
        .unwrap();

    let token = engine.cancellation_token();
    let handle = std::thread::spawn(move || block_on(engine.eval_async(expr.as_ref())));

    started_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("stall native never started");
    token.cancel();

    let res = handle.join().unwrap();
    assert!(matches!(res, Err(EngineError::Cancelled)));
}

#[test]
fn eval_with_gas_rejects_out_of_budget() {
    let expr = parse("1");
    let mut engine = Engine::with_prelude().unwrap();
    let mut gas = GasMeter::new(
        Some(0),
        GasCosts {
            eval_node: 1,
            ..GasCosts::sensible_defaults()
        },
    );
    let err = match engine.eval_with_gas(expr.as_ref(), &mut gas) {
        Ok(_) => panic!("expected out of gas"),
        Err(e) => e,
    };
    assert!(matches!(err, EngineError::OutOfGas(..)));
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

    let tokens = rex_lexer::Token::tokenize(&code).unwrap();
    let program = rex_parser::Parser::new(tokens)
        .parse_program_with_stack_size(128 * 1024 * 1024)
        .unwrap();
    let expr = program.expr;
    let mut engine = Engine::with_prelude().unwrap();
    let value = engine
        .eval_with_stack_size(expr.as_ref(), 64 * 1024 * 1024)
        .unwrap();
    let xs = list_values(&value);
    assert_eq!(xs.len(), N);
    let expected = engine
        .heap()
        .alloc_i32(0)
        .unwrap()
        .get_value(engine.heap())
        .unwrap();
    assert_eq!(xs.first(), Some(&expected));
    assert_eq!(xs.last(), Some(&expected));
}

#[test]
fn eval_type_annotation_let() {
    let expr = parse("let x: i32 = 42 in x");
    let mut engine = engine_with_arith();
    let value = engine.eval(expr.as_ref()).unwrap();
    assert_eq!(
        value,
        engine
            .heap()
            .alloc_i32(42)
            .unwrap()
            .get_value(engine.heap())
            .unwrap()
    );
}

#[test]
fn eval_type_annotation_is() {
    let expr = parse("\"hi\" is str");
    let mut engine = engine_with_arith();
    let value = engine.eval(expr.as_ref()).unwrap();
    assert_eq!(
        value,
        engine
            .heap()
            .alloc_string("hi".into())
            .unwrap()
            .get_value(engine.heap())
            .unwrap()
    );
}

#[test]
fn eval_type_annotation_lambda_param() {
    let expr = parse("let f = \\ (a : f32) -> a in f 1.5");
    let mut engine = engine_with_arith();
    let value = engine.eval(expr.as_ref()).unwrap();
    assert!(matches!(value, Value::F32(v) if (v - 1.5).abs() < f32::EPSILON));
}

#[test]
fn eval_record_update_single_variant_adt() {
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
    let value = engine.eval(program.expr.as_ref()).unwrap();
    assert_eq!(
        value,
        engine
            .heap()
            .alloc_i32(6)
            .unwrap()
            .get_value(engine.heap())
            .unwrap()
    );
}

#[test]
fn eval_record_update_refined_by_match() {
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
    let value = engine.eval(program.expr.as_ref()).unwrap();
    assert_eq!(
        value,
        engine
            .heap()
            .alloc_i32(2)
            .unwrap()
            .get_value(engine.heap())
            .unwrap()
    );
}

#[test]
fn eval_record_update_plain_record_type() {
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
    let value = engine.eval(program.expr.as_ref()).unwrap();
    assert_eq!(
        value,
        engine
            .heap()
            .alloc_i32(9)
            .unwrap()
            .get_value(engine.heap())
            .unwrap()
    );
}

#[test]
fn eval_type_annotation_mismatch() {
    let expr = parse("let x: i32 = 3.14 in x");
    let mut engine = engine_with_arith();
    match engine.eval(expr.as_ref()) {
        Err(EngineError::Type(err)) => {
            let err = strip_span(err);
            assert!(matches!(err, TypeError::Unification(_, _)));
        }
        Err(other) => panic!("expected type error, got {other:?}"),
        Ok(_) => panic!("expected type error, got Ok"),
    }
}

#[test]
fn eval_native_injection() {
    let mut engine = Engine::new();
    engine.inject_fn0("zero", || -> u32 { 0u32 }).unwrap();
    engine
        .inject_fn2("(+)", |x: u32, y: u32| -> u32 { x + y })
        .unwrap();
    engine.inject_value("one", 1u32).unwrap();

    let expr = parse("one + one");
    let value = engine.eval(expr.as_ref()).unwrap();
    assert_eq!(
        value,
        engine
            .heap()
            .alloc_u32(2)
            .unwrap()
            .get_value(engine.heap())
            .unwrap()
    );

    let expr = parse("zero");
    let value = engine.eval(expr.as_ref()).unwrap();
    assert_eq!(
        value,
        engine
            .heap()
            .alloc_u32(0)
            .unwrap()
            .get_value(engine.heap())
            .unwrap()
    );
}

#[test]
fn eval_match_list() {
    let mut engine = engine_with_arith();

    let expr = parse(
        r#"
        match [1, 2, 3]
            when [] -> 0
            when x:xs -> x
        "#,
    );
    let value = engine.eval(expr.as_ref()).unwrap();
    assert_eq!(
        value,
        engine
            .heap()
            .alloc_i32(1)
            .unwrap()
            .get_value(engine.heap())
            .unwrap()
    );
}

#[test]
fn eval_simple_addition() {
    let expr = parse("420 + 69");
    let mut engine = engine_with_arith();
    let value = engine.eval(expr.as_ref()).unwrap();
    assert_eq!(
        value,
        engine
            .heap()
            .alloc_i32(489)
            .unwrap()
            .get_value(engine.heap())
            .unwrap()
    );
}

#[test]
fn eval_simple_mod() {
    let expr = parse("10 % 3");
    let mut engine = engine_with_arith();
    let value = engine.eval(expr.as_ref()).unwrap();
    assert_eq!(
        value,
        engine
            .heap()
            .alloc_i32(1)
            .unwrap()
            .get_value(engine.heap())
            .unwrap()
    );
}

#[test]
fn eval_get_list_and_tuple() {
    let mut engine = engine_with_arith();

    let expr = parse("get 1 [1, 2, 3]");
    let value = engine.eval(expr.as_ref()).unwrap();
    assert_eq!(
        value,
        engine
            .heap()
            .alloc_i32(2)
            .unwrap()
            .get_value(engine.heap())
            .unwrap()
    );

    let expr = parse("(1, 2, 3).2");
    let value = engine.eval(expr.as_ref()).unwrap();
    assert_eq!(
        value,
        engine
            .heap()
            .alloc_i32(3)
            .unwrap()
            .get_value(engine.heap())
            .unwrap()
    );
}

#[test]
fn eval_simple_multiplication_float() {
    let expr = parse("420.0 * 6.9");
    let mut engine = engine_with_arith();
    let value = engine.eval(expr.as_ref()).unwrap();
    match value {
        Value::F32(v) => assert!((v - 2898.0).abs() < 1e-3),
        _ => panic!("expected f32 result"),
    }
}

#[test]
fn eval_let_id_nested() {
    let expr = parse(
        r#"
        let
            id = \x -> x
        in
            id (id 420 + id 69)
        "#,
    );
    let mut engine = engine_with_arith();
    let value = engine.eval(expr.as_ref()).unwrap();
    assert_eq!(
        value,
        engine
            .heap()
            .alloc_i32(489)
            .unwrap()
            .get_value(engine.heap())
            .unwrap()
    );
}

#[test]
fn eval_higher_order_add() {
    let expr = parse(
        r#"
        let
            add = \x -> \y -> x + y
        in
            add 40 2
        "#,
    );
    let mut engine = engine_with_arith();
    let value = engine.eval(expr.as_ref()).unwrap();
    assert_eq!(
        value,
        engine
            .heap()
            .alloc_i32(42)
            .unwrap()
            .get_value(engine.heap())
            .unwrap()
    );
}

#[test]
fn eval_match_dict_and_tuple() {
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
    let value = engine.eval(expr.as_ref()).unwrap();
    match value {
        Value::Tuple(xs) => {
            assert_eq!(xs.len(), 2);
            assert_eq!(
                xs[0],
                engine
                    .heap()
                    .alloc_i32(2)
                    .unwrap()
                    .get_value(engine.heap())
                    .unwrap()
            );
            assert_eq!(
                xs[1],
                engine
                    .heap()
                    .alloc_i32(3)
                    .unwrap()
                    .get_value(engine.heap())
                    .unwrap()
            );
        }
        _ => panic!("expected tuple result"),
    }
}

#[test]
fn eval_match_missing_arm_errors() {
    let expr = parse("match (Err 1) when Ok x -> x");
    let mut engine = Engine::with_prelude().unwrap();
    let result = engine.eval(expr.as_ref());
    match result {
        Err(EngineError::Type(err)) => {
            let err = strip_span(err);
            assert!(matches!(err, TypeError::NonExhaustiveMatch { .. }));
        }
        _ => panic!("expected non-exhaustive match type error"),
    }
}

#[test]
fn eval_match_invalid_pattern_type_error() {
    let expr = parse("match (Ok 1) when [] -> 0 when x:xs -> 1");
    let mut engine = Engine::with_prelude().unwrap();
    let result = engine.eval(expr.as_ref());
    match result {
        Err(EngineError::Type(err)) => {
            let err = strip_span(err);
            assert!(matches!(err, TypeError::Unification(_, _)));
        }
        _ => panic!("expected unification type error"),
    }
}

#[test]
fn eval_nested_match_list_sum() {
    let expr = parse(
        r#"
        match [1, 2, 3]
            when x:xs ->
                (match xs
                    when [] -> x
                    when y:ys -> x + y)
            when [] -> 0
        "#,
    );
    let mut engine = engine_with_arith();
    let value = engine.eval(expr.as_ref()).unwrap();
    assert_eq!(
        value,
        engine
            .heap()
            .alloc_i32(3)
            .unwrap()
            .get_value(engine.heap())
            .unwrap()
    );
}

#[test]
fn eval_safe_div_pipeline() {
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
    let mut engine = Engine::with_prelude().unwrap();
    let value = engine.eval(expr.as_ref()).unwrap();
    match value {
        Value::Tuple(xs) => {
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

#[test]
fn eval_user_adt_declaration() {
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
    let mut engine = Engine::with_prelude().unwrap();
    for decl in &program.decls {
        if let rex_ast::expr::Decl::Type(ty) = decl {
            engine.inject_type_decl(ty).unwrap();
        }
    }
    let value = engine.eval(program.expr.as_ref()).unwrap();
    assert_eq!(
        value,
        engine
            .heap()
            .alloc_i32(42)
            .unwrap()
            .get_value(engine.heap())
            .unwrap()
    );
}

#[test]
fn eval_fn_decl_simple() {
    let program = parse_program(
        r#"
        fn add (x: i32, y: i32) -> i32 = x + y
        add 1 2
        "#,
    );
    let mut engine = Engine::with_prelude().unwrap();
    for decl in &program.decls {
        if let rex_ast::expr::Decl::Type(ty) = decl {
            engine.inject_type_decl(ty).unwrap();
        }
    }
    let expr = program.expr_with_fns();
    let value = engine.eval(expr.as_ref()).unwrap();
    assert_eq!(
        value,
        engine
            .heap()
            .alloc_i32(3)
            .unwrap()
            .get_value(engine.heap())
            .unwrap()
    );
}

#[test]
fn eval_fn_decl_with_where_constraints() {
    let program = parse_program(
        r#"
        fn my_add (x: a, y: a) -> a where AdditiveMonoid a = x + y
        my_add 1 2
        "#,
    );
    let mut engine = Engine::with_prelude().unwrap();
    for decl in &program.decls {
        if let rex_ast::expr::Decl::Type(ty) = decl {
            engine.inject_type_decl(ty).unwrap();
        }
    }
    let expr = program.expr_with_fns();
    let value = engine.eval(expr.as_ref()).unwrap();
    assert_eq!(
        value,
        engine
            .heap()
            .alloc_i32(3)
            .unwrap()
            .get_value(engine.heap())
            .unwrap()
    );
}

#[test]
fn eval_adt_record_projection_single_variant() {
    let program = parse_program(
        r#"
        type MyADT = MyVariant1 { field1: i32, field2: f32 }
        let
            x = MyVariant1 { field1 = 1, field2 = 2.0 }
        in
            (x.field1, x.field2)
        "#,
    );
    let mut engine = Engine::with_prelude().unwrap();
    for decl in &program.decls {
        if let rex_ast::expr::Decl::Type(ty) = decl {
            engine.inject_type_decl(ty).unwrap();
        }
    }
    let value = engine.eval(program.expr.as_ref()).unwrap();
    match value {
        Value::Tuple(xs) => {
            assert_eq!(
                xs[0],
                engine
                    .heap()
                    .alloc_i32(1)
                    .unwrap()
                    .get_value(engine.heap())
                    .unwrap()
            );
            match xs[1] {
                Value::F32(v) => assert!((v - 2.0).abs() < 1e-3),
                _ => panic!("expected f32 field"),
            }
        }
        _ => panic!("expected tuple result"),
    }
}

#[test]
fn eval_adt_record_projection_match_arm() {
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
    let mut engine = Engine::with_prelude().unwrap();
    for decl in &program.decls {
        if let rex_ast::expr::Decl::Type(ty) = decl {
            engine.inject_type_decl(ty).unwrap();
        }
    }
    let value = engine.eval(program.expr.as_ref()).unwrap();
    assert_eq!(
        value,
        engine
            .heap()
            .alloc_i32(1)
            .unwrap()
            .get_value(engine.heap())
            .unwrap()
    );
}

#[test]
fn eval_list_map_fold_filter() {
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
    let mut engine = Engine::with_prelude().unwrap();
    let value = engine.eval(expr.as_ref()).unwrap();
    match value {
        Value::Tuple(xs) => {
            assert_eq!(xs.len(), 3);
            let vals = list_values(&xs[0]);
            assert_eq!(vals.len(), 3);
            assert_eq!(
                vals[0],
                engine
                    .heap()
                    .alloc_i32(2)
                    .unwrap()
                    .get_value(engine.heap())
                    .unwrap()
            );
            assert_eq!(
                vals[1],
                engine
                    .heap()
                    .alloc_i32(3)
                    .unwrap()
                    .get_value(engine.heap())
                    .unwrap()
            );
            assert_eq!(
                vals[2],
                engine
                    .heap()
                    .alloc_i32(4)
                    .unwrap()
                    .get_value(engine.heap())
                    .unwrap()
            );
            let vals = list_values(&xs[1]);
            assert_eq!(vals.len(), 1);
            assert_eq!(
                vals[0],
                engine
                    .heap()
                    .alloc_i32(2)
                    .unwrap()
                    .get_value(engine.heap())
                    .unwrap()
            );
            assert_eq!(
                xs[2],
                engine
                    .heap()
                    .alloc_i32(6)
                    .unwrap()
                    .get_value(engine.heap())
                    .unwrap()
            );
        }
        _ => panic!("expected tuple result"),
    }
}

#[test]
fn eval_list_flat_map_zip_unzip() {
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
    let mut engine = Engine::with_prelude().unwrap();
    let value = engine.eval(expr.as_ref()).unwrap();
    match value {
        Value::Tuple(xs) => {
            assert_eq!(xs.len(), 2);
            let vals = list_values(&xs[0]);
            assert_eq!(vals.len(), 4);
            assert_eq!(
                vals[0],
                engine
                    .heap()
                    .alloc_i32(1)
                    .unwrap()
                    .get_value(engine.heap())
                    .unwrap()
            );
            assert_eq!(
                vals[1],
                engine
                    .heap()
                    .alloc_i32(1)
                    .unwrap()
                    .get_value(engine.heap())
                    .unwrap()
            );
            assert_eq!(
                vals[2],
                engine
                    .heap()
                    .alloc_i32(2)
                    .unwrap()
                    .get_value(engine.heap())
                    .unwrap()
            );
            assert_eq!(
                vals[3],
                engine
                    .heap()
                    .alloc_i32(2)
                    .unwrap()
                    .get_value(engine.heap())
                    .unwrap()
            );
            match &xs[1] {
                Value::Tuple(parts) => {
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

#[test]
fn eval_list_sum_mean_min_max() {
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
    let mut engine = Engine::with_prelude().unwrap();
    let value = engine.eval(expr.as_ref()).unwrap();
    match value {
        Value::Tuple(xs) => {
            assert_eq!(xs.len(), 4);
            assert_eq!(
                xs[0],
                engine
                    .heap()
                    .alloc_i32(6)
                    .unwrap()
                    .get_value(engine.heap())
                    .unwrap()
            );
            match xs[1] {
                Value::F32(v) => assert!((v - 2.0).abs() < 1e-3),
                _ => panic!("expected mean f32"),
            }
            assert_eq!(
                xs[2],
                engine
                    .heap()
                    .alloc_i32(1)
                    .unwrap()
                    .get_value(engine.heap())
                    .unwrap()
            );
            assert_eq!(
                xs[3],
                engine
                    .heap()
                    .alloc_i32(3)
                    .unwrap()
                    .get_value(engine.heap())
                    .unwrap()
            );
        }
        _ => panic!("expected tuple result"),
    }
}

#[test]
fn eval_option_result_helpers() {
    let expr = parse(
        r#"
        let
            opt = map (\x -> x + 1) (Some 1),
            opt2 = bind (\x -> Some (x + 1)) opt,
            res = map (\x -> x + 1) (Ok 1),
            ok = is_ok res,
            err = is_err (Err "nope")
        in
            (opt2, res, ok, err)
        "#,
    );
    let mut engine = Engine::with_prelude().unwrap();
    let value = engine.eval(expr.as_ref()).unwrap();
    match value {
        Value::Tuple(xs) => {
            assert_eq!(xs.len(), 4);
            assert!(matches!(xs[0], Value::Adt(ref n, _) if sym_eq(n, "Some")));
            assert!(matches!(xs[1], Value::Adt(ref n, _) if sym_eq(n, "Ok")));
            assert_eq!(
                xs[2],
                engine
                    .heap()
                    .alloc_bool(true)
                    .unwrap()
                    .get_value(engine.heap())
                    .unwrap()
            );
            assert_eq!(
                xs[3],
                engine
                    .heap()
                    .alloc_bool(true)
                    .unwrap()
                    .get_value(engine.heap())
                    .unwrap()
            );
        }
        _ => panic!("expected tuple result"),
    }
}

#[test]
fn eval_order_ops() {
    let expr = parse(
        r#"
        let
            a = 1 < 2,
            b = 2 <= 2,
            c = 3 > 2,
            d = 2 >= 3,
            e = "a" < "b"
        in
            (a, b, c, d, e)
        "#,
    );
    let mut engine = Engine::with_prelude().unwrap();
    let value = engine.eval(expr.as_ref()).unwrap();
    match value {
        Value::Tuple(xs) => {
            assert_eq!(xs.len(), 5);
            assert_eq!(
                xs[0],
                engine
                    .heap()
                    .alloc_bool(true)
                    .unwrap()
                    .get_value(engine.heap())
                    .unwrap()
            );
            assert_eq!(
                xs[1],
                engine
                    .heap()
                    .alloc_bool(true)
                    .unwrap()
                    .get_value(engine.heap())
                    .unwrap()
            );
            assert_eq!(
                xs[2],
                engine
                    .heap()
                    .alloc_bool(true)
                    .unwrap()
                    .get_value(engine.heap())
                    .unwrap()
            );
            assert_eq!(
                xs[3],
                engine
                    .heap()
                    .alloc_bool(false)
                    .unwrap()
                    .get_value(engine.heap())
                    .unwrap()
            );
            assert_eq!(
                xs[4],
                engine
                    .heap()
                    .alloc_bool(true)
                    .unwrap()
                    .get_value(engine.heap())
                    .unwrap()
            );
        }
        _ => panic!("expected tuple result"),
    }
}

#[test]
fn eval_option_and_then_or_else() {
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
    let mut engine = Engine::with_prelude().unwrap();
    let value = engine.eval(expr.as_ref()).unwrap();
    match value {
        Value::Tuple(xs) => {
            assert_eq!(xs.len(), 3);
            assert!(matches!(xs[0], Value::Adt(ref n, _) if sym_eq(n, "Some")));
            assert!(matches!(xs[1], Value::Adt(ref n, _) if sym_eq(n, "None")));
            assert!(matches!(xs[2], Value::Adt(ref n, _) if sym_eq(n, "Some")));
        }
        _ => panic!("expected tuple result"),
    }
}

#[test]
fn eval_result_filter_pipeline() {
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
    let mut engine = Engine::with_prelude().unwrap();
    let value = engine.eval(expr.as_ref()).unwrap();
    match value {
        Value::Tuple(xs) => {
            assert_eq!(xs.len(), 2);
            assert_eq!(
                xs[0],
                engine
                    .heap()
                    .alloc_i32(3)
                    .unwrap()
                    .get_value(engine.heap())
                    .unwrap()
            );
            assert_eq!(
                xs[1],
                engine
                    .heap()
                    .alloc_i32(5)
                    .unwrap()
                    .get_value(engine.heap())
                    .unwrap()
            );
        }
        _ => panic!("expected tuple result"),
    }
}

#[test]
fn eval_array_combinators() {
    let mut engine = Engine::with_prelude().unwrap();
    engine.inject_value("arr", vec![1i32, 2i32, 3i32]).unwrap();
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
    let value = engine.eval(expr.as_ref()).unwrap();
    match value {
        Value::Tuple(xs) => {
            assert_eq!(xs.len(), 5);
            match &xs[0] {
                Value::Array(vals) => {
                    assert_eq!(vals.len(), 3);
                    assert_eq!(
                        vals[0],
                        engine
                            .heap()
                            .alloc_i32(2)
                            .unwrap()
                            .get_value(engine.heap())
                            .unwrap()
                    );
                    assert_eq!(
                        vals[1],
                        engine
                            .heap()
                            .alloc_i32(3)
                            .unwrap()
                            .get_value(engine.heap())
                            .unwrap()
                    );
                    assert_eq!(
                        vals[2],
                        engine
                            .heap()
                            .alloc_i32(4)
                            .unwrap()
                            .get_value(engine.heap())
                            .unwrap()
                    );
                }
                _ => panic!("expected mapped array"),
            }
            assert_eq!(
                xs[1],
                engine
                    .heap()
                    .alloc_i32(6)
                    .unwrap()
                    .get_value(engine.heap())
                    .unwrap()
            );
            match &xs[2] {
                Value::Array(vals) => {
                    assert_eq!(vals.len(), 2);
                    assert_eq!(
                        vals[0],
                        engine
                            .heap()
                            .alloc_i32(1)
                            .unwrap()
                            .get_value(engine.heap())
                            .unwrap()
                    );
                    assert_eq!(
                        vals[1],
                        engine
                            .heap()
                            .alloc_i32(2)
                            .unwrap()
                            .get_value(engine.heap())
                            .unwrap()
                    );
                }
                _ => panic!("expected taken array"),
            }
            match &xs[3] {
                Value::Array(vals) => {
                    assert_eq!(vals.len(), 2);
                    assert_eq!(
                        vals[0],
                        engine
                            .heap()
                            .alloc_i32(2)
                            .unwrap()
                            .get_value(engine.heap())
                            .unwrap()
                    );
                    assert_eq!(
                        vals[1],
                        engine
                            .heap()
                            .alloc_i32(3)
                            .unwrap()
                            .get_value(engine.heap())
                            .unwrap()
                    );
                }
                _ => panic!("expected skipped array"),
            }
            match &xs[4] {
                Value::Tuple(parts) => {
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
