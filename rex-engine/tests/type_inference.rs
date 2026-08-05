use rex_ast::{CompilationUnit, Decl, Expr, Span, Symbol};
use rex_engine::standard_type_system;
use rex_parser::parse as parse_rex;
use rex_typesystem::{
    error::TypeError,
    inference::{infer, infer_typed},
    types::{
        BuiltinTypeId, Predicate, Scheme, Type, TypeEnv, TypeKind, TypeVar, TypeVarId,
        TypedExprKind, collect_adts_in_types,
    },
    typesystem::{TypeSystem, TypeSystemLimits, TypeVarSupply, entails, generalize, instantiate},
    unification::unify,
};
use std::sync::Arc;
fn tvar(id: TypeVarId, name: &str) -> Type {
    Type::var(TypeVar::new(id, Some(Symbol::intern(name))))
}

fn dict_of(elem: Type) -> Type {
    Type::app(Type::builtin(BuiltinTypeId::Dict), elem)
}

#[test]
fn unify_simple() {
    let t1 = Type::fun(tvar(0, "a"), Type::builtin(BuiltinTypeId::U32));
    let t2 = Type::fun(Type::builtin(BuiltinTypeId::U16), tvar(1, "b"));
    let subst = unify(&t1, &t2).unwrap();
    assert_eq!(subst.get(&0), Some(&Type::builtin(BuiltinTypeId::U16)));
    assert_eq!(subst.get(&1), Some(&Type::builtin(BuiltinTypeId::U32)));
}

#[test]
fn occurs_check_blocks_infinite_type() {
    let tv = TypeVar::new(0, Some(Symbol::intern("a")));
    let t = Type::fun(Type::var(tv.clone()), Type::builtin(BuiltinTypeId::U8));
    let err = unify(&Type::var(tv), &t).unwrap_err();
    assert!(matches!(err, TypeError::Occurs(_, _)));
}

#[test]
fn instantiate_and_generalize_round_trip() {
    let mut supply = TypeVarSupply::new();
    let a = Type::var(supply.fresh(Some(Symbol::intern("a"))));
    let scheme = generalize(&TypeEnv::new(), vec![], Type::fun(a.clone(), a.clone()));
    let (preds, inst) = instantiate(&scheme, &mut supply);
    assert!(preds.is_empty());
    if let TypeKind::Fun(l, r) = inst.as_ref() {
        match (l.as_ref(), r.as_ref()) {
            (TypeKind::Var(_), TypeKind::Var(_)) => {}
            _ => panic!("expected polymorphic identity"),
        }
    } else {
        panic!("expected function type");
    }
}

#[test]
fn entail_superclasses() {
    let ts = standard_type_system().unwrap();
    let pred = Predicate::new("Semiring", Type::builtin(BuiltinTypeId::I32));
    let given = [Predicate::new(
        "AdditiveGroup",
        Type::builtin(BuiltinTypeId::I32),
    )];
    assert!(entails(&ts.classes, &given, &pred).unwrap());
}

#[test]
fn entail_instances() {
    let ts = standard_type_system().unwrap();
    let pred = Predicate::new("Field", Type::builtin(BuiltinTypeId::F32));
    assert!(entails(&ts.classes, &[], &pred).unwrap());

    let pred = Predicate::new("Divisive", Type::builtin(BuiltinTypeId::U32));
    assert!(entails(&ts.classes, &[], &pred).unwrap());

    let pred = Predicate::new("Subtractive", Type::builtin(BuiltinTypeId::U32));
    assert!(entails(&ts.classes, &[], &pred).unwrap());

    let pred_fail = Predicate::new("Field", Type::builtin(BuiltinTypeId::U32));
    assert!(!entails(&ts.classes, &[], &pred_fail).unwrap());
}

#[test]
fn prelude_injects_functions() {
    let ts = standard_type_system().unwrap();
    let minus = ts.env.lookup(&Symbol::intern("-")).expect("minus in env");
    let div = ts.env.lookup(&Symbol::intern("/")).expect("div in env");
    let length = ts
        .env
        .lookup(&Symbol::intern("length"))
        .expect("length in env");
    assert!(
        ts.env.lookup(&Symbol::intern("count")).is_none(),
        "count was renamed to length"
    );
    assert_eq!(minus.len(), 1);
    assert_eq!(div.len(), 1);
    assert_eq!(length.len(), 1);
    let minus = &minus[0];
    let div = &div[0];
    assert_eq!(minus.preds.len(), 1);
    assert_eq!(minus.vars.len(), 1);
    assert_eq!(div.preds.len(), 1);
    assert_eq!(div.vars.len(), 1);
    assert_eq!(minus.preds[0].class.as_ref(), "Subtractive");
    assert_eq!(div.preds[0].class.as_ref(), "Divisive");
    assert_eq!(length[0].preds.len(), 1);
    assert_eq!(length[0].preds[0].class.as_ref(), "Length");
}

#[test]
fn adt_constructors_are_present() {
    let ts = standard_type_system().unwrap();
    assert!(ts.env.lookup(&Symbol::intern("Empty")).is_some());
    assert!(ts.env.lookup(&Symbol::intern("Cons")).is_some());
    assert!(ts.env.lookup(&Symbol::intern("Ok")).is_some());
    assert!(ts.env.lookup(&Symbol::intern("Err")).is_some());
    assert!(ts.env.lookup(&Symbol::intern("Some")).is_some());
    assert!(ts.env.lookup(&Symbol::intern("None")).is_some());
}

fn parse_expr(code: &str) -> Arc<Expr> {
    parse_rex(code).unwrap().body.unwrap()
}

fn parse_program(code: &str) -> CompilationUnit {
    parse_rex(code).unwrap()
}

