mod common;

use rex::{
    engine::{Builder, Value},
    typesystem::{BuiltinTypeId, Type},
};

#[tokio::test]
async fn record_update_end_to_end() {
    let code = r#"
        type Foo = Bar { x: i32, y: i32, z: i32 };
        type Sum = A { x: i32 } | B { x: i32 };

        let
            foo: Foo = Bar { x = 1, y = 2, z = 3 },
            foo2 = { foo with { x = 6 } },
            sum: Sum = A { x = 1 },
            sum2 = match sum with {
                case A {x} -> { sum with { x = x + 1 } };
                case B {x} -> { sum with { x = x + 2 } };
            }
        in
            (foo2.x, match sum2 with { case A {x} -> x; case B {x} -> x; })
    "#;
    let (_heap, value_handle, ty) = common::eval_source(Builder::with_prelude(()).unwrap(), code)
        .await
        .unwrap();
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32)
        ])
    );
    let Value::Tuple(items) = value_handle.value().unwrap() else {
        panic!("expected tuple, got {}", value_handle.type_name().unwrap());
    };
    assert_eq!(items.len(), 2);

    let a_handle = &items[0];
    let Value::I32(a) = a_handle.value().unwrap() else {
        panic!("expected i32, got {}", a_handle.type_name().unwrap());
    };
    let b_handle = &items[1];
    let Value::I32(b) = b_handle.value().unwrap() else {
        panic!("expected i32, got {}", b_handle.type_name().unwrap());
    };
    assert_eq!(a, 6);
    assert_eq!(b, 2);
}
