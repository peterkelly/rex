use rexlang::{BuiltinTypeId, Engine, GasMeter, Parser, Token, Type, Value};

#[tokio::test]
async fn record_update_end_to_end() {
    let code = r#"
        type Foo = Bar { x: i32, y: i32, z: i32 }
        type Sum = A { x: i32 } | B { x: i32 }

        let
            foo: Foo = Bar { x = 1, y = 2, z = 3 },
            foo2 = { foo with { x = 6 } },
            sum: Sum = A { x = 1 },
            sum2 = match sum
                when A {x} -> { sum with { x = x + 1 } }
                when B {x} -> { sum with { x = x + 2 } }
        in
            (foo2.x, match sum2 when A {x} -> x when B {x} -> x)
    "#;
    let tokens = Token::tokenize(code).unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();

    let mut engine = Engine::with_prelude(()).unwrap();
    engine.inject_decls(&program.decls).unwrap();
    let mut gas = GasMeter::default();
    let (value_ptr, ty) = engine.eval(program.expr.as_ref(), &mut gas).await.unwrap();
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32)
        ])
    );
    let value = engine
        .heap
        .get(&value_ptr)
        .map(|value| value.as_ref().clone())
        .unwrap();

    let Value::Tuple(items) = value else {
        panic!(
            "expected tuple, got {}",
            engine.heap.type_name(&value_ptr).unwrap()
        );
    };
    assert_eq!(items.len(), 2);

    let a_ptr = &items[0];
    let a_value = engine.heap.get(a_ptr).unwrap();
    let Value::I32(a) = a_value.as_ref() else {
        panic!(
            "expected i32, got {}",
            engine.heap.type_name(a_ptr).unwrap()
        );
    };
    let b_ptr = &items[1];
    let b_value = engine.heap.get(b_ptr).unwrap();
    let Value::I32(b) = b_value.as_ref() else {
        panic!(
            "expected i32, got {}",
            engine.heap.type_name(b_ptr).unwrap()
        );
    };
    assert_eq!(*a, 6);
    assert_eq!(*b, 2);
}
