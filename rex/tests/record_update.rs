use rex::{Engine, Heap, Parser, Token};

#[test]
fn record_update_end_to_end() {
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
    let program = parser.parse_program().unwrap();

    let heap = Heap::new();
    let mut engine = Engine::with_prelude(&heap).unwrap();
    engine.inject_decls(&program.decls).unwrap();
    let value = engine.eval(program.expr.as_ref()).unwrap();

    let rex_engine::Value::Tuple(items) = value else {
        panic!("expected tuple, got {value}");
    };
    let items = items
        .into_iter()
        .map(|item| item.get_value(engine.heap()).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(items.len(), 2);

    let rex_engine::Value::I32(a) = items[0] else {
        panic!("expected i32, got {}", items[0]);
    };
    let rex_engine::Value::I32(b) = items[1] else {
        panic!("expected i32, got {}", items[1]);
    };
    assert_eq!(a, 6);
    assert_eq!(b, 2);
}
