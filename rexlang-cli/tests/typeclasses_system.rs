use rexlang::{
    BuiltinTypeId, Engine, GasMeter, Parser, Token, Type, TypeKind, ValueDisplayOptions,
    pointer_display_with,
};

fn type_compatible(actual: &Type, expected: &Type) -> bool {
    match (actual.as_ref(), expected.as_ref()) {
        (TypeKind::Var(_), TypeKind::Con(tc)) if tc.name.as_ref() == "i32" => true,
        (TypeKind::Con(a), TypeKind::Con(b)) => a.name == b.name && a.arity == b.arity,
        (TypeKind::App(af, aa), TypeKind::App(ef, ea))
        | (TypeKind::Fun(af, aa), TypeKind::Fun(ef, ea)) => {
            type_compatible(af, ef) && type_compatible(aa, ea)
        }
        (TypeKind::Tuple(as_), TypeKind::Tuple(es)) if as_.len() == es.len() => as_
            .iter()
            .zip(es.iter())
            .all(|(a, e)| type_compatible(a, e)),
        (TypeKind::Record(as_), TypeKind::Record(es)) if as_.len() == es.len() => as_
            .iter()
            .zip(es.iter())
            .all(|((an, at), (en, et))| an == en && type_compatible(at, et)),
        _ => false,
    }
}

async fn eval_to_string(code: &str, expected_ty: Type) -> Result<String, String> {
    let tokens = Token::tokenize(code).map_err(|e| format!("lex error: {e}"))?;
    let mut parser = Parser::new(tokens);
    let program = parser
        .parse_program(&mut GasMeter::default())
        .map_err(|errs| format!("parse error: {errs:?}"))?;

    let mut engine = Engine::with_prelude(()).unwrap();
    engine
        .inject_decls(&program.decls)
        .map_err(|e| format!("{e}"))?;
    let mut gas = GasMeter::default();
    let (pointer, ty) = engine
        .eval(program.expr.as_ref(), &mut gas)
        .await
        .map_err(|e| format!("{e}"))?;
    assert!(
        type_compatible(&ty, &expected_ty),
        "eval returned unexpected type for: {code}\nactual: {ty}\nexpected: {expected_ty}"
    );
    let opts = ValueDisplayOptions {
        include_numeric_suffixes: true,
        ..ValueDisplayOptions::default()
    };
    pointer_display_with(&engine.heap, &pointer, opts).map_err(|e| format!("{e}"))
}

async fn assert_eval(code: &str, expected: &str, expected_ty: Type) {
    let actual = eval_to_string(code, expected_ty)
        .await
        .unwrap_or_else(|e| panic!("expected ok, got error: {e}"));
    assert_eq!(actual, expected);
}

async fn assert_err_contains(code: &str, needle: &str) {
    let err = eval_to_string(code, Type::builtin(BuiltinTypeId::I32))
        .await
        .unwrap_err();
    assert!(
        err.contains(needle),
        "expected error containing {needle:?}, got: {err}"
    );
}

#[tokio::test]
async fn default_record_dispatch() {
    assert_eval(
        r#"
        type Foo = Foo { x: i32, y: i32 } | Bar { z: f32 }

        instance Default Foo
            default = Bar { z = 0.0 }

        let x: Foo = default in x
        "#,
        "Bar {z = 0f32}",
        Type::con("Foo", 0),
    )
    .await;
}

#[tokio::test]
async fn default_nested_context_list() {
    assert_eval(
        r#"
        let xs: List i32 = default in xs
        "#,
        "[]",
        Type::list(Type::builtin(BuiltinTypeId::I32)),
    )
    .await;
}

#[tokio::test]
async fn pattern_field_renaming() {
    assert_eval(
        r#"
        type Point = Point { x: f32, y: f32 }

        instance AdditiveMonoid Point
            zero = Point { x = 0.0, y = 0.0 }
            + = \p q -> match (p, q)
                when (Point { x: x1, y: y1 }, Point { x: x2, y: y2 }) ->
                    Point { x = x1 + x2, y = y1 + y2 }

        (Point { x = 1.0, y = 2.0 }) + (Point { x = 3.0, y = 4.0 })
        "#,
        "Point {x = 4f32, y = 6f32}",
        Type::con("Point", 0),
    )
    .await;
}

#[tokio::test]
async fn default_nested_context_option() {
    assert_eval(
        r#"
        let x: Option i32 = default in x
        "#,
        "None",
        Type::option(Type::builtin(BuiltinTypeId::I32)),
    )
    .await;
}

#[tokio::test]
async fn default_custom_adt_single_ctor_unnamed_fields() {
    assert_eval(
        r#"
        type Pair = Pair i32 bool

        instance Default Pair
            default = Pair 42 true

        let x: Pair = default in x
        "#,
        "Pair 42i32 true",
        Type::con("Pair", 0),
    )
    .await;
}

