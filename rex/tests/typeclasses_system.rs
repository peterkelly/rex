use rex::{Engine, GasCosts, GasMeter, Parser, Token, value_display};

async fn eval_to_string(code: &str) -> Result<String, String> {
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
    let pointer = engine
        .eval(program.expr.as_ref(), &mut gas)
        .await
        .map_err(|e| format!("{e}"))?;
    let value = engine.heap().get(&pointer).map_err(|e| format!("{e}"))?;
    value_display(engine.heap(), value.as_ref()).map_err(|e| format!("{e}"))
}

async fn assert_eval(code: &str, expected: &str) {
    let actual = eval_to_string(code)
        .await
        .unwrap_or_else(|e| panic!("expected ok, got error: {e}"));
    assert_eq!(actual, expected);
}

async fn assert_err_contains(code: &str, needle: &str) {
    let err = eval_to_string(code).await.unwrap_err();
    assert!(
        err.contains(needle),
        "expected error containing {needle:?}, got: {err}"
    );
}

#[tokio::test]
async fn default_record_dispatch() {
    assert_eval(
        r#"
        class Default a
            default : a

        type Foo = Foo { x: i32, y: i32 } | Bar { z: f32 }

        instance Default Foo
            default = Bar { z = 0.0 }

        let x: Foo = default in x
        "#,
        "Bar {z = 0f32}",
    )
    .await;
}

#[tokio::test]
async fn default_nested_context_list() {
    assert_eval(
        r#"
        class Default a
            default : a

        instance Default i32
            default = 0

        instance Default (List a) <= Default a
            default = [default, default]

        let xs: List i32 = default in xs
        "#,
        "[0i32, 0i32]",
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
    )
    .await;
}

#[tokio::test]
async fn default_nested_context_option() {
    assert_eval(
        r#"
        class Default a
            default : a

        instance Default i32
            default = 0

        instance Default (Option a) <= Default a
            default = Some default

        let x: Option i32 = default in x
        "#,
        "Some 0i32",
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

        let f = make_adder 5 in f 37
        "#,
        "42i32",
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
                    when x:rest -> x

        (head_or 0 [1, 2, 3], head_or 7 [])
        "#,
        "(1i32, 7i32)",
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
    )
    .await;
}

#[tokio::test]
async fn missing_instance_method_is_error() {
    assert_err_contains(
        r#"
        class Default a
            default : a

        instance Default i32
        0
        "#,
        "missing implementation of `default`",
    )
    .await;
}

#[tokio::test]
async fn unknown_instance_method_is_error() {
    assert_err_contains(
        r#"
        class Default a
            default : a

        instance Default i32
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
        class Default a
            default : a

        instance Default (List a)
            default = [default]
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
        class Default a
            default : a

        instance Default i32
            default = 0

        instance Default i32
            default = 1

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
        class Default a
            default : a

        instance Default i32
            default = 0

        default
        "#,
        "ambiguous overload",
    )
    .await;
}
