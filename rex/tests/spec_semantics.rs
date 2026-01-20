use rex_engine::{Engine, EngineError, Value};
use rex_lexer::Token;
use rex_parser::Parser;
use rex_ts::{Type, TypeError, TypeSystem};

fn strip_type_span(mut err: TypeError) -> TypeError {
    while let TypeError::Spanned { error, .. } = err {
        err = *error;
    }
    err
}

fn eval(code: &str) -> Result<Value, EngineError> {
    let tokens = Token::tokenize(code).unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let mut engine = Engine::with_prelude().unwrap();
    engine.inject_decls(&program.decls)?;
    engine.eval(program.expr.as_ref())
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

#[test]
fn spec_record_update_requires_refinement_for_sum_types() {
    let code = r#"
type Foo = Bar { x: i32 } | Baz { x: i32 }
let
  f = \ (foo : Foo) -> { foo with { x = 2 } }
in
  f (Bar { x = 1 })
"#;
    let err = match eval(code) {
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

#[test]
fn spec_typeclass_instance_overlap_is_rejected() {
    let code = r#"
class C a
    c : i32

instance C i32
    c = 0

instance C i32
    c = 1

c
"#;
    let err = match eval(code) {
        Ok(_) => panic!("expected error"),
        Err(e) => e,
    };
    assert!(matches!(err, EngineError::DuplicateTypeclassImpl { .. }));
}

#[test]
fn spec_typeclass_method_value_without_type_is_ambiguous() {
    let code = r#"
class Pick a
    pick : a

instance Pick i32
    pick = 0

instance Pick bool
    pick = true

pick
"#;
    let err = match eval(code) {
        Ok(_) => panic!("expected error"),
        Err(e) => e,
    };
    assert!(matches!(err, EngineError::AmbiguousOverload { .. }));
}

#[test]
fn spec_defaulting_picks_a_concrete_type_for_numeric_classes() {
    // `zero` has type `a` with an `AdditiveMonoid a` constraint.
    // With no other type hints, the engine defaults the ambiguous type.
    let v = eval("zero").unwrap();
    assert!(matches!(v, Value::F32(_)));
}

#[test]
fn test_let_tuple_destructuring() {
    let v = eval("let t = (1, \"Hello\", true), (x, y, z) = t in x").unwrap();
    match v {
        Value::I32(n) => assert_eq!(n, 1),
        other => panic!("expected i32, got {other}"),
    }
    let ty = type_of("let t = (1, \"Hello\", true), (x, y, z) = t in x").unwrap();
    assert_eq!(ty, Type::con("i32", 0));

    let v = eval("let t = (1, \"Hello\", true), (x, y, z) = t in y").unwrap();
    match v {
        Value::String(s) => assert_eq!(s, "Hello"),
        other => panic!("expected string, got {other}"),
    }
    let ty = type_of("let t = (1, \"Hello\", true), (x, y, z) = t in y").unwrap();
    assert_eq!(ty, Type::con("string", 0));

    let v = eval("let t = (1, \"Hello\", true), (x, y, z) = t in z").unwrap();
    match v {
        Value::Bool(b) => assert!(b),
        other => panic!("expected bool, got {other}"),
    }
    let ty = type_of("let t = (1, \"Hello\", true), (x, y, z) = t in z").unwrap();
    assert_eq!(ty, Type::con("bool", 0));
}

#[test]
fn test_match_tuple_destructuring() {
    let v = eval("let t = (1, \"Hello\", true) in match t when (x, y, z) -> x").unwrap();
    match v {
        Value::I32(n) => assert_eq!(n, 1),
        other => panic!("expected i32, got {other}"),
    }
    let ty = type_of("let t = (1, \"Hello\", true) in match t when (x, y, z) -> x").unwrap();
    assert_eq!(ty, Type::con("i32", 0));

    let v = eval("let t = (1, \"Hello\", true) in match t when (x, y, z) -> y").unwrap();
    match v {
        Value::String(s) => assert_eq!(s, "Hello"),
        other => panic!("expected string, got {other}"),
    }
    let ty = type_of("let t = (1, \"Hello\", true) in match t when (x, y, z) -> y").unwrap();
    assert_eq!(ty, Type::con("string", 0));

    let v = eval("let t = (1, \"Hello\", true) in match t when (x, y, z) -> z").unwrap();
    match v {
        Value::Bool(b) => assert!(b),
        other => panic!("expected bool, got {other}"),
    }
    let ty = type_of("let t = (1, \"Hello\", true) in match t when (x, y, z) -> z").unwrap();
    assert_eq!(ty, Type::con("bool", 0));
}