#[tokio::test]
async fn default_custom_adt_single_ctor_named_fields() {
    assert_eval(
        r#"
        type Config = Config { retries: i32, enabled: bool }

        instance Default Config
            default = Config { retries = 3, enabled = false }

        let x: Config = default in x
        "#,
        "Config {enabled = false, retries = 3i32}",
        Type::con("Config", 0),
    )
    .await;
}

#[tokio::test]
async fn default_custom_adt_enum_unit_variants() {
    assert_eval(
        r#"
        type Mode = Fast | Safe | Debug

        instance Default Mode
            default = Safe

        let x: Mode = default in x
        "#,
        "Safe",
        Type::con("Mode", 0),
    )
    .await;
}

#[tokio::test]
async fn default_custom_adt_enum_mixed_variant_payloads() {
    assert_eval(
        r#"
        type Token = Eof | IntLit i32 | Meta { line: i32, col: i32 }

        instance Default Token
            default = Meta { line = 1, col = 1 }

        let x: Token = default in x
        "#,
        "Meta {col = 1i32, line = 1i32}",
        Type::con("Token", 0),
    )
    .await;
}

#[tokio::test]
async fn default_custom_adt_generic_instance_uses_constraint() {
    assert_eval(
        r#"
        type Box a = Box a | Missing

        instance Default (Box a) <= Default a
            default = Box default

        let x: Box i32 = default in x
        "#,
        "Box 0i32",
        Type::app(Type::con("Box", 1), Type::builtin(BuiltinTypeId::I32)),
    )
    .await;
}

#[tokio::test]
async fn default_multiple_adts_same_named_fields_then_record_update_without_is_fails() {
    // Without contextual type information, `default` is still polymorphic at
    // `{ default with ... }`, so record update cannot prove field availability.
    assert_err_contains(
        r#"
        type A = A { x: i32, y: i32 }
        type B = B { x: i32, y: i32 }

        instance Default A
            default = A { x = 1, y = 2 }

        instance Default B
            default = B { x = 10, y = 20 }

        let
            a = { default with { x = 9 } },
            b = { default with { y = 8 } }
        in
            (a, b)
        "#,
        "field `x` is not definitely available",
    )
    .await;
}

#[tokio::test]
async fn default_multiple_adts_same_named_fields_then_record_update_uses_let_annotations() {
    // The `let` annotations provide expected types (`A` and `B`), so record
    // updates can resolve `default` without requiring explicit `is`.
    assert_eval(
        r#"
        type A = A { x: i32, y: i32 }
        type B = B { x: i32, y: i32 }

        instance Default A
            default = A { x = 1, y = 2 }

        instance Default B
            default = B { x = 10, y = 20 }

        let
            a: A = { default with { x = 9 } },
            b: B = { default with { y = 8 } }
        in
            (a, b)
        "#,
        "(A {x = 9i32, y = 2i32}, B {x = 10i32, y = 8i32})",
        Type::tuple(vec![Type::con("A", 0), Type::con("B", 0)]),
    )
    .await;
}

#[tokio::test]
async fn default_multiple_adts_same_named_fields_then_record_update() {
    // Even with shared field names, this works because each `default` call is
    // explicitly pinned to a concrete ADT (`A`/`B`) before record update.
    assert_eval(
        r#"
        type A = A { x: i32, y: i32 }
        type B = B { x: i32, y: i32 }

        instance Default A
            default = A { x = 1, y = 2 }

        instance Default B
            default = B { x = 10, y = 20 }

        let
            a: A = { (default is A) with { x = 9 } },
            b: B = { (default is B) with { y = 8 } }
        in
            (a, b)
        "#,
        "(A {x = 9i32, y = 2i32}, B {x = 10i32, y = 8i32})",
        Type::tuple(vec![Type::con("A", 0), Type::con("B", 0)]),
    )
    .await;
}

#[tokio::test]
async fn default_multiple_adts_same_named_fields_with_is_disambiguates_without_let_types() {
    // `is` is necessary here to choose which `Default` instance to use before
    // record update checks field availability. Without it, the base type stays
    // ambiguous even though `A` and `B` share the same field names.
    assert_eval(
        r#"
        type A = A { x: i32, y: i32 }
        type B = B { x: i32, y: i32 }

        instance Default A
            default = A { x = 1, y = 2 }

        instance Default B
            default = B { x = 10, y = 20 }

        let
            a = { (default is A) with { x = 9 } },
            b = { (default is B) with { y = 8 } }
        in
            (a, b)
        "#,
        "(A {x = 9i32, y = 2i32}, B {x = 10i32, y = 8i32})",
        Type::tuple(vec![Type::con("A", 0), Type::con("B", 0)]),
    )
    .await;
}

#[tokio::test]
async fn methods_can_call_other_methods() {
    assert_eval(
        r#"
        class PairOps p
            first : p -> i32
            second : p -> i32
            sum_pair : p -> i32

        type Pair = Pair { a: i32, b: i32 }

        instance PairOps Pair
            first = \p -> p.a
            second = \p -> p.b
            sum_pair = \p -> (first p) + (second p)

        sum_pair (Pair { a = 19, b = 23 })
        "#,
        "42i32",
        Type::builtin(BuiltinTypeId::I32),
    )
    .await;
}

