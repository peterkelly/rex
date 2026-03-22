use rex_core::{
    BuiltinTypeId, Engine, EngineError, FromPointer, GasMeter, Heap, JsonOptions, Parser, Pointer,
    Rex, RexAdt, RexType, Token, Type, Value, rex_to_json,
};
use rex_engine::assert_pointer_eq;
use serde::Serialize;
use std::collections::HashMap;

async fn eval(code: &str) -> Result<(Heap, Pointer, Type), EngineError> {
    let tokens = Token::tokenize(code).unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();

    let mut engine = Engine::with_prelude(())?;
    MyInnerStruct::inject_rex(&mut engine)?;
    MyStruct::inject_rex(&mut engine)?;
    Boxed::<i32>::inject_rex(&mut engine)?;
    Maybe::<i32>::inject_rex(&mut engine)?;
    Shape::inject_rex(&mut engine)?;

    engine.inject_decls(&program.decls)?;
    let mut gas = GasMeter::default();
    let (pointer, ty) = engine.eval(program.expr.as_ref(), &mut gas).await?;
    let heap = engine.into_heap();
    Ok((heap, pointer, ty))
}

#[derive(Rex, Debug, PartialEq, Serialize)]
struct MyInnerStruct {
    x: bool,
    y: i32,
}

#[derive(Rex, Debug, PartialEq, Serialize)]
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