#[test]
fn infer_deep_list_does_not_overflow() {
    const N: usize = 40;
    let mut code = String::new();
    code.push_str("let xs = ");
    for _ in 0..N {
        code.push_str("Cons 0 (");
    }
    code.push_str("Empty");
    for _ in 0..N {
        code.push(')');
    }
    code.push_str(" in xs");

    let parse_handle = std::thread::Builder::new()
        .name("infer_deep_list_parse".into())
        .stack_size(128 * 1024 * 1024)
        .spawn(move || parse_rex(&code))
        .unwrap();
    let program = parse_handle.join().unwrap().unwrap();
    let expr = program.body.unwrap();
    let mut ts = standard_type_system().unwrap();
    let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    assert_eq!(
        ty,
        Type::app(
            Type::builtin(BuiltinTypeId::List),
            Type::builtin(BuiltinTypeId::I32)
        )
    );
}

#[test]
fn collect_adts_in_types_finds_nested_unique_adts() {
    let foo = Type::user_con("Foo", 1);
    let bar = Type::user_con("Bar", 0);
    let ty = Type::fun(
        Type::app(
            Type::builtin(BuiltinTypeId::List),
            Type::app(foo.clone(), tvar(0, "a")),
        ),
        Type::tuple(vec![
            Type::app(foo.clone(), Type::builtin(BuiltinTypeId::I32)),
            bar.clone(),
        ]),
    );

    let adts = collect_adts_in_types(vec![ty]).unwrap();
    assert_eq!(adts, vec![foo, bar]);
}

#[test]
fn collect_adts_in_types_rejects_conflicting_names() {
    let arity1 = Type::user_con("Thing", 1);
    let arity2 = Type::user_con("Thing", 2);

    let err = collect_adts_in_types(vec![arity1.clone(), arity2.clone()]).unwrap_err();
    assert_eq!(err.conflicts.len(), 1);
    let conflict = &err.conflicts[0];
    assert_eq!(conflict.name, Symbol::intern("Thing"));
    assert_eq!(conflict.definitions, vec![arity1, arity2]);
}

#[test]
fn infer_depth_limit_is_enforced() {
    const N: usize = 40;
    let mut code = String::new();
    code.push_str("let xs = ");
    for _ in 0..N {
        code.push_str("Cons 0 (");
    }
    code.push_str("Empty");
    for _ in 0..N {
        code.push(')');
    }
    code.push_str(" in xs");

    let program = parse_program(&code);
    let mut ts = standard_type_system().unwrap();
    ts.set_limits(TypeSystemLimits {
        max_infer_depth: Some(8),
    });

    let err = infer(&mut ts, program.body.as_ref().unwrap().as_ref()).unwrap_err();
    assert!(
        err.to_string().contains("maximum inference depth exceeded"),
        "expected a max-depth inference error, got: {err:?}"
    );
}

#[test]
fn declare_fn_injects_scheme_for_use_sites() {
    let program = parse_program(
        r#"
            declare fn id<a> x: a -> a;
            id 1
            "#,
    );
    let mut ts = standard_type_system().unwrap();
    ts.register_decls(&program.decls).unwrap();
    let (preds, ty) = infer(&mut ts, program.body.as_ref().unwrap().as_ref()).unwrap();
    assert!(
        preds.is_empty()
            || preds
                .iter()
                .all(|p| p.class.as_ref() == "Integral"
                    && p.typ == Type::builtin(BuiltinTypeId::I32))
    );
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
}

#[test]
fn declare_fn_is_noop_when_matching_existing_scheme() {
    let mut ts = standard_type_system().unwrap();
    ts.add_value(
        "foo",
        Scheme::new(
            vec![],
            vec![],
            Type::fun(
                Type::builtin(BuiltinTypeId::I32),
                Type::builtin(BuiltinTypeId::I32),
            ),
        ),
    );

    let program = parse_program(
        r#"
            declare fn foo x: i32 -> i32;
            0
            "#,
    );
    let Decl::DeclareFn(fd) = &program.decls[0] else {
        panic!("expected declare fn decl");
    };
    ts.inject_declare_fn_decl(fd).unwrap();
}

#[test]
fn unit_type_parses_and_infers() {
    let program = parse_program(
        r#"
            fn unit_id x: () -> () = x;
            unit_id ()
            "#,
    );
    let mut ts = standard_type_system().unwrap();
    ts.register_decls(&program.decls).unwrap();
    let (preds, ty) = infer(&mut ts, program.body.as_ref().unwrap().as_ref()).unwrap();
    assert!(preds.is_empty());
    assert_eq!(ty, Type::tuple(Vec::<Type>::new()));
}

fn strip_span(mut err: TypeError) -> TypeError {
    while let TypeError::Spanned { error, .. } = err {
        err = *error;
    }
    err
}

#[test]
fn type_errors_include_span() {
    let expr = parse_expr("missing");
    let mut ts = standard_type_system().unwrap();
    let err = infer(&mut ts, expr.as_ref()).unwrap_err();
    match err {
        TypeError::Spanned { span, error } => {
            assert_ne!(span, Span::default());
            assert!(matches!(
                *error,
                TypeError::UnknownVar(name) if name.as_ref() == "missing"
            ));
        }
        other => panic!("expected spanned error, got {other:?}"),
    }
}

#[test]
fn reject_user_redefinition_of_primitive_type_name() {
    let program = parse_program("type i32 = I32Wrap i32;");
    let mut ts = standard_type_system().unwrap();
    let Decl::Type(decl) = &program.decls[0] else {
        panic!("expected type decl");
    };
    let err = ts.register_type_decl(decl).unwrap_err();
    assert!(matches!(
        err,
        TypeError::ReservedTypeName(name) if name.as_ref() == "i32"
    ));
}