#[tokio::test]
async fn method_can_return_function() {
    assert_eval(
        r#"
        class Builder a
            make_adder : a -> i32 -> i32

        instance Builder i32
            make_adder = \n x -> x + n

        let f = make_adder (5 is i32) in f (37 is i32)
        "#,
        "42i32",
        Type::builtin(BuiltinTypeId::I32),
    )
    .await;
}

#[tokio::test]
async fn instance_method_can_reference_global_fn() {
    assert_eval(
        r#"
        fn inc (x: i32) -> i32 = x + 1

        class Bump a
            bump : a -> a

        instance Bump i32
            bump = inc

        bump 41
        "#,
        "42i32",
        Type::builtin(BuiltinTypeId::I32),
    )
    .await;
}

#[tokio::test]
async fn hkt_functor_option_and_result() {
    assert_eval(
        r#"
        class MyFunctor f
            fmap : (a -> b) -> f a -> f b

        instance MyFunctor Option
            fmap = \f x ->
                match x
                    when Some v -> Some (f v)
                    when None -> None

        instance MyFunctor (Result e)
            fmap = \f x ->
                match x
                    when Ok v -> Ok (f v)
                    when Err err -> Err err

        let
            inc = \x -> x + 1,
            a = fmap inc (Some 1),
            b = fmap inc (None is Option i32),
            c = fmap inc ((Ok 1) is Result i32 string),
            d = fmap inc ((Err "bad") is Result i32 string)
        in
            (a, b, c, d)
        "#,
        r#"(Some 2i32, None, Ok 2i32, Err "bad")"#,
        Type::tuple(vec![
            Type::option(Type::builtin(BuiltinTypeId::I32)),
            Type::option(Type::builtin(BuiltinTypeId::I32)),
            Type::result(
                Type::builtin(BuiltinTypeId::I32),
                Type::builtin(BuiltinTypeId::String),
            ),
            Type::result(
                Type::builtin(BuiltinTypeId::I32),
                Type::builtin(BuiltinTypeId::String),
            ),
        ]),
    )
    .await;
}

#[tokio::test]
async fn pattern_match_inside_method_body() {
    assert_eval(
        r#"
        class Head a
            head_or : a -> List a -> a

        instance Head i32
            head_or = \fallback xs ->
                match xs
                    when [] -> fallback
                    when x::rest -> x

        (head_or 0 [1, 2, 3], head_or 7 [])
        "#,
        "(1i32, 7i32)",
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32),
        ]),
    )
    .await;
}

#[tokio::test]
async fn superclass_and_instance_context() {
    assert_eval(
        r#"
        class MyEq a
            eq : a -> a -> bool

        class MyOrd a <= MyEq a
            my_cmp : a -> a -> i32

        type Color = Red | Green | Blue

        instance MyEq Color
            eq = \x y ->
                match x
                    when Red ->
                        let r = match y when Red -> true when _ -> false in r
                    when Green ->
                        let r = match y when Green -> true when _ -> false in r
                    when Blue ->
                        let r = match y when Blue -> true when _ -> false in r

        instance MyOrd Color <= MyEq Color
            my_cmp = \x y ->
                if eq x y then 0 else
                match x
                    when Red -> -1
                    when Green -> if eq y Red then 1 else -1
                    when Blue -> 1

        (eq Red Blue, eq Blue Blue, my_cmp Red Green, my_cmp Blue Red)
        "#,
        "(false, true, -1i32, 1i32)",
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::Bool),
            Type::builtin(BuiltinTypeId::Bool),
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32),
        ]),
    )
    .await;
}

#[tokio::test]
async fn missing_instance_method_is_error() {
    assert_err_contains(
        r#"
        class NeedsMethod a
            needs : a

        instance NeedsMethod i32
        0
        "#,
        "missing implementation of `needs`",
    )
    .await;
}

#[tokio::test]
async fn unknown_instance_method_is_error() {
    assert_err_contains(
        r#"
        class NeedsMethod a
            needs : a

        instance NeedsMethod i32
            not_a_method = 0
        0
        "#,
        "unknown method `not_a_method`",
    )
    .await;
}

#[tokio::test]
async fn missing_instance_constraint_is_error() {
    assert_err_contains(
        r#"
        class NeedsCtx a
            make : a

        instance NeedsCtx (List a)
            make = [make]
        0
        "#,
        "not in the instance context",
    )
    .await;
}

#[tokio::test]
async fn duplicate_instances_are_rejected() {
    assert_err_contains(
        r#"
        class Dup a
            dup : a

        instance Dup i32
            dup = 0

        instance Dup i32
            dup = 1

        0
        "#,
        "duplicate type class instance",
    )
    .await;
}

#[tokio::test]
async fn ambiguous_class_method_use_is_error() {
    assert_err_contains(
        r#"
        class Pick a
            pick : a

        instance Pick i32
            pick = 0

        pick
        "#,
        "ambiguous overload",
    )
    .await;
}
