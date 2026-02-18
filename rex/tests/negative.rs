use rex::{Engine, EngineError, GasMeter, Parser, ParserLimits, Token, TypeError};

fn strip_span(mut err: TypeError) -> TypeError {
    while let TypeError::Spanned { error, .. } = err {
        err = *error;
    }
    err
}

fn parse_program(code: &str) -> Result<rex_ast::expr::Program, Vec<rex_parser::error::ParserErr>> {
    let tokens = Token::tokenize(code).expect("lexer should not panic");
    let mut parser = Parser::new(tokens);
    parser.set_limits(ParserLimits::safe_defaults());
    parser.parse_program(&mut GasMeter::default())
}

async fn compile_err(code: &str) -> EngineError {
    let program = parse_program(code).unwrap_or_else(|errs| {
        panic!("expected parse success, got: {errs:?}\ncode:\n{code}");
    });

    let mut engine = Engine::with_prelude(()).unwrap();
    if let Err(e) = engine.inject_decls(&program.decls) {
        return e;
    }
    let mut gas = GasMeter::default();
    match engine.eval(program.expr.as_ref(), &mut gas).await {
        Ok((v, _)) => {
            let value_type = engine.heap.type_name(&v).unwrap_or("<invalid pointer>");
            panic!("expected error, got value type: {value_type}\ncode:\n{code}");
        }
        Err(e) => e,
    }
}

async fn expect_type_err(code: &str, f: impl FnOnce(&TypeError) -> bool) {
    let err = compile_err(code).await;
    let EngineError::Type(te) = err else {
        panic!("expected type error, got: {err:?}\ncode:\n{code}");
    };
    let te = strip_span(te);
    assert!(f(&te), "unexpected type error: {te:?}\ncode:\n{code}");
}

async fn expect_engine_err(code: &str, f: impl FnOnce(&EngineError) -> bool) {
    let err = compile_err(code).await;
    assert!(f(&err), "unexpected engine error: {err:?}\ncode:\n{code}");
}

#[tokio::test]
async fn parse_rejects_invalid_programs() {
    let cases: &[(&str, &str)] = &[
        ("unterminated_paren", "("),
        ("orphan_close_paren", ")"),
        ("unterminated_bracket", "[1, 2"),
        ("orphan_close_bracket", "]"),
        ("unterminated_brace", "{ x = 1"),
        ("orphan_close_brace", "}"),
        ("bad_if", "if true then 1"),
        ("bad_let", "let x = 1 in"),
        ("bad_match", "match 1 when -> 2"),
        ("bad_type_decl", "type Foo ="),
        ("bad_fn_decl", "fn inc (x: i32) -> i32 ="),
        ("bad_record_update", "{ 1 with }"),
        ("bad_projection", "1."),
        ("bad_dict", "{ = 1 }"),
        ("bad_list", "[,]"),
        ("bad_tuple", "(,1)"),
    ];

    for (name, code) in cases {
        let res = parse_program(code);
        assert!(
            res.is_err(),
            "expected parse error for `{name}`, but parse succeeded"
        );
    }
}