#[test]
fn reject_user_redefinition_of_prelude_adt_name() {
    let program = parse_program("type Result e a = Nope e a;");
    let mut ts = standard_type_system().unwrap();
    let Decl::Type(decl) = &program.decls[0] else {
        panic!("expected type decl");
    };
    let err = ts.register_type_decl(decl).unwrap_err();
    assert!(matches!(
        err,
        TypeError::ReservedTypeName(name) if name.as_ref() == "Result"
    ));
}

#[test]
fn reject_user_redefinition_of_promise_type_name() {
    let program = parse_program("type Promise a = PromiseWrap a;");
    let mut ts = standard_type_system().unwrap();
    let Decl::Type(decl) = &program.decls[0] else {
        panic!("expected type decl");
    };
    let err = ts.register_type_decl(decl).unwrap_err();
    assert!(matches!(
        err,
        TypeError::ReservedTypeName(name) if name.as_ref() == "Promise"
    ));
}

#[test]
fn infer_polymorphic_id_tuple() {
    let expr = parse_expr(
        r#"
            let
                id = \x -> x
            in
                id (id 420, id 6.9, id "str")
            "#,
    );
    let mut ts = standard_type_system().unwrap();
    let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    let expected = Type::tuple(vec![
        Type::builtin(BuiltinTypeId::I32),
        Type::builtin(BuiltinTypeId::F32),
        Type::builtin(BuiltinTypeId::String),
    ]);
    assert_eq!(ty, expected);
}

#[test]
fn infer_type_annotation_ok() {
    let expr = parse_expr("let x: i32 = 42 in x");
    let mut ts = standard_type_system().unwrap();
    let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
}

#[test]
fn renamed_builtin_type_annotations_use_capitalized_names() {
    for (name, id) in [
        ("Bool", BuiltinTypeId::Bool),
        ("Char", BuiltinTypeId::Char),
        ("String", BuiltinTypeId::String),
        ("UUID", BuiltinTypeId::Uuid),
        ("Hash", BuiltinTypeId::Hash),
        ("DateTime", BuiltinTypeId::DateTime),
    ] {
        let expr = parse_expr(&format!(r"\(x: {name}) -> x"));
        let mut ts = standard_type_system().unwrap();
        let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        let builtin = Type::builtin(id);
        assert_eq!(ty, Type::fun(builtin.clone(), builtin), "{name}");
    }

    for name in ["bool", "char", "string", "uuid", "hash", "datetime"] {
        let expr = parse_expr(&format!(r"\(x: {name}) -> x"));
        let mut ts = standard_type_system().unwrap();
        let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
        assert!(
            matches!(&err, TypeError::UnknownTypeName(unknown) if unknown.as_ref() == name),
            "{name}: {err:?}"
        );
    }
}

#[test]
fn infer_float_literal_specializes_to_f64_annotation() {
    let expr = parse_expr("let x: f64 = 3.14 in x");
    let mut ts = standard_type_system().unwrap();
    let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::F64));
}

#[test]
fn infer_float_literals_specialize_to_annotated_function_args() {
    for (source, expected) in [
        (
            r#"
            let
              add_float: f32 -> f32 -> f32 = \x y -> x + y
            in
              add_float 3.0 4.0
            "#,
            BuiltinTypeId::F32,
        ),
        (
            r#"
            let
              add_float: f64 -> f64 -> f64 = \x y -> x + y
            in
              add_float 3.0 4.0
            "#,
            BuiltinTypeId::F64,
        ),
    ] {
        let expr = parse_expr(source);
        let mut ts = standard_type_system().unwrap();
        let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        assert_eq!(ty, Type::builtin(expected), "{source}");
    }
}

#[test]
fn infer_type_annotation_lambda_param() {
    let expr = parse_expr("\\ (a : f32) -> a");
    let mut ts = standard_type_system().unwrap();
    let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    assert_eq!(
        ty,
        Type::fun(
            Type::builtin(BuiltinTypeId::F32),
            Type::builtin(BuiltinTypeId::F32)
        )
    );
}

#[test]
fn infer_type_annotation_is_alias() {
    let expr = parse_expr("\"hi\" is str");
    let mut ts = standard_type_system().unwrap();
    let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::String));
}

#[test]
fn infer_type_annotation_with_promise_constructor() {
    let expr = parse_expr("\\(x: Promise i32) -> x");
    let mut ts = standard_type_system().unwrap();
    let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    let promise_i32 = Type::promise(Type::builtin(BuiltinTypeId::I32));
    assert_eq!(ty, Type::fun(promise_i32.clone(), promise_i32));
}

#[test]
fn infer_type_annotation_mismatch_error() {
    let expr = parse_expr("let x: i32 = 3.14 in x");
    let mut ts = standard_type_system().unwrap();
    let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
    assert!(matches!(err, TypeError::Unification(_, _)));
}

#[test]
fn infer_project_single_variant_let() {
    let program = parse_program(
        r#"
            type MyADT = MyVariant1 { field1: i32, field2: f32 };
            let
                x = MyVariant1 { field1 = 1, field2 = 2.0 }
            in
                (x.field1, x.field2)
            "#,
    );
    let mut ts = standard_type_system().unwrap();
    for decl in &program.decls {
        if let Decl::Type(decl) = decl {
            ts.register_type_decl(decl).unwrap();
        }
    }
    let (_preds, ty) = infer(&mut ts, program.body.as_ref().unwrap().as_ref()).unwrap();
    let expected = Type::tuple(vec![
        Type::builtin(BuiltinTypeId::I32),
        Type::builtin(BuiltinTypeId::F32),
    ]);
    assert_eq!(ty, expected);
}

