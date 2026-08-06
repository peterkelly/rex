mod common;

use rex::{
    ast::Symbol,
    engine::{Builder, Value},
    typesystem::{BuiltinTypeId, Type},
};

use common::{
    assert_eval_display as assert_eval, assert_eval_error_contains as assert_err_contains,
    eval_source,
};

struct ParseCase {
    typ: BuiltinTypeId,
    valid: &'static str,
    expected: Value,
    invalid: &'static str,
}

fn supported_parse_cases() -> Vec<ParseCase> {
    const HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";
    const UUID: &str = "12345678-1234-5678-90ab-cdef12345678";
    const DATETIME: &str = "2024-02-29T12:34:56Z";

    vec![
        ParseCase {
            typ: BuiltinTypeId::Bool,
            valid: "true",
            expected: Value::Bool(true),
            invalid: "TRUE",
        },
        ParseCase {
            typ: BuiltinTypeId::Char,
            valid: "😀",
            expected: Value::Char('😀'),
            invalid: "",
        },
        ParseCase {
            typ: BuiltinTypeId::U8,
            valid: "255",
            expected: Value::U8(u8::MAX),
            invalid: "256",
        },
        ParseCase {
            typ: BuiltinTypeId::U16,
            valid: "65535",
            expected: Value::U16(u16::MAX),
            invalid: "65536",
        },
        ParseCase {
            typ: BuiltinTypeId::U32,
            valid: "4294967295",
            expected: Value::U32(u32::MAX),
            invalid: "4294967296",
        },
        ParseCase {
            typ: BuiltinTypeId::U64,
            valid: "18446744073709551615",
            expected: Value::U64(u64::MAX),
            invalid: "18446744073709551616",
        },
        ParseCase {
            typ: BuiltinTypeId::I8,
            valid: "-128",
            expected: Value::I8(i8::MIN),
            invalid: "128",
        },
        ParseCase {
            typ: BuiltinTypeId::I16,
            valid: "-32768",
            expected: Value::I16(i16::MIN),
            invalid: "32768",
        },
        ParseCase {
            typ: BuiltinTypeId::I32,
            valid: "-2147483648",
            expected: Value::I32(i32::MIN),
            invalid: "2147483648",
        },
        ParseCase {
            typ: BuiltinTypeId::I64,
            valid: "-9223372036854775808",
            expected: Value::I64(i64::MIN),
            invalid: "9223372036854775808",
        },
        ParseCase {
            typ: BuiltinTypeId::F32,
            valid: "3.5",
            expected: Value::F32(3.5),
            invalid: "three point five",
        },
        ParseCase {
            typ: BuiltinTypeId::F64,
            valid: "-2.25e100",
            expected: Value::F64(-2.25e100),
            invalid: "negative infinity?",
        },
        ParseCase {
            typ: BuiltinTypeId::Uuid,
            valid: UUID,
            expected: Value::Uuid(UUID.parse().unwrap()),
            invalid: "not-a-uuid",
        },
        ParseCase {
            typ: BuiltinTypeId::Hash,
            valid: HASH,
            expected: Value::Hash(blake3::Hash::from_hex(HASH).unwrap()),
            invalid: "000000000000000000000000000000000000000000000000000000000000000",
        },
        ParseCase {
            typ: BuiltinTypeId::DateTime,
            valid: DATETIME,
            expected: Value::DateTime(DATETIME.parse().unwrap()),
            invalid: "2023-02-29T12:34:56Z",
        },
    ]
}

async fn eval_parse(input: &str, typ: BuiltinTypeId) -> (Value, Type) {
    let source = format!(
        "let parsed: Option {} = parse {input:?} in parsed",
        typ.as_str()
    );
    let (_, value, actual_type) = eval_source(Builder::with_prelude(()).unwrap(), &source)
        .await
        .unwrap_or_else(|error| panic!("failed to evaluate `{source}`: {error}"));
    (value, actual_type)
}

#[tokio::test]
async fn parse_returns_some_for_every_supported_type() {
    for case in supported_parse_cases() {
        let (actual, actual_type) = eval_parse(case.valid, case.typ).await;
        assert_eq!(actual_type, Type::option(Type::builtin(case.typ)));
        assert_eq!(
            actual,
            Value::Adt(Symbol::intern("Some"), vec![case.expected]),
            "failed positive Parse case for {}",
            case.typ.as_str()
        );
    }
}