#[tokio::test]
async fn compile_rejects_invalid_programs() {
    // Each case is intentionally small; they act as “failure examples” for the language.
    type TypeErrorCase = (&'static str, &'static str, fn(&TypeError) -> bool);
    let cases: &[TypeErrorCase] = &[
        (
            "unknown_var",
            "x",
            |e| matches!(e, TypeError::UnknownVar(name) if name.as_ref() == "x"),
        ),
        ("if_condition_not_bool", "if 1 then 2 else 3", |e| {
            matches!(e, TypeError::Unification(..) | TypeError::NoInstance(..))
        }),
        ("if_branches_must_match", "if true then 1 else false", |e| {
            matches!(e, TypeError::Unification(..) | TypeError::NoInstance(..))
        }),
        ("self_application_occurs_check", "\\x -> x x", |e| {
            matches!(e, TypeError::Occurs(..))
        }),
        (
            "unknown_constructor_in_pattern",
            "match 1 when Foo -> 1",
            |e| matches!(e, TypeError::UnknownVar(name) if name.as_ref() == "Foo"),
        ),
        (
            "unknown_constructor_in_expr",
            "Foo 1",
            |e| matches!(e, TypeError::UnknownVar(name) if name.as_ref() == "Foo"),
        ),
        (
            "non_exhaustive_match_on_adt",
            r#"
            type Sum = A i32 | B i32
            match (A 1)
                when A x -> x
            "#,
            |e| matches!(e, TypeError::NonExhaustiveMatch { .. }),
        ),
        (
            "non_exhaustive_match_on_option",
            r#"
            match (Some 1)
                when Some x -> x
            "#,
            |e| matches!(e, TypeError::NonExhaustiveMatch { .. }),
        ),
        ("type_annotation_must_match", "let x: bool = 1 in x", |e| {
            matches!(e, TypeError::Unification(..) | TypeError::NoInstance(..))
        }),
        (
            "unknown_field_projection",
            r#"
            type Foo = Bar { x: i32, y: i32 }
            let foo: Foo = Bar { x = 1, y = 2 } in foo.z
            "#,
            |e| matches!(e, TypeError::UnknownField { field, .. } if field.as_ref() == "z"),
        ),
        (
            "unknown_field_record_update",
            r#"
            type Foo = Bar { x: i32 }
            let foo: Foo = Bar { x = 1 } in { foo with { y = 2 } }
            "#,
            |e| matches!(e, TypeError::UnknownField { field, .. } if field.as_ref() == "y"),
        ),
        (
            "record_update_requires_record_like_base",
            "{ 1 with { x = 2 } }",
            |e| {
                matches!(
                    e,
                    TypeError::UnknownField { .. } | TypeError::FieldNotKnown { .. }
                )
            },
        ),
        (
            "unknown_type_in_annotation",
            "let x: Nope = 1 in x",
            |e| matches!(e, TypeError::UnknownTypeName(name) if name.as_ref() == "Nope"),
        ),
        (
            "invalid_class_arity",
            r#"
            class C where
                m : i32 -> i32
            0
            "#,
            |e| matches!(e, TypeError::InvalidClassArity { class, .. } if class.as_ref() == "C"),
        ),
        (
            "duplicate_class_definition",
            r#"
            class C a where
                m : a -> a
            class C a where
                m : a -> a
            0
            "#,
            |e| matches!(e, TypeError::DuplicateClass(name) if name.as_ref() == "C"),
        ),
        (
            "duplicate_class_method_definition",
            r#"
            class C a where
                m : a -> a
                m : a -> a
            0
            "#,
            |e| matches!(e, TypeError::DuplicateClassMethod(name) if name.as_ref() == "m"),
        ),
        (
            "unknown_class_in_instance",
            r#"
            instance NoSuch i32 where
                m = \x -> x
            0
            "#,
            |e| matches!(e, TypeError::UnknownClass(name) if name.as_ref() == "NoSuch"),
        ),
        (
            "unknown_method_in_instance",
            r#"
            class C a where
                m : a -> a
            instance C i32 where
                n = \x -> x
            0
            "#,
            |e| matches!(e, TypeError::UnknownInstanceMethod { method, .. } if method.as_ref() == "n"),
        ),
        (
            "missing_method_in_instance",
            r#"
            class C a where
                m : a -> a
                n : a -> a
            instance C i32 where
                m = \x -> x
            0
            "#,
            |e| matches!(e, TypeError::MissingInstanceMethod { method, .. } if method.as_ref() == "n"),
        ),
        (
            "duplicate_function_declaration",
            r#"
            fn f (x: i32) -> i32 = x
            fn f (x: i32) -> i32 = x
            f 1
            "#,
            |e| matches!(e, TypeError::DuplicateValue(name) if name.as_ref() == "f"),
        ),
        (
            "function_body_must_match_declared_return_type",
            r#"
            fn f (x: i32) -> bool = x
            f 1
            "#,
            |e| matches!(e, TypeError::Unification(..)),
        ),
        (
            "no_instance_for_predicate",
            r#"
            class C a where
                m : a -> i32
            let x = m 1 in x
            "#,
            |e| {
                matches!(e, TypeError::NoInstance(class, _) if class.as_ref() == "C")
                    || matches!(e, TypeError::AmbiguousTypeVars { constraints, .. } if constraints.contains("C"))
            },
        ),
        ("projection_requires_record_like_base", "1.x", |e| {
            matches!(
                e,
                TypeError::UnknownField { .. } | TypeError::FieldNotKnown { .. }
            )
        }),
        (
            "non_exhaustive_match_on_record_adt",
            r#"
            type Sum = A { x: i32 } | B { x: i32 }
            let s: Sum = A { x = 1 } in
            match s
                when A {x} -> x
            "#,
            |e| matches!(e, TypeError::NonExhaustiveMatch { .. }),
        ),
        (
            "constructor_wrong_payload_type",
            r#"
            type Foo = Bar i32
            let x: Foo = Bar true in x
            "#,
            |e| matches!(e, TypeError::Unification(..)),
        ),
        (
            "wrong_field_type_in_record_constructor",
            r#"
            type Foo = Bar { x: i32 }
            let x: Foo = Bar { x = false } in x
            "#,
            |e| matches!(e, TypeError::Unification(..)),
        ),
        (
            "unknown_type_in_type_decl",
            r#"
            type Foo = Bar Nope
            0
            "#,
            |e| matches!(e, TypeError::UnknownTypeName(name) if name.as_ref() == "Nope"),
        ),
        (
            "unknown_field_in_record_pattern",
            r#"
            type Foo = Bar { x: i32 }
            let v: Foo = Bar { x = 1 } in
            match v
                when Bar { y } -> y
            "#,
            |e| matches!(e, TypeError::UnknownField { field, .. } if field.as_ref() == "y"),
        ),
        (
            "record_payload_pattern_used_on_positional_ctor",
            r#"
            type Foo = Bar i32
            let v: Foo = Bar 1 in
            match v
                when Bar { x } -> x
            "#,
            |e| {
                matches!(
                    e,
                    TypeError::UnknownField { .. }
                        | TypeError::FieldNotKnown { .. }
                        | TypeError::Unification(..)
                )
            },
        ),
        (
            "constraint_kind_mismatch_rejected",
            r#"
            fn my_fn (x: t i32) -> i32 where Foldable t, Default t =
                0

            my_fn [1, 2, 3]
            "#,
            |e| matches!(e, TypeError::KindMismatch { class, .. } if class.as_ref() == "Default"),
        ),
        (
            "fn_decl_missing_required_constraint_is_error",
            r#"
            fn my_fn (x: t a) -> a where Foldable t =
                foldl (\_ acc -> acc) (default) x

            my_fn [[1, 2], [3]]
            "#,
            |e| matches!(e, TypeError::MissingConstraints { constraints } if constraints.contains("Default")),
        ),
    ];

    for (_name, code, pred) in cases {
        expect_type_err(code, *pred).await;
    }
}

