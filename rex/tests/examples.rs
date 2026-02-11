use rex::{Engine, Parser, Token, TypeSystem};

fn format_parse_errors(errs: &[rex_parser::error::ParserErr]) -> String {
    let mut out = String::from("parse error:");
    for err in errs {
        out.push_str(&format!("\n  {err}"));
    }
    out
}

fn assert_program_ok(name: &str, source: &str) {
    let tokens = Token::tokenize(source).unwrap_or_else(|err| panic!("{name}: lex error: {err}"));
    let mut parser = Parser::new(tokens);
    let program = parser
        .parse_program()
        .unwrap_or_else(|errs| panic!("{name}:\n{}", format_parse_errors(&errs)));

    let mut ts = TypeSystem::with_prelude().unwrap();
    ts.inject_decls(&program.decls)
        .unwrap_or_else(|err| panic!("{name}: decl error: {err}"));
    ts.infer(program.expr.as_ref())
        .unwrap_or_else(|err| panic!("{name}: type error: {err}"));

    let mut engine = Engine::with_prelude().unwrap();
    engine
        .inject_decls(&program.decls)
        .unwrap_or_else(|err| panic!("{name}: engine decl error: {err}"));
    let _value = engine
        .eval(program.expr.as_ref())
        .unwrap_or_else(|err| panic!("{name}: eval error: {err}"));
}

#[test]
fn example_adt_record_constructor() {
    assert_program_ok(
        "adt_record_constructor",
        r#"
            type Foo = Bar { x: i32, y: i32 }

            let v: Foo = Bar { x = 1, y = 2 } in
              v.x + v.y
        "#,
    );
}

#[test]
fn example_nested_lets() {
    assert_program_ok(
        "nested_lets",
        r#"
            let
              a = 1,
              b = 2,
              c = a + b
            in c
        "#,
    );
}

#[test]
fn example_lambda_application() {
    assert_program_ok(
        "lambda_application",
        r#"
            let inc = \x -> x + 1 in
              inc 41
        "#,
    );
}

#[test]
fn example_match() {
    assert_program_ok(
        "match",
        r#"
            type Sum = A { x: i32 } | B { x: i32 }

            let v: Sum = A { x = 7 } in
              match v
                when A {x} -> x
                when B {x} -> x + 100
        "#,
    );
}
