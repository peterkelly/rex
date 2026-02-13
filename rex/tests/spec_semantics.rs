use rex::{
    Engine, EngineError, GasCosts, GasMeter, Heap, Parser, Pointer, Token, Type, TypeError,
    TypeSystem, Value,
};

fn strip_type_span(mut err: TypeError) -> TypeError {
    while let TypeError::Spanned { error, .. } = err {
        err = *error;
    }
    err
}

async fn eval(code: &str) -> Result<(Heap, Pointer), EngineError> {
    let tokens = Token::tokenize(code).unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut engine = Engine::with_prelude(()).unwrap();
    engine.inject_decls(&program.decls)?;
    let mut gas = GasMeter::unlimited(GasCosts::sensible_defaults());
    let pointer = engine.eval(program.expr.as_ref(), &mut gas).await?;
    let heap = engine.into_heap();
    Ok((heap, pointer))
}

fn type_of(code: &str) -> Result<Type, TypeError> {
    let tokens = Token::tokenize(code).unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut ts = TypeSystem::with_prelude().unwrap();
    ts.inject_decls(&program.decls)?;
    let (_preds, ty) = ts.infer(program.expr.as_ref())?;
    Ok(ty)
}

#[tokio::test]
async fn spec_record_update_requires_refinement_for_sum_types() {
    let code = r#"
type Foo = Bar { x: i32 } | Baz { x: i32 }
let
  f = \ (foo : Foo) -> { foo with { x = 2 } }
in
  f (Bar { x = 1 })
"#;
    let err = match eval(code).await {
        Ok(_) => panic!("expected error"),
        Err(e) => e,
    };
    let EngineError::Type(te) = err else {
        panic!("expected type error, got {err}");
    };
    assert!(matches!(
        strip_type_span(te),
        TypeError::FieldNotKnown { .. }
    ));
}

#[tokio::test]
async fn spec_typeclass_instance_overlap_is_rejected() {
    let code = r#"
class C a
    c : i32

instance C i32
    c = 0

instance C i32
    c = 1

c
"#;
    let err = match eval(code).await {
        Ok(_) => panic!("expected error"),
        Err(e) => e,
    };
    assert!(matches!(err, EngineError::DuplicateTypeclassImpl { .. }));
}

#[tokio::test]
async fn spec_typeclass_method_value_without_type_is_ambiguous() {
    let code = r#"
class Pick a
    pick : a

instance Pick i32
    pick = 0

instance Pick bool
    pick = true

pick
"#;
    let err = match eval(code).await {
        Ok(_) => panic!("expected error"),
        Err(e) => e,
    };
    assert!(matches!(err, EngineError::AmbiguousOverload { .. }));
}

#[tokio::test]
async fn spec_defaulting_picks_a_concrete_type_for_numeric_classes() {
    // `zero` has type `a` with an `AdditiveMonoid a` constraint.
    // With no other type hints, the engine defaults the ambiguous type.
    let (heap, pointer) = eval("zero").await.unwrap();
    let value = heap.get(&pointer).unwrap();
    assert!(matches!(value.as_ref(), Value::F32(_)));
}

#[tokio::test]
async fn test_let_tuple_destructuring() {
    let (heap, pointer) = eval("let t = (1, \"Hello\", true), (x, y, z) = t in x")
        .await
        .unwrap();
    match heap.get(&pointer).unwrap().as_ref() {
        Value::I32(n) => assert_eq!(*n, 1),
        _ => panic!("expected i32, got {}", heap.type_name(&pointer).unwrap()),
    }
    let ty = type_of("let t = (1, \"Hello\", true), (x, y, z) = t in x").unwrap();
    assert_eq!(ty, Type::con("i32", 0));

    let (heap, pointer) = eval("let t = (1, \"Hello\", true), (x, y, z) = t in y")
        .await
        .unwrap();
    match heap.get(&pointer).unwrap().as_ref() {
        Value::String(s) => assert_eq!(s, "Hello"),
        _ => panic!("expected string, got {}", heap.type_name(&pointer).unwrap()),
    }
    let ty = type_of("let t = (1, \"Hello\", true), (x, y, z) = t in y").unwrap();
    assert_eq!(ty, Type::con("string", 0));

    let (heap, pointer) = eval("let t = (1, \"Hello\", true), (x, y, z) = t in z")
        .await
        .unwrap();
    match heap.get(&pointer).unwrap().as_ref() {
        Value::Bool(b) => assert!(*b),
        _ => panic!("expected bool, got {}", heap.type_name(&pointer).unwrap()),
    }
    let ty = type_of("let t = (1, \"Hello\", true), (x, y, z) = t in z").unwrap();
    assert_eq!(ty, Type::con("bool", 0));
}