#[test]
fn infer_project_known_variant_let() {
    let program = parse_program(
        r#"
            type MyADT = MyVariant1 { field1: i32, field2: f32 } | MyVariant2 i32 f32;
            let
                x = MyVariant1 { field1 = 1, field2 = 2.0 }
            in
                x.field1
            "#,
    );
    let mut ts = standard_type_system().unwrap();
    for decl in &program.decls {
        if let Decl::Type(decl) = decl {
            ts.register_type_decl(decl).unwrap();
        }
    }
    let (_preds, ty) = infer(&mut ts, program.body.as_ref().unwrap().as_ref()).unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
}

#[test]
fn infer_project_unknown_variant_error() {
    let program = parse_program(
        r#"
            type MyADT = MyVariant1 { field1: i32, field2: f32 } | MyVariant2 i32 f32;
            let
                x = MyVariant2 1 2.0
            in
                x.field1
            "#,
    );
    let mut ts = standard_type_system().unwrap();
    for decl in &program.decls {
        if let Decl::Type(decl) = decl {
            ts.register_type_decl(decl).unwrap();
        }
    }
    let err = strip_span(infer(&mut ts, program.body.as_ref().unwrap().as_ref()).unwrap_err());
    assert!(matches!(err, TypeError::FieldNotKnown { .. }));
}

#[test]
fn infer_project_lambda_param_single_variant() {
    let program = parse_program(
        r#"
            type Boxed = Boxed { value: i32 };
            let
                f = \x -> x.value
            in
                f (Boxed { value = 1 })
            "#,
    );
    let mut ts = standard_type_system().unwrap();
    for decl in &program.decls {
        if let Decl::Type(decl) = decl {
            ts.register_type_decl(decl).unwrap();
        }
    }
    let (_preds, ty) = infer(&mut ts, program.body.as_ref().unwrap().as_ref()).unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
}

#[test]
fn infer_project_in_match_arm() {
    let program = parse_program(
        r#"
            type MyADT = MyVariant1 { field1: i32 } | MyVariant2 i32;
            let
                x = MyVariant1 { field1 = 1 }
            in
                match x with {
                    case MyVariant1 { field1 } -> x.field1;
                    case MyVariant2 _ -> 0;
                }
            "#,
    );
    let mut ts = standard_type_system().unwrap();
    for decl in &program.decls {
        if let Decl::Type(decl) = decl {
            ts.register_type_decl(decl).unwrap();
        }
    }
    let (_preds, ty) = infer(&mut ts, program.body.as_ref().unwrap().as_ref()).unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
}

#[test]
fn infer_nested_let_lambda_match_option() {
    let expr = parse_expr(
        r#"
            let
                choose = \flag a b -> if flag then a else b,
                build = \flag ->
                    let
                        pick = choose flag,
                        val = pick 1 2
                    in
                        Some val
            in
                match (build true) with {
                    case Some x -> x;
                    case None -> 0;
                }
            "#,
    );
    let mut ts = standard_type_system().unwrap();
    let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
}

#[test]
fn infer_polymorphic_apply_in_tuple() {
    let expr = parse_expr(
        r#"
            let
                apply = \f x -> f x,
                id = \x -> x,
                wrap = \x -> (x, x)
            in
                (apply id 1, apply id "hi", apply wrap true)
            "#,
    );
    let mut ts = standard_type_system().unwrap();
    let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    let expected = Type::tuple(vec![
        Type::builtin(BuiltinTypeId::I32),
        Type::builtin(BuiltinTypeId::String),
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::Bool),
            Type::builtin(BuiltinTypeId::Bool),
        ]),
    ]);
    assert_eq!(ty, expected);
}

#[test]
fn infer_nested_result_option_match() {
    let expr = parse_expr(
        r#"
            let
                unwrap = \x ->
                    match x with {
                        case Ok (Some v) -> v;
                        case Ok None -> 0;
                        case Err _ -> 0;
                    }
            in
                unwrap (Ok (Some 5))
            "#,
    );
    let mut ts = standard_type_system().unwrap();
    let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
}

#[test]
fn infer_head_or_list_match() {
    let expr = parse_expr(
        r#"
            let
                head_or = \fallback xs ->
                    match xs with {
                        case [] -> fallback;
                        case x::xs -> x;
                    }
            in
                (head_or 0 [1, 2, 3], head_or 0 [])
            "#,
    );
    let mut ts = standard_type_system().unwrap();
    let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    let expected = Type::tuple(vec![
        Type::builtin(BuiltinTypeId::I32),
        Type::builtin(BuiltinTypeId::I32),
    ]);
    assert_eq!(ty, expected);
}

#[test]
fn infer_head_or_list_match_cons_constructor_form() {
    let expr = parse_expr(
        r#"
            let
                head_or = \fallback xs ->
                    match xs with {
                        case [] -> fallback;
                        case Cons x xs1 -> x;
                    }
            in
                (head_or 0 (Cons 1 (Cons 2 Empty)), head_or 0 Empty)
            "#,
    );
    let mut ts = standard_type_system().unwrap();
    let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    let expected = Type::tuple(vec![
        Type::builtin(BuiltinTypeId::I32),
        Type::builtin(BuiltinTypeId::I32),
    ]);
    assert_eq!(ty, expected);
}

#[test]
fn infer_record_pattern_in_lambda() {
    let program = parse_program(
        r#"
            type Pair = Pair { left: i32, right: i32 };
            let
                sum = \p ->
                    match p with {
                        case Pair { left, right } -> left + right;
                    }
            in
                sum (Pair { left = 1, right = 2 })
            "#,
    );
    let mut ts = standard_type_system().unwrap();
    for decl in &program.decls {
        if let Decl::Type(decl) = decl {
            ts.register_type_decl(decl).unwrap();
        }
    }
    let (_preds, ty) = infer(&mut ts, program.body.as_ref().unwrap().as_ref()).unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
}

