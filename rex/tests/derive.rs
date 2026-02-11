use rex::{Engine, FromValue, Parser, Rex, RexType, Token, Value};
use std::collections::HashMap;

fn eval(code: &str) -> Result<Value, String> {
    let tokens = Token::tokenize(code).map_err(|e| format!("lex error: {e}"))?;
    let mut parser = Parser::new(tokens);
    let program = parser
        .parse_program()
        .map_err(|errs| format!("parse error: {errs:?}"))?;

    let mut engine = Engine::with_prelude().unwrap();
    MyInnerStruct::inject_rex(&mut engine).map_err(|e| format!("{e}"))?;
    MyStruct::inject_rex(&mut engine).map_err(|e| format!("{e}"))?;
    Boxed::<i32>::inject_rex(&mut engine).map_err(|e| format!("{e}"))?;
    Maybe::<i32>::inject_rex(&mut engine).map_err(|e| format!("{e}"))?;
    Shape::inject_rex(&mut engine).map_err(|e| format!("{e}"))?;

    engine
        .inject_decls(&program.decls)
        .map_err(|e| format!("{e}"))?;
    engine
        .eval(program.expr.as_ref())
        .map_err(|e| format!("{e}"))
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
    #[serde(default = "xxx")] // should have no effect
    inner: MyInnerStruct,
    #[serde(alias = "ignore")] // should have no effect
    pair: (i32, String, bool),
    #[serde(rename = "renamed")]
    renamed_field: i32,
}

#[derive(Rex, Debug, PartialEq)]
struct Boxed<T> {
    value: T,
}

