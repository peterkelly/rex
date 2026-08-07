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
    let Value::Tuple(items) = value_handle else {
        panic!("expected tuple, got {}", value_handle.value_type_name());
    };
    assert_eq!(items.len(), 2);

    let a_handle = &items[0];
    let Value::I32(a) = a_handle else {
        panic!("expected i32, got {}", a_handle.value_type_name());
    };
    let b_handle = &items[1];
    let Value::I32(b) = b_handle else {
        panic!("expected i32, got {}", b_handle.value_type_name());
    };
    assert_eq!(*a, 6);
    assert_eq!(*b, 2);
}

#[tokio::test]
async fn named_records_pass_through_functions_and_return_updated_values() {
    let code = r#"
        type Person = { name: String, age: i32 };

        fn make_person name: String -> age: i32 -> Person =
            { name = name, age = age };

        fn birthday person: Person -> Person =
            { person with { age = person.age + 1 } };

        let
            ada: Person = make_person "Ada" 36,
            older: Person = birthday ada
        in
            (older.name, older.age)
    "#;
    let (_heap, value, ty) = common::eval_source(Builder::with_prelude(()).unwrap(), code)
        .await
        .unwrap();

    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::String),
            Type::builtin(BuiltinTypeId::I32),
        ])
    );
    assert_eq!(
        value,
        Value::Tuple(vec![Value::String("Ada".into()), Value::I32(37)])
    );
}

#[tokio::test]
async fn named_records_support_nesting_generics_projection_and_patterns() {
    let code = r#"
        type Address = { city: String, zip: i32 };
        type Person = { name: String, address: Address };
        type Tagged a = { tag: String, value: a };

        let
            person: Person = {
                name = "Ada",
                address = { city = "London", zip = 101 }
            },
            tagged: Tagged Person = { tag = "author", value = person }
        in
            match tagged.value with {
                case {name, address} -> (tagged.tag, name, address.city, address.zip);
            }
    "#;
    let (_heap, value, ty) = common::eval_source(Builder::with_prelude(()).unwrap(), code)
        .await
        .unwrap();

    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::String),
            Type::builtin(BuiltinTypeId::String),
            Type::builtin(BuiltinTypeId::String),
            Type::builtin(BuiltinTypeId::I32),
        ])
    );
    assert_eq!(
        value,
        Value::Tuple(vec![
            Value::String("author".into()),
            Value::String("Ada".into()),
            Value::String("London".into()),
            Value::I32(101),
        ])
    );
}

#[tokio::test]
async fn record_aliases_are_structural_across_alias_and_anonymous_types() {
    let code = r#"
        type Point = { x: i32, y: i32 };
        type Coordinates = { y: i32, x: i32 };

        let
            sum = \(point: { x: i32, y: i32 }) -> point.x + point.y,
            point: Point = { x = 2, y = 3 },
            moved: Coordinates = { point with { x = 10 } }
        in
            (sum point, sum moved)
    "#;
    let (_heap, value, ty) = common::eval_source(Builder::with_prelude(()).unwrap(), code)
        .await
        .unwrap();

    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32),
        ])
    );
    assert_eq!(value, Value::Tuple(vec![Value::I32(5), Value::I32(13)]));
}

#[tokio::test]
async fn named_records_cross_adt_boundaries_without_extra_wrapping() {
    let code = r#"
        type Packet = Packet Metadata;
        type Metadata = { label: String, count: i32 };

        let
            metadata: Metadata = { label = "samples", count = 4 },
            packet: Packet = Packet metadata
        in
            match packet with {
                case Packet value -> (value.label, value.count);
            }
    "#;
    let (_heap, value, ty) = common::eval_source(Builder::with_prelude(()).unwrap(), code)
        .await
        .unwrap();

    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::String),
            Type::builtin(BuiltinTypeId::I32),
        ])
    );
    assert_eq!(
        value,
        Value::Tuple(vec![Value::String("samples".into()), Value::I32(4)])
    );
}
