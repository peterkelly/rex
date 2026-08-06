mod common;

use rex::typesystem::{BuiltinTypeId, Type};

use common::assert_eval_display as assert_eval;

fn i32_type() -> Type {
    Type::builtin(BuiltinTypeId::I32)
}

fn i32_list_type() -> Type {
    Type::list(i32_type())
}

#[tokio::test]
async fn list_get_and_slice_return_options_for_invalid_bounds() {
    let option_i32 = Type::option(i32_type());
    let option_list = Type::option(i32_list_type());
    assert_eval(
        r#"
        let xs = ([10, 20, 30] is List i32) in
            ( list_get 0 xs
            , list_get 2 xs
            , list_get 3 xs
            , list_slice 1 3 xs
            , list_slice 3 3 xs
            , list_slice 3 4 xs
            , list_slice 2 1 xs
            )
        "#,
        "(Some 10i32, Some 30i32, None, Some [20i32, 30i32], Some [], None, None)",
        Type::tuple(vec![
            option_i32.clone(),
            option_i32.clone(),
            option_i32,
            option_list.clone(),
            option_list.clone(),
            option_list.clone(),
            option_list,
        ]),
    )
    .await;
}

#[tokio::test]
async fn list_reverse_concat_and_repeat_preserve_order() {
    assert_eval(
        r#"
        ( list_reverse ([1, 2, 3, 4] is List i32)
        , list_reverse ([] is List i32)
        , list_concat ([[1, 2], [], [3], [4, 5]] is List (List i32))
        , list_repeat 4 (7 is i32)
        , list_repeat 0 (9 is i32)
        )
        "#,
        "([4i32, 3i32, 2i32, 1i32], [], [1i32, 2i32, 3i32, 4i32, 5i32], [7i32, 7i32, 7i32, 7i32], [])",
        Type::tuple(vec![
            i32_list_type(),
            i32_list_type(),
            i32_list_type(),
            i32_list_type(),
            i32_list_type(),
        ]),
    )
    .await;
}

#[tokio::test]
async fn list_any_and_all_short_circuit_left_to_right() {
    assert_eval(
        r#"
        let
            any_predicate = \x ->
                if x == 2 then true
                else if x == 3 then unwrap (None is Option Bool)
                else false,
            all_predicate = \x ->
                if x == 2 then false
                else if x == 3 then unwrap (None is Option Bool)
                else true
        in
            ( list_any any_predicate [1, 2, 3]
            , list_all all_predicate [1, 2, 3]
            , list_any (\x -> x > 0) ([] is List i32)
            , list_all (\x -> x > 0) ([] is List i32)
            )
        "#,
        "(true, false, false, true)",
        Type::tuple(vec![Type::builtin(BuiltinTypeId::Bool); 4]),
    )
    .await;
}

#[tokio::test]
async fn list_find_and_find_index_return_the_first_match() {
    assert_eval(
        r#"
        let
            predicate = \x ->
                if x == 20 then true
                else if x == 30 then unwrap (None is Option Bool)
                else false
        in
            ( list_find predicate [10, 20, 30]
            , list_find_index predicate [10, 20, 30]
            , list_find (\x -> x > 100) [10, 20, 30]
            , list_find_index (\x -> x > 100) [10, 20, 30]
            )
        "#,
        "(Some 20i32, Some 1u64, None, None)",
        Type::tuple(vec![
            Type::option(i32_type()),
            Type::option(Type::builtin(BuiltinTypeId::U64)),
            Type::option(i32_type()),
            Type::option(Type::builtin(BuiltinTypeId::U64)),
        ]),
    )
    .await;
}

#[tokio::test]
async fn list_count_and_partition_evaluate_every_element() {
    assert_eval(
        r#"
        let xs = ([1, 2, 3, 4, 5, 6] is List i32) in
        ( list_count (\x -> x % 2 == 0) xs
        , list_count (\x -> x > 0) ([] is List i32)
        , list_partition (\x -> x % 2 == 0) xs
        , list_partition (\x -> x > 0) ([] is List i32)
        )
        "#,
        "(3u64, 0u64, ([2i32, 4i32, 6i32], [1i32, 3i32, 5i32]), ([], []))",
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::U64),
            Type::builtin(BuiltinTypeId::U64),
            Type::tuple(vec![i32_list_type(), i32_list_type()]),
            Type::tuple(vec![i32_list_type(), i32_list_type()]),
        ]),
    )
    .await;
}
