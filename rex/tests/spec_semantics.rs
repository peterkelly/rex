mod common;

use rex::{
    ast::Symbol,
    engine::{Builder, CompileOptions, EngineError, Value},
    parser::parse as parse_rex,
    typesystem::{BuiltinTypeId, Type, TypeError, TypeKind},
};

use common::strip_type_span;

#[tokio::test]
async fn spec_char_is_one_unicode_scalar_value() {
    let (_heap, handle, ty) = common::eval_source(Builder::with_prelude(()).unwrap(), "'😀'")
        .await
        .unwrap();

    assert_eq!(ty, Type::builtin(BuiltinTypeId::Char));
    assert_eq!(handle, Value::Char('😀'));
}

#[tokio::test]
async fn spec_char_typeclasses_follow_rust_char_semantics() {
    let (_heap, handle, ty) = common::eval_source(
        Builder::with_prelude(()).unwrap(),
        r#"('a' == 'a', 'a' < 'b', show '😀', let x: Char = default in x)"#,
    )
    .await
    .unwrap();

    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::Bool),
            Type::builtin(BuiltinTypeId::Bool),
            Type::builtin(BuiltinTypeId::String),
            Type::builtin(BuiltinTypeId::Char),
        ])
    );
    assert_eq!(
        handle,
        Value::Tuple(vec![
            Value::Bool(true),
            Value::Bool(true),
            Value::String("😀".to_owned()),
            Value::Char('\0'),
        ])
    );
}

#[tokio::test]
async fn spec_c_style_comments_are_trivia() {
    let (_heap, handle, ty) = common::eval_source(
        Builder::with_prelude(()).unwrap(),
        r#"
        /* Block comments may contain arbitrary text like @#$.
           They are removed before parsing. */
        let x = 1 in
        x + 2 // line comments run to the end of the line
        "#,
    )
    .await
    .unwrap();

    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    match handle.clone() {
        Value::I32(n) => assert_eq!(n, 3),
        _ => panic!("expected i32, got {}", handle.value_type_name()),
    }
}

#[tokio::test]
async fn spec_explicit_type_parameter_fixes_bad_rex() {
    let (_heap, handle, ty) = common::eval_source(
        Builder::with_prelude(()).unwrap(),
        r#"
fn add<z32> : i32 -> z32 -> z32 = \x y -> x + y;

add 3 4
"#,
    )
    .await
    .unwrap();

    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    match handle.clone() {
        Value::I32(n) => assert_eq!(n, 7),
        _ => panic!("expected i32, got {}", handle.value_type_name()),
    }
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
    let err = match common::eval_source(Builder::with_prelude(()).unwrap(), code).await {
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
    let err = match common::eval_source(Builder::with_prelude(()).unwrap(), code).await {
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
instance Pick Bool where {
    pick = true;
}
pick
"#;
    let err = match common::eval_source(Builder::with_prelude(()).unwrap(), code).await {
        Ok(_) => panic!("expected error"),
        Err(e) => e,
    };
    assert!(matches!(err, EngineError::AmbiguousOverload { .. }));
}

#[tokio::test]
async fn spec_defaulting_picks_a_concrete_type_for_numeric_classes() {
    // `zero` has type `a` with an `AdditiveMonoid a` constraint.
    // With no other type hints, the engine defaults the ambiguous type.
    let (_heap, handle, ty) = common::eval_source(Builder::with_prelude(()).unwrap(), "zero")
        .await
        .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::F32));
    assert!(matches!(handle.clone(), Value::F32(_)));
}

#[tokio::test]
async fn spec_defaulting_accepts_satisfied_compound_predicates() {
    let (_heap, value, ty) =
        common::eval_source(Builder::with_prelude(()).unwrap(), "[1, 2, 3] + [4, 5, 6]")
            .await
            .unwrap();

    assert_eq!(ty, Type::list(Type::builtin(BuiltinTypeId::I32)));
    assert_eq!(
        common::list_elements(&value),
        vec![
            Value::I32(1),
            Value::I32(2),
            Value::I32(3),
            Value::I32(4),
            Value::I32(5),
            Value::I32(6),
        ]
    );
}

#[tokio::test]
async fn spec_defaulting_requires_a_simple_numeric_predicate() {
    let program = parse_rex("[] + []").unwrap();
    let compiler = Builder::with_prelude(()).unwrap().build_compiler();
    let (compiled, _evaluator) = compiler
        .compile_program(
            &program,
            CompileOptions::for_module("test.defaulting").unwrap(),
        )
        .await
        .unwrap();
    let ty = compiled.result_type();

    let TypeKind::App(list, element) = ty.as_ref() else {
        panic!("expected a list type, got {ty}");
    };
    assert_eq!(list, &Type::builtin(BuiltinTypeId::List));
    assert!(
        matches!(element.as_ref(), TypeKind::Var(_)),
        "compound predicates alone must not default the element type: {ty}"
    );
}