#[tokio::test]
async fn parse_returns_none_for_every_supported_type() {
    for case in supported_parse_cases() {
        let (actual, actual_type) = eval_parse(case.invalid, case.typ).await;
        assert_eq!(actual_type, Type::option(Type::builtin(case.typ)));
        assert_eq!(
            actual,
            Value::Adt(Symbol::intern("None"), vec![]),
            "failed negative Parse case for {}",
            case.typ.as_str()
        );
    }
}

#[tokio::test]
async fn parse_char_rejects_multiple_unicode_scalars() {
    let (actual, actual_type) = eval_parse("é😀", BuiltinTypeId::Char).await;
    assert_eq!(
        actual_type,
        Type::option(Type::builtin(BuiltinTypeId::Char))
    );
    assert_eq!(actual, Value::Adt(Symbol::intern("None"), vec![]));
}

#[tokio::test]
async fn default_record_dispatch() {
    assert_eval(
        r#"
        type Foo = Foo { x: i32, y: i32 } | Bar { z: f32 };

        instance Default Foo where {
            default = Bar { z = 0.0 };
        }
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
async fn additive_monoid_list_concatenates_in_order() {
    assert_eval(
        "[1, 2, 3] + [4, 5, 6]",
        "[1i32, 2i32, 3i32, 4i32, 5i32, 6i32]",
        Type::list(Type::builtin(BuiltinTypeId::I32)),
    )
    .await;
}

#[tokio::test]
async fn additive_monoid_list_requires_no_element_constraint() {
    assert_eval(
        r#"
        let empty: List Bool = zero in empty + [true, false]
        "#,
        "[true, false]",
        Type::list(Type::builtin(BuiltinTypeId::Bool)),
    )
    .await;
}

#[tokio::test]
async fn pattern_field_renaming() {
    assert_eval(
        r#"
        type Point = Point { x: f32, y: f32 };

        instance AdditiveMonoid Point where {
            zero = Point { x = 0.0, y = 0.0 };
            + = \p q -> match (p, q) with {
                case (Point { x: x1, y: y1 }, Point { x: x2, y: y2 }) ->
                    Point { x = x1 + x2, y = y1 + y2 };
            };
        }
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
        type Pair = Pair i32 Bool;

        instance Default Pair where {
            default = Pair 42 true;
        }
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
        type Config = Config { retries: i32, enabled: Bool };

        instance Default Config where {
            default = Config { retries = 3, enabled = false };
        }
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
        type Mode = Fast | Safe | Debug;

        instance Default Mode where {
            default = Safe;
        }
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
        type Token = Eof | IntLit i32 | Meta { line: i32, col: i32 };

        instance Default Token where {
            default = Meta { line = 1, col = 1 };
        }
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
        type Box a = Box a | Missing;

        instance<a> Default (Box a) <= Default a where {
            default = Box default;
        }
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
        type A = A { x: i32, y: i32 };
        type B = B { x: i32, y: i32 };

        instance Default A where {
            default = A { x = 1, y = 2 };
        }
        instance Default B where {
            default = B { x = 10, y = 20 };
        }
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
        type A = A { x: i32, y: i32 };
        type B = B { x: i32, y: i32 };

        instance Default A where {
            default = A { x = 1, y = 2 };
        }
        instance Default B where {
            default = B { x = 10, y = 20 };
        }
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
        type A = A { x: i32, y: i32 };
        type B = B { x: i32, y: i32 };

        instance Default A where {
            default = A { x = 1, y = 2 };
        }
        instance Default B where {
            default = B { x = 10, y = 20 };
        }
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
        type A = A { x: i32, y: i32 };
        type B = B { x: i32, y: i32 };

        instance Default A where {
            default = A { x = 1, y = 2 };
        }
        instance Default B where {
            default = B { x = 10, y = 20 };
        }
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
        class PairOps p where {
            pair_first : p -> i32;
            second : p -> i32;
            sum_pair : p -> i32;
        }
        type Pair = Pair { a: i32, b: i32 };

        instance PairOps Pair where {
            pair_first = \p -> p.a;
            second = \p -> p.b;
            sum_pair = \p -> (pair_first p) + (second p);
        }
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
        class Builder a where {
            make_adder : a -> i32 -> i32;
        }
        instance Builder i32 where {
            make_adder = \n x -> x + n;
        }
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
        fn inc (x: i32) -> i32 = x + 1;

        class Bump a where {
            bump : a -> a;
        }
        instance Bump i32 where {
            bump = inc;
        }
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
        class MyFunctor f where {
            fmap<a,b> : (a -> b) -> f a -> f b;
        }
        instance MyFunctor Option where {
            fmap = \f x ->
                match x with {
                    case Some v -> Some (f v);
                    case None -> None;
                };
        }
        instance<e> MyFunctor (Result e) where {
            fmap = \f x ->
                match x with {
                    case Ok v -> Ok (f v);
                    case Err err -> Err err;
                };
        }
        let
            inc = \x -> x + 1,
            a = fmap inc (Some 1),
            b = fmap inc (None is Option i32),
            c = fmap inc ((Ok 1) is Result i32 String),
            d = fmap inc ((Err "bad") is Result i32 String)
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
        class Head a where {
            head_or : a -> List a -> a;
        }
        instance Head i32 where {
            head_or = \fallback xs ->
                match xs with {
                    case [] -> fallback;
                    case x::rest -> x;
                };
        }
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
async fn ord_cmp_returns_ordering_variants() {
    let ordering_ty = Type::con("Ordering", 0);
    assert_eval(
        r#"
        [ cmp (1 is u8) (2 is u8)
        , cmp (2 is u16) (2 is u16)
        , cmp (3 is u32) (2 is u32)
        , cmp (1 is u64) (2 is u64)
        , cmp (2 is i8) (2 is i8)
        , cmp (3 is i16) (2 is i16)
        , cmp (1 is i32) (2 is i32)
        , cmp (2 is i64) (2 is i64)
        , cmp (3.0 is f32) (2.0 is f32)
        , cmp (1.0 is f64) (2.0 is f64)
        , cmp "same" "same"
        ]
        "#,
        "[Less, Equal, Greater, Less, Equal, Greater, Less, Equal, Greater, Less, Equal]",
        Type::list(ordering_ty),
    )
    .await;
}

#[tokio::test]
async fn ordering_variants_can_be_pattern_matched() {
    assert_eval(
        r#"
        fn label : Ordering -> String = \ordering ->
            match ordering with {
                case Less -> "less";
                case Equal -> "equal";
                case Greater -> "greater";
            };

        ( label (cmp "a" "b")
        , label (cmp (2.0 is f32) (2.0 is f32))
        , label (cmp (3 is i32) (2 is i32))
        )
        "#,
        r#"("less", "equal", "greater")"#,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::String),
            Type::builtin(BuiltinTypeId::String),
            Type::builtin(BuiltinTypeId::String),
        ]),
    )
    .await;
}

#[tokio::test]
async fn superclass_and_instance_context() {
    assert_eval(
        r#"
        class MyEq a where {
            eq : a -> a -> Bool;
        }
        class MyOrd a <= MyEq a where {
            my_cmp : a -> a -> i32;
        }
        type Color = Red | Green | Blue;

        instance MyEq Color where {
            eq = \x y ->
                match x with {
                    case Red ->
                        let r = match y with { case Red -> true; case _ -> false; } in r;
                    case Green ->
                        let r = match y with { case Green -> true; case _ -> false; } in r;
                    case Blue ->
                        let r = match y with { case Blue -> true; case _ -> false; } in r;
                };
        }
        instance MyOrd Color <= MyEq Color where {
            my_cmp = \x y ->
                if eq x y then 0 else
                match x with {
                    case Red -> -1;
                    case Green -> if eq y Red then 1 else -1;
                    case Blue -> 1;
                };
        }
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
        class NeedsMethod a where {
            needs : a;
        }
        instance NeedsMethod i32;
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
        class NeedsMethod a where {
            needs : a;
        }
        instance NeedsMethod i32 where {
            not_a_method = 0;
        }
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
        class NeedsCtx a where {
            make : a;
        }
        instance<a> NeedsCtx (List a) where {
            make = [make];
        }
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
        class Dup a where {
            dup : a;
        }
        instance Dup i32 where {
            dup = 0;
        }
        instance Dup i32 where {
            dup = 1;
        }
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
        class Pick a where {
            pick : a;
        }
        instance Pick i32 where {
            pick = 0;
        }
        pick
        "#,
        "ambiguous overload",
    )
    .await;
}