#[tokio::test]
async fn compile_rejects_invalid_programs_engine_errors() {
    type EngineErrorCase = (&'static str, &'static str, fn(&EngineError) -> bool);
    let cases: &[EngineErrorCase] = &[
        ("apply_non_function", "1 2", |e| {
            matches!(
                e,
                EngineError::NativeType { expected, .. } if expected.as_str() == "integral"
            )
        }),
        (
            "ambiguous_overload_requires_application",
            "prim_fold",
            |e| matches!(e, EngineError::AmbiguousOverload { name } if name.as_ref() == "prim_fold"),
        ),
        (
            "ambiguous_type_variable_only_in_constraints",
            r#"
            fn my_fn (x: i32) -> i32 where Default b =
                let y: b = default in x

            my_fn 1
            "#,
            |e| {
                matches!(e, EngineError::AmbiguousOverload { name } if name.as_ref() == "default")
                    || matches!(
                        e,
                        EngineError::Type(TypeError::Spanned {
                            error,
                            ..
                        }) if matches!(
                            error.as_ref(),
                            TypeError::AmbiguousTypeVars { constraints, .. }
                                if constraints.contains("Default")
                        )
                    )
            },
        ),
    ];

    for (_name, code, pred) in cases {
        expect_engine_err(code, *pred).await;
    }
}