#[tokio::test]
async fn spec_integer_literals_unify_with_integral_context() {
    let (_heap, handle, ty) =
        common::eval_source(Builder::with_prelude(()).unwrap(), "let x: u64 = 4 in x")
            .await
            .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::U64));
    match handle.clone() {
        Value::U64(n) => assert_eq!(n, 4),
        _ => panic!("expected u64, got {}", handle.value_type_name()),
    }
}

#[tokio::test]
async fn spec_integer_values_widen_when_context_requires_lossless_target() {
    let code = r#"
fn a : i32 -> i32 -> i32 = \x y -> x * y;
fn b : i8 -> i8 = \x -> x + 1;

a 4 (b 5)
"#;
    let (_heap, handle, ty) = common::eval_source(Builder::with_prelude(()).unwrap(), code)
        .await
        .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    match handle.clone() {
        Value::I32(n) => assert_eq!(n, 24),
        _ => panic!("expected i32, got {}", handle.value_type_name()),
    }
}

#[tokio::test]
async fn spec_float_literals_unify_with_float_context() {
    let (_heap, handle, ty) = common::eval_source(
        Builder::with_prelude(()).unwrap(),
        r#"
        let
          add_float: f64 -> f64 -> f64 = \x y -> x + y
        in
          add_float 3.0 4.0
        "#,
    )
    .await
    .unwrap();

    assert_eq!(ty, Type::builtin(BuiltinTypeId::F64));
    match handle.clone() {
        Value::F64(n) => assert!((n - 7.0).abs() < f64::EPSILON),
        _ => panic!("expected f64, got {}", handle.value_type_name()),
    }
}

#[tokio::test]
async fn test_let_tuple_destructuring() {
    let (_heap, handle, ty) = common::eval_source(
        Builder::with_prelude(()).unwrap(),
        "let t = (1, \"Hello\", true), (x, y, z) = t in x",
    )
    .await
    .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    match handle.clone() {
        Value::I32(n) => assert_eq!(n, 1),
        _ => panic!("expected i32, got {}", handle.value_type_name()),
    }
    let (_heap, handle, ty) = common::eval_source(
        Builder::with_prelude(()).unwrap(),
        "let t = (1, \"Hello\", true), (x, y, z) = t in y",
    )
    .await
    .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::String));
    match handle.clone() {
        Value::String(s) => assert_eq!(s, "Hello"),
        _ => panic!("expected String, got {}", handle.value_type_name()),
    }
    let (_heap, handle, ty) = common::eval_source(
        Builder::with_prelude(()).unwrap(),
        "let t = (1, \"Hello\", true), (x, y, z) = t in z",
    )
    .await
    .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::Bool));
    match handle.clone() {
        Value::Bool(b) => assert!(b),
        _ => panic!("expected Bool, got {}", handle.value_type_name()),
    }
}

#[tokio::test]
async fn test_string_literal_escape_sequences() {
    let (_heap, handle, ty) = common::eval_source(
        Builder::with_prelude(()).unwrap(),
        r#""a\nb\r\t\\\"\'\?\a\b\f\v\0\x41\101\u03BB\U0001F600""#,
    )
    .await
    .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::String));
    match handle.clone() {
        Value::String(s) => {
            assert_eq!(s, "a\nb\r\t\\\"'?\x07\x08\x0c\x0b\0AA\u{03BB}\u{1F600}")
        }
        _ => panic!("expected String, got {}", handle.value_type_name()),
    }
}

#[tokio::test]
async fn spec_length_counts_string_unicode_scalar_values() {
    let (_heap, value, ty) = common::eval_source(
        Builder::with_prelude(()).unwrap(),
        r#"[length "", length "rex", length "hé😀", length "é"]"#,
    )
    .await
    .unwrap();

    assert_eq!(ty, Type::list(Type::builtin(BuiltinTypeId::U64)),);
    assert_eq!(
        common::list_elements(&value),
        vec![Value::U64(0), Value::U64(3), Value::U64(3), Value::U64(2)]
    );
}

#[tokio::test]
async fn spec_length_remains_available_for_lists() {
    let (_heap, value, ty) =
        common::eval_source(Builder::with_prelude(()).unwrap(), "length [1, 2, 3]")
            .await
            .unwrap();

    assert_eq!(ty, Type::builtin(BuiltinTypeId::U64));
    assert_eq!(value, Value::U64(3));
}

#[tokio::test]
async fn spec_length_counts_dictionary_entries() {
    let (_heap, value, ty) = common::eval_source(
        Builder::with_prelude(()).unwrap(),
        "[length (({}) is Dict i32), length (({ a = 1, b = 2 }) is Dict i32)]",
    )
    .await
    .unwrap();

    assert_eq!(ty, Type::list(Type::builtin(BuiltinTypeId::U64)));
    assert_eq!(
        common::list_elements(&value),
        vec![Value::U64(0), Value::U64(2)]
    );
}

