use rex::{
    engine::{Engine, EngineError, Handle, Heap, Module, Value},
    parser::parse as parse_rex,
    typesystem::{BuiltinTypeId, Type, TypeError},
};

fn strip_type_span(mut err: TypeError) -> TypeError {
    while let TypeError::Spanned { error, .. } = err {
        err = *error;
    }
    err
}

async fn eval(code: &str) -> Result<(Heap, Handle, Type), EngineError> {
    let program = parse_rex(code).unwrap();
    let mut engine = Engine::with_prelude(()).unwrap();
    let mut module = Module::global();
    module.add_decls(program.decls.clone());
    engine.inject_module(module)?;
    let heap = engine.heap.clone();
    let (handle, ty) = engine
        .into_evaluator()
        .eval(program.body.as_ref().unwrap().as_ref())
        .await
        .map_err(|err| err.into_engine_error())?;
    Ok((heap, handle, ty))
}

#[tokio::test]
async fn spec_record_update_requires_refinement_for_sum_types() {
    let code = r#"
type Foo = Bar { x: i32 } | Baz { x: i32 };
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
class C a where {
    c : i32;
}
instance C i32 where {
    c = 0;
}
instance C i32 where {
    c = 1;
}
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
class Pick a where {
    pick : a;
}
instance Pick i32 where {
    pick = 0;
}
instance Pick bool where {
    pick = true;
}
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
    let (_heap, handle, ty) = eval("zero").await.unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::F32));
    assert!(matches!(handle.value().unwrap(), Value::F32(_)));
}

#[tokio::test]
async fn spec_integer_literals_unify_with_integral_context() {
    let (_heap, handle, ty) = eval("let x: u64 = 4 in x").await.unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::U64));
    match handle.value().unwrap() {
        Value::U64(n) => assert_eq!(n, 4),
        _ => panic!("expected u64, got {}", handle.type_name().unwrap()),
    }
}

#[tokio::test]
async fn test_let_tuple_destructuring() {
    let (_heap, handle, ty) = eval("let t = (1, \"Hello\", true), (x, y, z) = t in x")
        .await
        .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    match handle.value().unwrap() {
        Value::I32(n) => assert_eq!(n, 1),
        _ => panic!("expected i32, got {}", handle.type_name().unwrap()),
    }
    let (_heap, handle, ty) = eval("let t = (1, \"Hello\", true), (x, y, z) = t in y")
        .await
        .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::String));
    match handle.value().unwrap() {
        Value::String(s) => assert_eq!(s, "Hello"),
        _ => panic!("expected string, got {}", handle.type_name().unwrap()),
    }
    let (_heap, handle, ty) = eval("let t = (1, \"Hello\", true), (x, y, z) = t in z")
        .await
        .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::Bool));
    match handle.value().unwrap() {
        Value::Bool(b) => assert!(b),
        _ => panic!("expected bool, got {}", handle.type_name().unwrap()),
    }
}

#[tokio::test]
async fn test_match_tuple_destructuring() {
    let (_heap, handle, ty) =
        eval("let t = (1, \"Hello\", true) in match t with { case (x, y, z) -> x; }")
            .await
            .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    match handle.value().unwrap() {
        Value::I32(n) => assert_eq!(n, 1),
        _ => panic!("expected i32, got {}", handle.type_name().unwrap()),
    }
    let (_heap, handle, ty) =
        eval("let t = (1, \"Hello\", true) in match t with { case (x, y, z) -> y; }")
            .await
            .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::String));
    match handle.value().unwrap() {
        Value::String(s) => assert_eq!(s, "Hello"),
        _ => panic!("expected string, got {}", handle.type_name().unwrap()),
    }
    let (_heap, handle, ty) =
        eval("let t = (1, \"Hello\", true) in match t with { case (x, y, z) -> z; }")
            .await
            .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::Bool));
    match handle.value().unwrap() {
        Value::Bool(b) => assert!(b),
        _ => panic!("expected bool, got {}", handle.type_name().unwrap()),
    }
}

#[tokio::test]
async fn test_tuple_projection() {
    let (_heap, handle, ty) = eval("let t = (4, \"Hello\", true) in t.0").await.unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    match handle.value().unwrap() {
        Value::I32(n) => assert_eq!(n, 4),
        _ => panic!("expected i32, got {}", handle.type_name().unwrap()),
    }
    let (_heap, handle, ty) = eval("let t = (4, \"Hello\", true) in t.1").await.unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::String));
    match handle.value().unwrap() {
        Value::String(s) => assert_eq!(s, "Hello"),
        _ => panic!("expected string, got {}", handle.type_name().unwrap()),
    }
    let (_heap, handle, ty) = eval("let t = (4, \"Hello\", true) in t.2").await.unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::Bool));
    match handle.value().unwrap() {
        Value::Bool(b) => assert!(b),
        _ => panic!("expected bool, got {}", handle.type_name().unwrap()),
    }
}
