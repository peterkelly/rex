mod common;

use rex::{
    engine::{Builder, Value},
    parser::parse as parse_rex,
    typesystem::{BuiltinTypeId, Type},
};

async fn assert_program_ok(name: &str, source: &str, expected_value: i32, expected_type: Type) {
    parse_rex(source).unwrap_or_else(|errs| {
        let mut out = String::from("parse error:");
        for err in errs {
            out.push_str(&format!("\n  {err}"));
        }
        panic!("{name}:\n{out}");
    });
    let (_heap, value, ty) = common::eval_source(Builder::with_prelude(()).unwrap(), source)
        .await
        .unwrap_or_else(|err| panic!("{name}: eval error: {err}"));
    assert_eq!(ty, expected_type, "{name}: unexpected eval type");

    match value {
        Value::I32(actual) => {
            assert_eq!(actual, expected_value, "{name}: unexpected eval value")
        }
        _ => panic!("{name}: expected i32 result"),
    }
}

#[tokio::test]
async fn example_adt_record_constructor() {
    assert_program_ok(
        "adt_record_constructor",
        r#"
            type Foo = Bar { x: i32, y: i32 };

            let v: Foo = Bar { x = 1, y = 2 } in
              v.x + v.y
        "#,
        3,
        Type::builtin(BuiltinTypeId::I32),
    )
    .await;
}

#[tokio::test]
async fn example_nested_lets() {
    assert_program_ok(
        "nested_lets",
        r#"
            let
              a = 1,
              b = 2,
              c = a + b
            in c
        "#,
        3,
        Type::builtin(BuiltinTypeId::I32),
    )
    .await;
}

#[tokio::test]
async fn example_lambda_application() {
    assert_program_ok(
        "lambda_application",
        r#"
            let inc = \x -> x + 1 in
              inc 41
        "#,
        42,
        Type::builtin(BuiltinTypeId::I32),
    )
    .await;
}

#[tokio::test]
async fn example_match() {
    assert_program_ok(
        "match",
        r#"
            type Sum = A { x: i32 } | B { x: i32 };

            let v: Sum = A { x = 7 } in
              match v with {
                case A {x} -> x;
                case B {x} -> x + 100;
              }
        "#,
        7,
        Type::builtin(BuiltinTypeId::I32),
    )
    .await;
}
