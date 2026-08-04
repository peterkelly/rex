mod common;

use rex::typesystem::{BuiltinTypeId, Type};

use common::assert_eval_display as assert_eval;

fn i32_type() -> Type {
    Type::builtin(BuiltinTypeId::I32)
}

fn i32_dict_type() -> Type {
    Type::dict(i32_type())
}

#[tokio::test]
async fn dict_functor_and_filterable_instances_transform_values() {
    assert_eval(
        r#"
        let
            d = (({ a = 1, b = 2, c = 3 }) is Dict i32),
            mapped = map ((+) 10) d,
            filtered = filter (\value -> value >= 2) d,
            filtered_mapped =
                filter_map
                    (\value -> if value == 2 then None else Some (value * 2))
                    d
        in
            (mapped, filtered, filtered_mapped)
        "#,
        "({a = 11i32, b = 12i32, c = 13i32}, {b = 2i32, c = 3i32}, {a = 2i32, c = 6i32})",
        Type::tuple(vec![i32_dict_type(), i32_dict_type(), i32_dict_type()]),
    )
    .await;
}

#[tokio::test]
async fn dict_map_transforms_entries_and_resolves_collisions_in_input_key_order() {
    assert_eval(
        r#"
        dict_map
            (\entry ->
                match entry with {
                    case (key, value) -> ("same", key + ":" + show value);
                })
            (({ z = 3, a = 1, m = 2 }) is Dict i32)
        "#,
        r#"{same = "z:3"}"#,
        Type::dict(Type::builtin(BuiltinTypeId::String)),
    )
    .await;
}

#[tokio::test]
async fn dict_filter_can_inspect_keys_and_values() {
    assert_eval(
        r#"
        dict_filter
            (\entry ->
                match entry with {
                    case (key, value) -> key != "b" && value % 2 == 1;
                })
            (({ a = 1, b = 3, c = 2, d = 5 }) is Dict i32)
        "#,
        "{a = 1i32, d = 5i32}",
        i32_dict_type(),
    )
    .await;
}

#[tokio::test]
async fn dict_core_operations_are_immutable_and_option_based() {
    assert_eval(
        r#"
        let
            original = dict_singleton "a" 1,
            inserted = dict_insert "b" 2 original,
            replaced = dict_update "a" (\old -> map ((+) 10) old) inserted,
            removed = dict_remove "b" replaced,
            deleted = dict_update "a" (\_ -> None) removed
        in
            ( original
            , removed
            , dict_get "a" removed
            , dict_get "missing" removed
            , dict_has "b" removed
            , dict_is_empty deleted
            )
        "#,
        "({a = 1i32}, {a = 11i32}, Some 11i32, None, false, true)",
        Type::tuple(vec![
            i32_dict_type(),
            i32_dict_type(),
            Type::option(i32_type()),
            Type::option(i32_type()),
            Type::builtin(BuiltinTypeId::Bool),
            Type::builtin(BuiltinTypeId::Bool),
        ]),
    )
    .await;
}

#[tokio::test]
async fn dict_conversion_and_inspection_use_lexicographic_order_and_last_wins() {
    assert_eval(
        r#"
        let
            d = dict_from_entries
                [("z key", 1), ("a", 2), ("z key", 3), ("m", 4)]
        in
            (dict_keys d, dict_values d, dict_entries d, d)
        "#,
        "([\"a\", \"m\", \"z key\"], [2i32, 4i32, 3i32], [(\"a\", 2i32), (\"m\", 4i32), (\"z key\", 3i32)], {a = 2i32, m = 4i32, z key = 3i32})",
        Type::tuple(vec![
            Type::list(Type::builtin(BuiltinTypeId::String)),
            Type::list(i32_type()),
            Type::list(Type::tuple(vec![
                Type::builtin(BuiltinTypeId::String),
                i32_type(),
            ])),
            i32_dict_type(),
        ]),
    )
    .await;
}

#[tokio::test]
async fn dict_empty_is_polymorphic_in_context() {
    assert_eval(
        "let d: Dict i32 = dict_empty in (d, dict_is_empty d)",
        "({}, true)",
        Type::tuple(vec![i32_dict_type(), Type::builtin(BuiltinTypeId::Bool)]),
    )
    .await;
}