#[tokio::test]
async fn spec_take_skip_and_list_get_use_u64() {
    let (_heap, value, ty) = common::eval_source(
        Builder::with_prelude(()).unwrap(),
        r#"
        (
            take (2 is u64) (([1, 2, 3]) is List i32),
            skip (1 is u64) (([1, 2, 3]) is List i32),
            list_get (1 is u64) (([1, 2, 3]) is List i32),
            take (18446744073709551615 is u64) (([1, 2, 3]) is List i32),
            skip (18446744073709551615 is u64) (([1, 2, 3]) is List i32)
        )
        "#,
    )
    .await
    .unwrap();

    let i32_ty = Type::builtin(BuiltinTypeId::I32);
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::list(i32_ty.clone()),
            Type::list(i32_ty.clone()),
            Type::option(i32_ty.clone()),
            Type::list(i32_ty.clone()),
            Type::list(i32_ty),
        ])
    );
    assert_eq!(
        value,
        Value::Tuple(vec![
            Value::List(vec![Value::I32(1), Value::I32(2)]),
            Value::List(vec![Value::I32(2), Value::I32(3)]),
            Value::Adt(Symbol::intern("Some"), vec![Value::I32(2)]),
            Value::List(vec![Value::I32(1), Value::I32(2), Value::I32(3)]),
            Value::List(vec![]),
        ])
    );
}

#[tokio::test]
async fn spec_list_get_accepts_full_u64_and_returns_none() {
    let (_heap, value, ty) = common::eval_source(
        Builder::with_prelude(()).unwrap(),
        "list_get (18446744073709551615 is u64) (([1, 2, 3]) is List i32)",
    )
    .await
    .unwrap();

    assert_eq!(ty, Type::option(Type::builtin(BuiltinTypeId::I32)));
    assert_eq!(value, Value::Adt(Symbol::intern("None"), vec![]));
}

#[tokio::test]
async fn spec_length_is_not_implemented_for_options() {
    let err = common::eval_source(Builder::with_prelude(()).unwrap(), "length (Some 1)")
        .await
        .expect_err("Option must not implement Length");

    assert!(matches!(
        err,
        EngineError::MissingTypeclassImpl { class, .. } if class.as_ref() == "Length"
    ));
}

#[tokio::test]
async fn test_match_tuple_destructuring() {
    let (_heap, handle, ty) = common::eval_source(
        Builder::with_prelude(()).unwrap(),
        "let t = (1, \"Hello\", true) in match t with { case (x, y, z) -> x; }",
    )
    .await
    .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    match handle.clone() {
        Value::I32(n) => assert_eq!(n, 1),
        _ => panic!("expected i32, got {}", handle.value_type_name()),
    }
    let (_heap, handle, ty) = common::eval_source(
        Builder::with_prelude(()).unwrap(),
        "let t = (1, \"Hello\", true) in match t with { case (x, y, z) -> y; }",
    )
    .await
    .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::String));
    match handle.clone() {
        Value::String(s) => assert_eq!(s, "Hello"),
        _ => panic!("expected String, got {}", handle.value_type_name()),
    }
    let (_heap, handle, ty) = common::eval_source(
        Builder::with_prelude(()).unwrap(),
        "let t = (1, \"Hello\", true) in match t with { case (x, y, z) -> z; }",
    )
    .await
    .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::Bool));
    match handle.clone() {
        Value::Bool(b) => assert!(b),
        _ => panic!("expected Bool, got {}", handle.value_type_name()),
    }
}

#[tokio::test]
async fn test_tuple_projection() {
    let (_heap, handle, ty) = common::eval_source(
        Builder::with_prelude(()).unwrap(),
        "let t = (4, \"Hello\", true) in t.0",
    )
    .await
    .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    match handle.clone() {
        Value::I32(n) => assert_eq!(n, 4),
        _ => panic!("expected i32, got {}", handle.value_type_name()),
    }
    let (_heap, handle, ty) = common::eval_source(
        Builder::with_prelude(()).unwrap(),
        "let t = (4, \"Hello\", true) in t.1",
    )
    .await
    .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::String));
    match handle.clone() {
        Value::String(s) => assert_eq!(s, "Hello"),
        _ => panic!("expected String, got {}", handle.value_type_name()),
    }
    let (_heap, handle, ty) = common::eval_source(
        Builder::with_prelude(()).unwrap(),
        "let t = (4, \"Hello\", true) in t.2",
    )
    .await
    .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::Bool));
    match handle.clone() {
        Value::Bool(b) => assert!(b),
        _ => panic!("expected Bool, got {}", handle.value_type_name()),
    }
}