#[tokio::test]
async fn derive_struct_roundtrip_value() {
    let (heap, v_ptr, ty) = eval(
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
    .await
    .unwrap();
    assert_eq!(ty, MyStruct::rex_type());

    let decoded = MyStruct::from_pointer(&heap, &v_ptr).unwrap();
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

#[tokio::test]
async fn derive_generic_struct_roundtrip_value() {
    let (heap, v_ptr, ty) = eval("Boxed { value = 123 }").await.unwrap();
    assert_eq!(ty, Boxed::<i32>::rex_type());
    let decoded = Boxed::<i32>::from_pointer(&heap, &v_ptr).unwrap();
    assert_eq!(decoded, Boxed { value: 123 });
}

#[tokio::test]
async fn derive_struct_eval_json_matches_rust_serde_json() {
    let code = r#"
        MyStruct {
            x = true,
            y = 42,
            tags = ["a", "b", "c"],
            props = { a = 1, b = 2 },
            inner = MyInnerStruct { x = false, y = 7 },
            pair = (1, "hi", true),
            renamed = 9
        }
    "#;

    let expected = serde_json::json!({
        "x": true,
        "y": 42,
        "tags": ["a", "b", "c"],
        "props": { "a": 1, "b": 2 },
        "inner": { "x": false, "y": 7 },
        "pair": [1, "hi", true],
        "renamed": 9
    });

    let tokens = Token::tokenize(code).unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();

    let mut engine = Engine::with_prelude(()).unwrap();
    MyInnerStruct::inject_rex(&mut engine).unwrap();
    MyStruct::inject_rex(&mut engine).unwrap();
    engine.inject_decls(&program.decls).unwrap();

    let mut gas = GasMeter::default();
    let (v_ptr, ty) = engine.eval(program.expr.as_ref(), &mut gas).await.unwrap();

    let actual_rex = rex_to_json(
        &engine.heap,
        &v_ptr,
        &ty,
        &engine.type_system,
        &JsonOptions::default(),
    )
    .unwrap();

    let actual_serde = serde_json::to_value(MyStruct {
        x: true,
        y: 42,
        tags: vec!["a".into(), "b".into(), "c".into()],
        props: HashMap::from([("a".into(), 1), ("b".into(), 2)]),
        inner: MyInnerStruct { x: false, y: 7 },
        pair: (1, "hi".into(), true),
        renamed_field: 9,
    })
    .unwrap();

    assert_eq!(actual_rex, expected);
    assert_eq!(actual_serde, expected);
}

#[tokio::test]
async fn derive_generic_worked_example_polymorphic_adt() {
    // Worked example: `Maybe<T>` is injected into Rex once, but constructors stay polymorphic.
    //
    // The proc-macro generates *both*:
    // - `RexType` for Rust values (e.g. `Maybe<i32>` -> `Maybe i32`)
    // - an `AdtDecl` with a type parameter `T` (so `Just` has scheme `a -> Maybe a`)
    let mut engine = Engine::with_prelude(()).unwrap();

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
        .parse_program(&mut GasMeter::default())
        .map_err(|errs| format!("parse error: {errs:?}"))
        .unwrap();

    engine.inject_decls(&program.decls).unwrap();
    let mut gas = GasMeter::default();
    let (v_ptr, ty) = engine.eval(program.expr.as_ref(), &mut gas).await.unwrap();
    let expected_ty = Type::tuple(vec![Maybe::<i32>::rex_type(), Maybe::<bool>::rex_type()]);
    assert_eq!(ty, expected_ty);
    let v = engine
        .heap
        .get(&v_ptr)
        .map(|value| value.as_ref().clone())
        .unwrap();

    let Value::Tuple(items) = v else {
        panic!(
            "expected tuple, got {}",
            engine.heap.type_name(&v_ptr).unwrap()
        );
    };
    assert_eq!(items.len(), 2);
    assert_eq!(
        Maybe::<i32>::from_pointer(&engine.heap, &items[0]).unwrap(),
        Maybe::Just(1)
    );
    assert_eq!(
        Maybe::<bool>::from_pointer(&engine.heap, &items[1]).unwrap(),
        Maybe::Just(true)
    );
}

#[derive(Rex, Debug, PartialEq)]
enum Shape {
    Rectangle(i32, i32),
    Circle(i32),
}

#[tokio::test]
async fn derive_can_be_used_in_injected_native_functions() {
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
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();

    let mut engine = Engine::with_prelude(()).unwrap();
    MyInnerStruct::inject_rex(&mut engine).unwrap();
    MyStruct::inject_rex(&mut engine).unwrap();

    engine
        .export("bump_y", |_: &(), mut s: MyStruct| {
            s.y += 1;
            Ok(s)
        })
        .unwrap();

    let mut gas = GasMeter::default();
    let (v_ptr, ty) = engine.eval(program.expr.as_ref(), &mut gas).await.unwrap();
    assert_eq!(ty, MyStruct::rex_type());
    let bumped = MyStruct::from_pointer(&engine.heap, &v_ptr).unwrap();
    assert_eq!(bumped.y, 43);

    engine
        .export_value(
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
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();
    let (v, ty) = engine.eval(program.expr.as_ref(), &mut gas).await.unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    let heap = &engine.heap;
    assert_pointer_eq!(heap, v, heap.alloc_i32(100).unwrap());
}

#[tokio::test]
async fn derive_enum_can_be_injected_as_value_and_pattern_matched() {
    let mut engine = Engine::with_prelude(()).unwrap();
    Shape::inject_rex(&mut engine).unwrap();

    engine
        .export_value("shape", Shape::Rectangle(3, 4))
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
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();

    let mut gas = GasMeter::default();
    let (v, ty) = engine.eval(program.expr.as_ref(), &mut gas).await.unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    let heap = &engine.heap;
    assert_pointer_eq!(heap, v, heap.alloc_i32(12).unwrap());
}

#[tokio::test]
async fn derive_types_implement_rex_adt_trait() {
    let mut engine = Engine::with_prelude(()).unwrap();
    <Shape as RexAdt>::inject_rex(&mut engine).unwrap();

    let tokens = Token::tokenize(
        r#"
        match (Rectangle 2 5)
            when Rectangle w h -> w * h
            when Circle r -> r
        "#,
    )
    .unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();

    let mut gas = GasMeter::default();
    let (v, ty) = engine.eval(program.expr.as_ref(), &mut gas).await.unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_pointer_eq!(&engine.heap, v, engine.heap.alloc_i32(10).unwrap());
}

#[tokio::test]
async fn derive_generic_enum_can_be_used_as_injected_fn_arg_and_return() {
    let mut engine = Engine::with_prelude(()).unwrap();
    Maybe::<i32>::inject_rex(&mut engine).unwrap();

    engine
        .export("unwrap_or_zero", |_: &(), m: Maybe<i32>| {
            Ok(match m {
                Maybe::Just(v) => v,
                Maybe::Nothing => 0,
            })
        })
        .unwrap();

    let tokens = Token::tokenize("(unwrap_or_zero (Just 5), unwrap_or_zero Nothing)").unwrap();
    let mut parser = Parser::new(tokens);
    let mut gas = GasMeter::default();
    let program = parser.parse_program(&mut gas).unwrap();
    let (v_ptr, ty) = engine.eval(program.expr.as_ref(), &mut gas).await.unwrap();
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32)
        ])
    );
    let v = engine
        .heap
        .get(&v_ptr)
        .map(|value| value.as_ref().clone())
        .unwrap();
    let Value::Tuple(items) = v else {
        panic!(
            "expected tuple, got {}",
            engine.heap.type_name(&v_ptr).unwrap()
        );
    };
    let items = items
        .into_iter()
        .map(|item| {
            engine
                .heap
                .get(&item)
                .map(|value| value.as_ref().clone())
                .unwrap()
        })
        .collect::<Vec<_>>();
    let heap = &engine.heap;
    assert_pointer_eq!(
        heap,
        heap.alloc_value(items[0].clone()).unwrap(),
        heap.alloc_i32(5).unwrap()
    );
    assert_pointer_eq!(
        heap,
        heap.alloc_value(items[1].clone()).unwrap(),
        heap.alloc_i32(0).unwrap()
    );
}

#[tokio::test]
async fn derive_enum_constructor_currying() {
    let (heap, v_ptr, ty) = eval(
        r#"
        let partial = Rectangle (2 * 3) in
            (partial (3 * 4), partial (2 * 4))
        "#,
    )
    .await
    .unwrap();
    assert_eq!(ty, Type::tuple(vec![Shape::rex_type(), Shape::rex_type()]));

    let value = heap.get(&v_ptr).unwrap().as_ref().clone();
    let Value::Tuple(items) = value else {
        panic!("expected tuple, got {}", heap.type_name(&v_ptr).unwrap());
    };
    assert_eq!(items.len(), 2);
    let a = Shape::from_pointer(&heap, &items[0]).unwrap();
    let b = Shape::from_pointer(&heap, &items[1]).unwrap();
    assert_eq!(a, Shape::Rectangle(6, 12));
    assert_eq!(b, Shape::Rectangle(6, 8));
}
