use std::collections::HashMap;

use rex_engine::{Engine, FromValue};
use rex_lexer::Token;
use rex_parser::Parser;
use rex_proc_macro::Rex;

fn eval(code: &str) -> Result<rex_engine::Value, String> {
    let tokens = Token::tokenize(code).map_err(|e| format!("lex error: {e}"))?;
    let mut parser = Parser::new(tokens);
    let program = parser
        .parse_program()
        .map_err(|errs| format!("parse error: {errs:?}"))?;

    let mut engine = Engine::with_prelude();
    MyInnerStruct::inject_rex(&mut engine).map_err(|e| format!("{e}"))?;
    MyStruct::inject_rex(&mut engine).map_err(|e| format!("{e}"))?;
    Shape::inject_rex(&mut engine).map_err(|e| format!("{e}"))?;

    engine
        .inject_decls(&program.decls)
        .map_err(|e| format!("{e}"))?;
    engine.eval(program.expr.as_ref()).map_err(|e| format!("{e}"))
}

#[derive(Rex, Debug, PartialEq)]
struct MyInnerStruct {
    x: bool,
    y: i32,
}

#[derive(Rex, Debug, PartialEq)]
struct MyStruct {
    x: bool,
    y: i32,
    tags: Vec<String>,
    props: HashMap<String, i32>,
    inner: MyInnerStruct,
    pair: (i32, String, bool),
    #[serde(rename = "renamed")]
    renamed_field: i32,
}

#[test]
fn derive_struct_roundtrip_value() {
    let v = eval(
        r#"
        MyStruct {
            x = true,
            y = 42,
            tags = ["a", "b", "c"],
            props = { a = 1, b = 2 },
            inner = MyInnerStruct { x = false, y = 7 },
            pair = (1, "hi", true),
            renamed = 9
        }
        "#,
    )
    .unwrap();

    let decoded = MyStruct::from_value(&v, "test").unwrap();
    assert_eq!(
        decoded,
        MyStruct {
            x: true,
            y: 42,
            tags: vec!["a".into(), "b".into(), "c".into()],
            props: HashMap::from([("a".into(), 1), ("b".into(), 2)]),
            inner: MyInnerStruct { x: false, y: 7 },
            pair: (1, "hi".into(), true),
            renamed_field: 9,
        }
    );
}

#[derive(Rex, Debug, PartialEq)]
enum Shape {
    Rectangle(i32, i32),
    Circle(i32),
}

#[test]
fn derive_enum_constructor_currying() {
    let v = eval(
        r#"
        let partial = Rectangle (2 * 3) in
            (partial (3 * 4), partial (2 * 4))
        "#,
    )
    .unwrap();

    let rex_engine::Value::Tuple(items) = v else {
        panic!("expected tuple, got {v}");
    };
    assert_eq!(items.len(), 2);
    let a = Shape::from_value(&items[0], "a").unwrap();
    let b = Shape::from_value(&items[1], "b").unwrap();
    assert_eq!(a, Shape::Rectangle(6, 12));
    assert_eq!(b, Shape::Rectangle(6, 8));
}

