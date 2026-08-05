mod common;

use rex::typesystem::{BuiltinTypeId, Type};

use common::assert_eval_display as assert_eval;

fn builtin(id: BuiltinTypeId) -> Type {
    Type::builtin(id)
}

#[tokio::test]
async fn string_get_and_slice_use_unicode_scalar_indices() {
    let option_char = Type::option(builtin(BuiltinTypeId::Char));
    assert_eval(
        r#"
        ( string_get 0 "a😀éz"
        , string_get 1 "a😀éz"
        , string_get 3 "a😀éz"
        , string_get 4 "a😀éz"
        )
        "#,
        "(Some 'a', Some '😀', Some 'z', None)",
        Type::tuple(vec![
            option_char.clone(),
            option_char.clone(),
            option_char.clone(),
            option_char,
        ]),
    )
    .await;

    let option_string = Type::option(builtin(BuiltinTypeId::String));
    assert_eval(
        r#"
        ( string_slice 1 3 "a😀éz"
        , string_slice 4 4 "a😀éz"
        , string_slice 4 5 "a😀éz"
        , string_slice 3 2 "a😀éz"
        )
        "#,
        r#"(Some "😀é", Some "", None, None)"#,
        Type::tuple(vec![
            option_string.clone(),
            option_string.clone(),
            option_string.clone(),
            option_string,
        ]),
    )
    .await;
}

#[tokio::test]
async fn string_search_predicates_use_query_first_order() {
    let bool_ty = builtin(BuiltinTypeId::Bool);
    let option_u64 = Type::option(builtin(BuiltinTypeId::U64));
    assert_eval(
        r#"
        ( string_contains "😀é" "a😀éz"
        , string_contains "é😀" "a😀éz"
        , string_starts_with "a😀" "a😀éz"
        , string_ends_with "éz" "a😀éz"
        , string_find "😀" "é😀z"
        , string_find "z" "é😀z"
        , string_find "missing" "é😀z"
        , string_find "" "é😀z"
        )
        "#,
        "(true, false, true, true, Some 1u64, Some 2u64, None, Some 0u64)",
        Type::tuple(vec![
            bool_ty.clone(),
            bool_ty.clone(),
            bool_ty.clone(),
            bool_ty,
            option_u64.clone(),
            option_u64.clone(),
            option_u64.clone(),
            option_u64,
        ]),
    )
    .await;
}

#[tokio::test]
async fn string_split_join_and_replace_preserve_empty_segments() {
    let string_ty = builtin(BuiltinTypeId::String);
    let list_string = Type::list(string_ty.clone());
    assert_eval(
        r#"
        ( string_split "," "a,,b,"
        , string_split "" "a😀"
        , string_join ":" ["a", "😀", ""]
        , string_join "," []
        , string_replace "na" "X" "banana"
        , string_replace "" "." "a😀"
        )
        "#,
        r#"(["a", "", "b", ""], ["", "a", "😀", ""], "a:😀:", "", "baXX", ".a.😀.")"#,
        Type::tuple(vec![
            list_string.clone(),
            list_string,
            string_ty.clone(),
            string_ty.clone(),
            string_ty.clone(),
            string_ty,
        ]),
    )
    .await;
}

#[tokio::test]
async fn string_trimming_and_case_conversion_use_unicode_rules() {
    let string_ty = builtin(BuiltinTypeId::String);
    assert_eval(
        r#"
        ( string_trim "\u2003 Hello \n"
        , string_trim_start "\t x \n"
        , string_trim_end "\t x \u2003"
        , string_to_lower "İΣA"
        , string_to_upper "Straße"
        )
        "#,
        r#"("Hello", "x \n", "\t x", "i\u{307}σa", "STRASSE")"#,
        Type::tuple(vec![
            string_ty.clone(),
            string_ty.clone(),
            string_ty.clone(),
            string_ty.clone(),
            string_ty,
        ]),
    )
    .await;
}

#[tokio::test]
async fn strings_and_character_lists_convert_in_both_directions() {
    let string_ty = builtin(BuiltinTypeId::String);
    let list_char = Type::list(builtin(BuiltinTypeId::Char));
    assert_eval(
        r#"
        ( string_to_chars "a😀é"
        , string_to_chars ""
        , chars_to_string ['a', '😀', 'é']
        , chars_to_string []
        )
        "#,
        r#"(['a', '😀', 'é'], [], "a😀é", "")"#,
        Type::tuple(vec![
            list_char.clone(),
            list_char,
            string_ty.clone(),
            string_ty,
        ]),
    )
    .await;
}

#[tokio::test]
async fn strings_and_utf8_bytes_convert_with_validation() {
    let string_ty = builtin(BuiltinTypeId::String);
    let list_u8 = Type::list(builtin(BuiltinTypeId::U8));
    let option_string = Type::option(string_ty);
    assert_eval(
        r#"
        let bytes = string_to_utf8 "Aé😀" in
        ( bytes
        , utf8_to_string bytes
        , utf8_to_string [(195 is u8), (40 is u8)]
        , utf8_to_string []
        )
        "#,
        r#"([65u8, 195u8, 169u8, 240u8, 159u8, 152u8, 128u8], Some "Aé😀", None, Some "")"#,
        Type::tuple(vec![
            list_u8,
            option_string.clone(),
            option_string.clone(),
            option_string,
        ]),
    )
    .await;
}
