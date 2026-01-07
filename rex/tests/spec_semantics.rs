use rex_engine::{Engine, EngineError, Value};
use rex_lexer::Token;
use rex_parser::Parser;
use rex_ts::TypeError;

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
    let mut engine = Engine::with_prelude();
    engine.inject_decls(&program.decls)?;
    engine.eval(program.expr.as_ref())
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
