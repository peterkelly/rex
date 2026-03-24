use rexlang_core::{BuiltinTypeId, Engine, GasMeter, Parser, Token, Type, Value};
use rexlang_parser::error::ParserErr;

fn format_parse_errors(errs: &[ParserErr]) -> String {
    let mut out = String::from("parse error:");
    for err in errs {
        out.push_str(&format!("\n  {err}"));
    }
    out
}

async fn assert_program_ok(name: &str, source: &str, expected_value: i32, expected_type: Type) {
    let tokens = Token::tokenize(source).unwrap_or_else(|err| panic!("{name}: lex error: {err}"));
    let mut parser = Parser::new(tokens);
    let program = parser
        .parse_program(&mut GasMeter::default())
        .unwrap_or_else(|errs| panic!("{name}:\n{}", format_parse_errors(&errs)));

    let mut engine = Engine::with_prelude(()).unwrap();
    engine
        .inject_decls(&program.decls)
        .unwrap_or_else(|err| panic!("{name}: engine decl error: {err}"));
    let mut gas = GasMeter::default();
    let (value, ty) = engine
        .eval(program.expr.as_ref(), &mut gas)
        .await
        .unwrap_or_else(|err| panic!("{name}: eval error: {err}"));
    assert_eq!(ty, expected_type, "{name}: unexpected eval type");

    let value = engine
        .heap
        .get(&value)
        .map(|value| value.as_ref().clone())
        .unwrap_or_else(|err| panic!("{name}: heap read error: {err}"));
    match value {
        Value::I32(actual) => assert_eq!(actual, expected_value, "{name}: unexpected eval value"),
        _ => panic!("{name}: expected i32 result"),
    }
}

#[tokio::test]
async fn example_adt_record_constructor() {
    assert_program_ok(
        "adt_record_constructor",
        r#"
            type Foo = Bar { x: i32, y: i32 }

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
            type Sum = A { x: i32 } | B { x: i32 }

            let v: Sum = A { x = 7 } in
              match v
                when A {x} -> x
                when B {x} -> x + 100
        "#,
        7,
        Type::builtin(BuiltinTypeId::I32),
    )
    .await;
}