#[test]
fn infer_fn_decl_simple() {
    let program = parse_program(
        r#"
            fn add (x: i32, y: i32) -> i32 = x + y;
            add 1 2
            "#,
    );
    let mut ts = standard_type_system().unwrap();
    let expr = program.body_with_fns().unwrap();
    let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
}

#[test]
fn infer_fn_decl_signature_form() {
    let program = parse_program(
        r#"
            fn add : i32 -> i32 -> i32 = \x y -> x + y;
            add 1 2
            "#,
    );
    let mut ts = standard_type_system().unwrap();
    let expr = program.body_with_fns().unwrap();
    let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
}

#[test]
fn infer_fn_decl_polymorphic_where_constraints() {
    let program = parse_program(
        r#"
            fn my_add<a> (x: a, y: a) -> a where AdditiveMonoid a = x + y;
            (my_add 1 2, my_add 1.0 2.0)
            "#,
    );
    let mut ts = standard_type_system().unwrap();
    let expr = program.body_with_fns().unwrap();
    let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::F32)
        ])
    );
}

#[test]
fn infer_additive_monoid_constraint() {
    let expr = parse_expr("\\x y -> x + y");
    let mut ts = standard_type_system().unwrap();
    let (preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    assert_eq!(preds.len(), 1);
    assert_eq!(preds[0].class.as_ref(), "AdditiveMonoid");

    if let TypeKind::Fun(a, rest) = ty.as_ref()
        && let TypeKind::Fun(b, c) = rest.as_ref()
    {
        assert_eq!(a.as_ref(), b.as_ref());
        assert_eq!(b.as_ref(), c.as_ref());
        assert_eq!(preds[0].typ, a.clone());
        return;
    }
    panic!("expected a -> a -> a");
}

#[test]
fn infer_multiplicative_monoid_constraint() {
    let expr = parse_expr("\\x y -> x * y");
    let mut ts = standard_type_system().unwrap();
    let (preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    assert_eq!(preds.len(), 1);
    assert_eq!(preds[0].class.as_ref(), "MultiplicativeMonoid");

    if let TypeKind::Fun(a, rest) = ty.as_ref()
        && let TypeKind::Fun(b, c) = rest.as_ref()
    {
        assert_eq!(a.as_ref(), b.as_ref());
        assert_eq!(b.as_ref(), c.as_ref());
        assert_eq!(preds[0].typ, a.clone());
        return;
    }
    panic!("expected a -> a -> a");
}

#[test]
fn infer_subtractive_constraint() {
    let expr = parse_expr("\\x y -> x - y");
    let mut ts = standard_type_system().unwrap();
    let (preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    assert_eq!(preds.len(), 1);
    assert_eq!(preds[0].class.as_ref(), "Subtractive");

    if let TypeKind::Fun(a, rest) = ty.as_ref()
        && let TypeKind::Fun(b, c) = rest.as_ref()
    {
        assert_eq!(a.as_ref(), b.as_ref());
        assert_eq!(b.as_ref(), c.as_ref());
        assert_eq!(preds[0].typ, a.clone());
        return;
    }
    panic!("expected a -> a -> a");
}

#[test]
fn infer_integral_constraint() {
    let expr = parse_expr("\\x y -> x % y");
    let mut ts = standard_type_system().unwrap();
    let (preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    assert_eq!(preds.len(), 1);
    assert_eq!(preds[0].class.as_ref(), "Integral");

    if let TypeKind::Fun(a, rest) = ty.as_ref()
        && let TypeKind::Fun(b, c) = rest.as_ref()
    {
        assert_eq!(a.as_ref(), b.as_ref());
        assert_eq!(b.as_ref(), c.as_ref());
        assert_eq!(preds[0].typ, a.clone());
        return;
    }
    panic!("expected a -> a -> a");
}

#[test]
fn infer_literal_addition_defaults() {
    let expr = parse_expr("1 + 2");
    let mut ts = standard_type_system().unwrap();
    let (preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_eq!(preds.len(), 2);
    assert!(preds.iter().any(|p| p.class.as_ref() == "AdditiveMonoid"));
    assert!(preds.iter().any(|p| p.class.as_ref() == "Integral"));
    assert!(
        preds
            .iter()
            .all(|p| p.typ == Type::builtin(BuiltinTypeId::I32))
    );
}

#[test]
fn infer_mod_defaults() {
    let expr = parse_expr("1 % 2");
    let mut ts = standard_type_system().unwrap();
    let (preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_eq!(preds.len(), 1);
    assert_eq!(preds[0].class.as_ref(), "Integral");
    assert_eq!(preds[0].typ, Type::builtin(BuiltinTypeId::I32));
}

#[test]
fn infer_get_list_type() {
    let expr = parse_expr("get (1 is u64) (([1, 2, 3]) is List i32)");
    let mut ts = standard_type_system().unwrap();
    let (preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert!(preds.iter().any(|p| p.class.as_ref() == "Indexable"));
    assert!(preds.iter().all(|p| {
        p.class.as_ref() == "Indexable"
            || (p.class.as_ref() == "Integral"
                && (p.typ == Type::builtin(BuiltinTypeId::U64)
                    || p.typ == Type::builtin(BuiltinTypeId::I32)))
    }));
    assert!(
        preds.iter().any(|p| {
            p.class.as_ref() == "Integral" && p.typ == Type::builtin(BuiltinTypeId::U64)
        })
    );
    for pred in preds.iter().filter(|p| p.class.as_ref() == "Indexable") {
        assert!(entails(&ts.classes, &[], pred).unwrap());
    }
}

#[test]
fn infer_get_tuple_type() {
    let expr = parse_expr(r#"(1, "Hello", true).0"#);
    let mut ts = standard_type_system().unwrap();
    let (preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert!(preds.is_empty() || preds.iter().all(|p| p.class.as_ref() == "Integral"));

    let expr = parse_expr(r#"(1, "Hello", true).1"#);
    let mut ts = standard_type_system().unwrap();
    let (preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::String));
    assert!(preds.is_empty() || preds.iter().all(|p| p.class.as_ref() == "Integral"));

    let expr = parse_expr(r#"(1, "Hello", true).2"#);
    let mut ts = standard_type_system().unwrap();
    let (preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::Bool));
    assert!(preds.is_empty() || preds.iter().all(|p| p.class.as_ref() == "Integral"));
}

#[test]
fn infer_division_defaults() {
    let expr = parse_expr("1.0 / 2.0");
    let mut ts = standard_type_system().unwrap();
    let (preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::F32));
    assert_eq!(preds.len(), 2);
    assert!(preds.iter().any(|p| p.class.as_ref() == "Divisive"));
    assert!(preds.iter().any(|p| p.class.as_ref() == "Field"));
    assert!(
        preds
            .iter()
            .all(|p| p.typ == Type::builtin(BuiltinTypeId::F32))
    );
    for pred in preds {
        assert!(entails(&ts.classes, &[], &pred).unwrap());
    }
}

#[test]
fn infer_integer_division_defaults() {
    let expr = parse_expr("1 / 2");
    let mut ts = standard_type_system().unwrap();
    let (preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_eq!(preds.len(), 2);
    assert!(preds.iter().any(|p| p.class.as_ref() == "Divisive"));
    assert!(preds.iter().any(|p| p.class.as_ref() == "Integral"));
    assert!(
        preds
            .iter()
            .all(|p| p.typ == Type::builtin(BuiltinTypeId::I32))
    );
}

#[test]
fn infer_unbound_variable_error() {
    let expr = parse_expr("missing");
    let mut ts = standard_type_system().unwrap();
    let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
    assert!(matches!(
        err,
        TypeError::UnknownVar(name) if name.as_ref() == "missing"
    ));
}

#[test]
fn infer_if_branch_type_mismatch_error() {
    let expr = parse_expr(r#"if true then 1 else "no""#);
    let mut ts = standard_type_system().unwrap();
    let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
    match err {
        TypeError::Unification(a, b) => {
            let ok = (a == "i32" && b == "String") || (a == "String" && b == "i32");
            assert!(ok, "expected i32 vs String, got {a} vs {b}");
        }
        other => panic!("expected unification error, got {other:?}"),
    }
}

#[test]
fn infer_unknown_pattern_constructor_error() {
    let expr = parse_expr("match 1 with { case Nope -> 1; }");
    let mut ts = standard_type_system().unwrap();
    let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
    assert!(matches!(
        err,
        TypeError::UnknownVar(name) if name.as_ref() == "Nope"
    ));
}

#[test]
fn infer_ambiguous_overload_error() {
    let mut ts = TypeSystem::new();
    let a = TypeVar::new(0, Some(Symbol::intern("a")));
    let b = TypeVar::new(1, Some(Symbol::intern("b")));
    let scheme_a = Scheme::new(vec![a.clone()], vec![], Type::var(a));
    let scheme_b = Scheme::new(vec![b.clone()], vec![], Type::var(b));
    ts.add_overload(Symbol::intern("dup"), scheme_a);
    ts.add_overload(Symbol::intern("dup"), scheme_b);
    let expr = parse_expr("dup");
    let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
    assert!(matches!(
        err,
        TypeError::AmbiguousOverload(name) if name.as_ref() == "dup"
    ));
}

#[test]
fn infer_if_cond_not_bool_error() {
    let expr = parse_expr("if 1 then 2 else 3");
    let mut ts = standard_type_system().unwrap();
    let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
    match err {
        TypeError::Unification(a, b) => {
            let ok = (a == "Bool" && b == "i32") || (a == "i32" && b == "Bool");
            assert!(ok, "expected Bool vs i32, got {a} vs {b}");
        }
        other => panic!("expected unification error, got {other:?}"),
    }
}

#[test]
fn infer_apply_non_function_error() {
    let expr = parse_expr("1 2");
    let mut ts = standard_type_system().unwrap();
    let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
    assert!(matches!(err, TypeError::Unification(_, _)));
}

#[test]
fn infer_list_element_mismatch_error() {
    let expr = parse_expr("[1, true]");
    let mut ts = standard_type_system().unwrap();
    let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
    match err {
        TypeError::Unification(a, b) => {
            let ok = (a == "i32" && b == "Bool") || (a == "Bool" && b == "i32");
            assert!(ok, "expected i32 vs Bool, got {a} vs {b}");
        }
        other => panic!("expected unification error, got {other:?}"),
    }
}

#[test]
fn infer_dict_value_mismatch_error() {
    let expr = parse_expr("{a = 1, b = true}");
    let mut ts = standard_type_system().unwrap();
    let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
    match err {
        TypeError::Unification(a, b) => {
            let ok = (a == "i32" && b == "Bool") || (a == "Bool" && b == "i32");
            assert!(ok, "expected i32 vs Bool, got {a} vs {b}");
        }
        other => panic!("expected unification error, got {other:?}"),
    }
}

#[test]
fn infer_match_list_on_non_list_error() {
    let expr = parse_expr("match 1 with { case [x] -> x; }");
    let mut ts = standard_type_system().unwrap();
    assert!(infer(&mut ts, expr.as_ref()).is_err());
}

#[test]
fn infer_pattern_constructor_arity_error() {
    let expr = parse_expr("match (Ok 1) with { case Ok x y -> x; }");
    let mut ts = standard_type_system().unwrap();
    let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
    assert!(matches!(
        err,
        TypeError::UnsupportedExpr("pattern constructor")
    ));
}

#[test]
fn infer_match_arm_type_mismatch_error() {
    let expr = parse_expr(r#"match 1 with { case _ -> 1; case _ -> "no"; }"#);
    let mut ts = standard_type_system().unwrap();
    let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
    match err {
        TypeError::Unification(a, b) => {
            let ok = (a == "i32" && b == "String") || (a == "String" && b == "i32");
            assert!(ok, "expected i32 vs String, got {a} vs {b}");
        }
        other => panic!("expected unification error, got {other:?}"),
    }
}

#[test]
fn infer_match_option_on_non_option_error() {
    let expr = parse_expr("match 1 with { case Some x -> x; }");
    let mut ts = standard_type_system().unwrap();
    assert!(infer(&mut ts, expr.as_ref()).is_err());
}

#[test]
fn infer_dict_pattern_on_non_dict_error() {
    let expr = parse_expr("match 1 with { case {a} -> a; }");
    let mut ts = standard_type_system().unwrap();
    let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
    assert!(matches!(err, TypeError::Unification(_, _)));
}

#[test]
fn infer_cons_pattern_on_non_list_error() {
    let expr = parse_expr("match 1 with { case x::xs -> x; }");
    let mut ts = standard_type_system().unwrap();
    assert!(infer(&mut ts, expr.as_ref()).is_err());
}

#[test]
fn infer_apply_wrong_arg_type_error() {
    let expr = parse_expr("(\\x -> x + 1) true");
    let mut ts = standard_type_system().unwrap();
    let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
    assert!(matches!(err, TypeError::Unification(_, _)));
}

#[test]
fn infer_self_application_occurs_error() {
    let expr = parse_expr("\\x -> x x");
    let mut ts = standard_type_system().unwrap();
    let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
    assert!(matches!(err, TypeError::Occurs(_, _)));
}

#[test]
fn infer_apply_constructor_too_many_args_error() {
    let expr = parse_expr("Some 1 2");
    let mut ts = standard_type_system().unwrap();
    let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
    assert!(matches!(err, TypeError::Unification(_, _)));
}

#[test]
fn infer_operator_type_mismatch_error() {
    let expr = parse_expr("1 + true");
    let mut ts = standard_type_system().unwrap();
    let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
    assert!(matches!(err, TypeError::Unification(_, _)));
}

#[test]
fn infer_non_exhaustive_match_is_error() {
    let expr = parse_expr("match (Ok 1) with { case Ok x -> x; }");
    let mut ts = standard_type_system().unwrap();
    let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
    assert!(matches!(err, TypeError::NonExhaustiveMatch { .. }));
}

#[test]
fn infer_non_exhaustive_match_on_bound_var_error() {
    let expr = parse_expr("let x = Ok 1 in match x with { case Ok y -> y; }");
    let mut ts = standard_type_system().unwrap();
    let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
    assert!(matches!(err, TypeError::NonExhaustiveMatch { .. }));
}

#[test]
fn infer_non_exhaustive_match_in_lambda_error() {
    let expr = parse_expr("\\x -> match x with { case Ok y -> y; }");
    let mut ts = standard_type_system().unwrap();
    let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
    assert!(matches!(err, TypeError::NonExhaustiveMatch { .. }));
}

#[test]
fn infer_non_exhaustive_option_match_error() {
    let expr = parse_expr("match (Some 1) with { case Some x -> x; }");
    let mut ts = standard_type_system().unwrap();
    let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
    match err {
        TypeError::NonExhaustiveMatch { missing, .. } => {
            assert_eq!(missing, vec![Symbol::intern("None")]);
        }
        other => panic!("expected non-exhaustive match, got {other:?}"),
    }
}

#[test]
fn infer_non_exhaustive_result_match_error() {
    let expr = parse_expr("match (Err 1) with { case Ok x -> x; }");
    let mut ts = standard_type_system().unwrap();
    let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
    match err {
        TypeError::NonExhaustiveMatch { missing, .. } => {
            assert_eq!(missing, vec![Symbol::intern("Err")]);
        }
        other => panic!("expected non-exhaustive match, got {other:?}"),
    }
}

#[test]
fn infer_non_exhaustive_list_missing_empty_error() {
    let expr = parse_expr("match [1, 2] with { case x::xs -> x; }");
    let mut ts = standard_type_system().unwrap();
    let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
    match err {
        TypeError::NonExhaustiveMatch { missing, .. } => {
            assert_eq!(missing, vec![Symbol::intern("Empty")]);
        }
        other => panic!("expected non-exhaustive match, got {other:?}"),
    }
}

#[test]
fn infer_non_exhaustive_list_match_on_bound_var_error() {
    let expr = parse_expr("let xs = [1, 2] in match xs with { case x::xs -> x; }");
    let mut ts = standard_type_system().unwrap();
    let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
    assert!(matches!(err, TypeError::NonExhaustiveMatch { .. }));
}

#[test]
fn infer_non_exhaustive_list_missing_cons_error() {
    let expr = parse_expr("match [1] with { case [] -> 0; }");
    let mut ts = standard_type_system().unwrap();
    let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
    match err {
        TypeError::NonExhaustiveMatch { missing, .. } => {
            assert_eq!(missing, vec![Symbol::intern("Cons")]);
        }
        other => panic!("expected non-exhaustive match, got {other:?}"),
    }
}

#[test]
fn infer_match_list_patterns_on_result_error() {
    let expr = parse_expr("match (Ok 1) with { case [] -> 0; case x::xs -> 1; }");
    let mut ts = standard_type_system().unwrap();
    let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
    assert!(matches!(err, TypeError::Unification(_, _)));
}

#[test]
fn infer_missing_instances_produce_unsatisfied_predicates() {
    for (name, code) in [
        ("eq_dict", "{a = 1} == {a = 2}"),
        ("min_bool", "min [true]"),
    ] {
        let (class, pred_type, expected_ty) = match name {
            "eq_dict" => ("Eq", dict_of(Type::builtin(BuiltinTypeId::I32)), None),
            "min_bool" => ("Ord", Type::builtin(BuiltinTypeId::Bool), None),
            _ => unreachable!("unknown test case {name}"),
        };

        let expr = parse_expr(code);
        let mut ts = standard_type_system().unwrap();
        let (preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        if let Some(expected) = expected_ty {
            assert_eq!(ty, expected, "{name}");
        }

        let pred = preds
            .iter()
            .find(|p| p.class.as_ref() == class && p.typ == pred_type)
            .unwrap();
        assert!(!entails(&ts.classes, &[], pred).unwrap(), "{name}");
    }
}

#[test]
fn record_update_single_variant_adt_infers() {
    let program = parse_program(
        r#"
            type Foo = Bar { x: i32, y: i32 };
            let
              foo: Foo = Bar { x = 1, y = 2 },
              bar = { foo with { x = 3 } }
            in
              bar
            "#,
    );
    let mut ts = standard_type_system().unwrap();
    ts.register_decls(&program.decls).unwrap();
    let (_preds, typ) = infer(&mut ts, program.body.as_ref().unwrap().as_ref()).unwrap();
    assert_eq!(typ.to_string(), "Foo");
}

#[test]
fn record_update_unknown_field_errors() {
    let program = parse_program(
        r#"
            type Foo = Bar { x: i32 };
            let
              foo: Foo = Bar { x = 1 }
            in
              { foo with { y = 2 } }
            "#,
    );
    let mut ts = standard_type_system().unwrap();
    ts.register_decls(&program.decls).unwrap();
    let err = infer(&mut ts, program.body.as_ref().unwrap().as_ref()).unwrap_err();
    let err = strip_span(err);
    assert!(matches!(err, TypeError::UnknownField { .. }));
}

#[test]
fn record_update_requires_refined_variant_for_sum_types() {
    let program = parse_program(
        r#"
            type Foo = Bar { x: i32 } | Baz { x: i32 };
            let
              f = \ (foo : Foo) -> { foo with { x = 2 } }
            in
              f (Bar { x = 1 })
            "#,
    );
    let mut ts = standard_type_system().unwrap();
    ts.register_decls(&program.decls).unwrap();
    let err = infer(&mut ts, program.body.as_ref().unwrap().as_ref()).unwrap_err();
    let err = strip_span(err);
    assert!(matches!(err, TypeError::FieldNotKnown { .. }));
}

#[test]
fn record_update_allowed_after_match_refines_variant() {
    let program = parse_program(
        r#"
            type Foo = Bar { x: i32 } | Baz { x: i32 };
            let
              f = \ (foo : Foo) ->
                match foo with {
                  case Bar {x} -> { foo with { x = x + 1 } };
                  case Baz {x} -> { foo with { x = x + 2 } };
                }
            in
              f (Bar { x = 1 })
            "#,
    );
    let mut ts = standard_type_system().unwrap();
    ts.register_decls(&program.decls).unwrap();
    let (_preds, typ) = infer(&mut ts, program.body.as_ref().unwrap().as_ref()).unwrap();
    assert_eq!(typ.to_string(), "Foo");
}

#[test]
fn record_update_plain_record_type() {
    let program = parse_program(
        r#"
            let
              f = \ (r : { x: i32, y: i32 }) -> { r with { y = 9 } }
            in
              f { x = 1, y = 2 }
            "#,
    );
    let mut ts = standard_type_system().unwrap();
    ts.register_decls(&program.decls).unwrap();
    let (_preds, typ) = infer(&mut ts, program.body.as_ref().unwrap().as_ref()).unwrap();
    assert_eq!(typ.to_string(), "{x: i32, y: i32}");
}

#[test]
fn infer_typed_hole_expr_is_hole_kind() {
    let expr = parse_expr("?");
    let mut ts = standard_type_system().unwrap();
    let (typed, _preds, _ty) = infer_typed(&mut ts, expr.as_ref()).unwrap();
    assert!(
        matches!(typed.kind.as_ref(), TypedExprKind::Hole),
        "typed={typed:#?}"
    );
}

#[test]
fn infer_hole_with_annotation_unifies_to_annotation() {
    let expr = parse_expr("let x : i32 = ? in x");
    let mut ts = standard_type_system().unwrap();
    let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
}

#[test]
fn infer_hole_in_if_condition_is_bool_constrained() {
    let expr = parse_expr("if ? then 1 else 2");
    let mut ts = standard_type_system().unwrap();
    let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
}

#[test]
fn infer_hole_in_arithmetic_is_numeric_constrained() {
    let expr = parse_expr("? + 1");
    let mut ts = standard_type_system().unwrap();
    let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
}

#[test]
fn infer_hole_arithmetic_conflicting_annotation_failure() {
    let expr = parse_expr("let x : String = (? + 1) in x");
    let mut ts = standard_type_system().unwrap();
    let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
    assert!(matches!(err, TypeError::Unification(_, _)), "err={err:#?}");
}