#[derive(Rex, Debug, PartialEq)]
enum Maybe<T> {
    Just(T),
    Nothing,
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

#[test]
fn derive_generic_struct_roundtrip_value() {
    let v = eval("Boxed { value = 123 }").unwrap();
    let decoded = Boxed::<i32>::from_value(&v, "boxed").unwrap();
    assert_eq!(decoded, Boxed { value: 123 });
}

#[test]
fn derive_generic_worked_example_polymorphic_adt() {
    // Worked example: `Maybe<T>` is injected into Rex once, but constructors stay polymorphic.
    //
    // The proc-macro generates *both*:
    // - `RexType` for Rust values (e.g. `Maybe<i32>` -> `Maybe i32`)
    // - an `AdtDecl` with a type parameter `T` (so `Just` has scheme `a -> Maybe a`)
    let mut engine = Engine::with_prelude().unwrap();

    // Build the ADT surface (params + variants) and sanity-check that it really uses a type var.
    let adt = Maybe::<i32>::rex_adt_decl(&mut engine).unwrap();
    assert_eq!(adt.name.as_ref(), "Maybe");
    assert_eq!(adt.params.len(), 1);

    let t = adt
        .param_type(&rex_ast::expr::intern("T"))
        .expect("expected `T` param type");

    let just = adt
        .variants
        .iter()
        .find(|v| v.name.as_ref() == "Just")
        .expect("expected `Just` variant");
    assert_eq!(just.args, vec![t.clone()]);

    let nothing = adt
        .variants
        .iter()
        .find(|v| v.name.as_ref() == "Nothing")
        .expect("expected `Nothing` variant");
    assert!(nothing.args.is_empty());

    // Inject the ADT once: constructor *schemes* are registered in the type system, and runtime
    // constructor *functions* are registered in the evaluator.
    engine.inject_adt(adt).unwrap();

    // On the Rust side, `RexType` is the nominal head applied to the Rust generic arguments.
    assert_eq!(
        Maybe::<i32>::rex_type(),
        rex_ts::Type::app(rex_ts::Type::con("Maybe", 1), <i32 as RexType>::rex_type())
    );
    assert_eq!(
        Maybe::<bool>::rex_type(),
        rex_ts::Type::app(rex_ts::Type::con("Maybe", 1), <bool as RexType>::rex_type())
    );

    // On the Rex side, `Just` stays polymorphic because the injected `AdtDecl` used a type var `T`
    // in the argument type. That lets the same constructor be used at multiple instantiations.
    let tokens = Token::tokenize(
        r#"
        let id = \x -> Just x in
            (id 1, id true)
        "#,
    )
    .map_err(|e| format!("lex error: {e}"))
    .unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser
        .parse_program()
        .map_err(|errs| format!("parse error: {errs:?}"))
        .unwrap();

    engine.inject_decls(&program.decls).unwrap();
    let v = engine.eval(program.expr.as_ref()).unwrap();

    let Value::Tuple(items) = v else {
        panic!("expected tuple, got {v}");
    };
    assert_eq!(items.len(), 2);
    assert_eq!(
        Maybe::<i32>::from_value(&items[0], "a").unwrap(),
        Maybe::Just(1)
    );
    assert_eq!(
        Maybe::<bool>::from_value(&items[1], "b").unwrap(),
        Maybe::Just(true)
    );
}

#[derive(Rex, Debug, PartialEq)]
enum Shape {
    Rectangle(i32, i32),
    Circle(i32),
}

#[test]
fn derive_can_be_used_in_injected_native_functions() {
    let tokens = Token::tokenize(
        r#"
        bump_y (MyStruct {
            x = true,
            y = 42,
            tags = ["a", "b", "c"],
            props = { a = 1, b = 2 },
            inner = MyInnerStruct { x = false, y = 7 },
            pair = (1, "hi", true),
            renamed = 9
        })
        "#,
    )
    .unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();

    let mut engine = Engine::with_prelude().unwrap();
    MyInnerStruct::inject_rex(&mut engine).unwrap();
    MyStruct::inject_rex(&mut engine).unwrap();

    engine
        .inject_fn1("bump_y", |mut s: MyStruct| {
            s.y += 1;
            s
        })
        .unwrap();

    let v = engine.eval(program.expr.as_ref()).unwrap();
    let bumped = MyStruct::from_value(&v, "bumped").unwrap();
    assert_eq!(bumped.y, 43);

    engine
        .inject_value(
            "const_struct",
            MyStruct {
                x: false,
                y: 100,
                tags: vec![],
                props: HashMap::new(),
                inner: MyInnerStruct { x: true, y: 1 },
                pair: (2, "ok".into(), false),
                renamed_field: 0,
            },
        )
        .unwrap();
    let tokens = Token::tokenize("const_struct.y").unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let v = engine.eval(program.expr.as_ref()).unwrap();
    assert_eq!(v, Value::I32(100));
}

#[test]
fn derive_enum_can_be_injected_as_value_and_pattern_matched() {
    let mut engine = Engine::with_prelude().unwrap();
    Shape::inject_rex(&mut engine).unwrap();

    engine
        .inject_value("shape", Shape::Rectangle(3, 4))
        .unwrap();

    let tokens = Token::tokenize(
        r#"
        match shape
            when Rectangle w h -> w * h
            when Circle r -> r
        "#,
    )
    .unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();

    let v = engine.eval(program.expr.as_ref()).unwrap();
    assert_eq!(v, Value::I32(12));
}

#[test]
fn derive_generic_enum_can_be_used_as_injected_fn_arg_and_return() {
    let mut engine = Engine::with_prelude().unwrap();
    Maybe::<i32>::inject_rex(&mut engine).unwrap();

    engine
        .inject_fn1("unwrap_or_zero", |m: Maybe<i32>| match m {
            Maybe::Just(v) => v,
            Maybe::Nothing => 0,
        })
        .unwrap();

    let tokens = Token::tokenize("(unwrap_or_zero (Just 5), unwrap_or_zero Nothing)").unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let v = engine.eval(program.expr.as_ref()).unwrap();
    let Value::Tuple(items) = v else {
        panic!("expected tuple, got {v}");
    };
    assert_eq!(items[0], Value::I32(5));
    assert_eq!(items[1], Value::I32(0));
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

    let Value::Tuple(items) = v else {
        panic!("expected tuple, got {v}");
    };
    assert_eq!(items.len(), 2);
    let a = Shape::from_value(&items[0], "a").unwrap();
    let b = Shape::from_value(&items[1], "b").unwrap();
    assert_eq!(a, Shape::Rectangle(6, 12));
    assert_eq!(b, Shape::Rectangle(6, 8));
}