#[tokio::test]
async fn test_match_tuple_destructuring() {
    let (heap, pointer) = eval("let t = (1, \"Hello\", true) in match t when (x, y, z) -> x")
        .await
        .unwrap();
    match heap.get(&pointer).unwrap().as_ref() {
        Value::I32(n) => assert_eq!(*n, 1),
        _ => panic!("expected i32, got {}", heap.type_name(&pointer).unwrap()),
    }
    let ty = type_of("let t = (1, \"Hello\", true) in match t when (x, y, z) -> x").unwrap();
    assert_eq!(ty, Type::con("i32", 0));

    let (heap, pointer) = eval("let t = (1, \"Hello\", true) in match t when (x, y, z) -> y")
        .await
        .unwrap();
    match heap.get(&pointer).unwrap().as_ref() {
        Value::String(s) => assert_eq!(s, "Hello"),
        _ => panic!("expected string, got {}", heap.type_name(&pointer).unwrap()),
    }
    let ty = type_of("let t = (1, \"Hello\", true) in match t when (x, y, z) -> y").unwrap();
    assert_eq!(ty, Type::con("string", 0));

    let (heap, pointer) = eval("let t = (1, \"Hello\", true) in match t when (x, y, z) -> z")
        .await
        .unwrap();
    match heap.get(&pointer).unwrap().as_ref() {
        Value::Bool(b) => assert!(*b),
        _ => panic!("expected bool, got {}", heap.type_name(&pointer).unwrap()),
    }
    let ty = type_of("let t = (1, \"Hello\", true) in match t when (x, y, z) -> z").unwrap();
    assert_eq!(ty, Type::con("bool", 0));
}

#[tokio::test]
async fn test_tuple_projection() {
    let (heap, pointer) = eval("let t = (4, \"Hello\", true) in t.0").await.unwrap();
    match heap.get(&pointer).unwrap().as_ref() {
        Value::I32(n) => assert_eq!(*n, 4),
        _ => panic!("expected i32, got {}", heap.type_name(&pointer).unwrap()),
    }
    let ty = type_of("let t = (4, \"Hello\", true) in t.0").unwrap();
    assert_eq!(ty, Type::con("i32", 0));

    let (heap, pointer) = eval("let t = (4, \"Hello\", true) in t.1").await.unwrap();
    match heap.get(&pointer).unwrap().as_ref() {
        Value::String(s) => assert_eq!(s, "Hello"),
        _ => panic!("expected string, got {}", heap.type_name(&pointer).unwrap()),
    }
    let ty = type_of("let t = (4, \"Hello\", true) in t.1").unwrap();
    assert_eq!(ty, Type::con("string", 0));

    let (heap, pointer) = eval("let t = (4, \"Hello\", true) in t.2").await.unwrap();
    match heap.get(&pointer).unwrap().as_ref() {
        Value::Bool(b) => assert!(*b),
        _ => panic!("expected bool, got {}", heap.type_name(&pointer).unwrap()),
    }
    let ty = type_of("let t = (4, \"Hello\", true) in t.2").unwrap();
    assert_eq!(ty, Type::con("bool", 0));
}
