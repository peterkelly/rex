mod common;

use rex::{
    engine::Builder,
    typesystem::{BuiltinTypeId, Type},
};

// These tests pin the language rule that top-level declaration order is not
// semantically meaningful. The old registration/injection pipeline walked
// declarations once and used source order as an implicit phase schedule: only
// contiguous `fn`s were mutually recursive, ADTs/classes had to appear before
// annotations that mentioned them, and instances had to follow their classes.
// Each pair below keeps the same declarations and changes only their order.

async fn assert_i32_result(source: &str, expected: i32) {
    let (heap, handle, ty) = common::eval_source(Builder::with_prelude(()).unwrap(), source)
        .await
        .unwrap();
    common::assert_i32_or_var(&ty);
    let expected = heap.alloc_i32(expected).unwrap();
    common::assert_handles_eq(&handle, &expected);
}

async fn assert_even_odd_tuple(source: &str) {
    let bool_ty = Type::builtin(BuiltinTypeId::Bool);
    let expected_ty = Type::tuple(vec![
        bool_ty.clone(),
        bool_ty.clone(),
        bool_ty.clone(),
        bool_ty,
    ]);
    let (heap, handle, ty) = common::eval_source(Builder::with_prelude(()).unwrap(), source)
        .await
        .unwrap();
    assert_eq!(
        ty, expected_ty,
        "eval returned unexpected type for: {source}"
    );
    let t0 = heap.alloc_bool(true).unwrap();
    let t1 = heap.alloc_bool(false).unwrap();
    let t2 = heap.alloc_bool(false).unwrap();
    let t3 = heap.alloc_bool(true).unwrap();
    let expected = heap.alloc_tuple(vec![t0, t1, t2, t3]).unwrap();
    common::assert_handles_eq(&handle, &expected);
}

#[tokio::test]
async fn top_level_functions_are_mutually_recursive_when_contiguous() {
    assert_even_odd_tuple(
        r#"
        fn even (n: i32) -> bool = if n == 0 then true else odd (n - 1);
        fn odd (n: i32) -> bool = if n == 0 then false else even (n - 1);
        (even 10, odd 10, even 11, odd 11)
        "#,
    )
    .await;
}

// Regression: a non-`fn` declaration used to split the top-level recursive
// function group, making `even` unable to see the later `odd` declaration.
#[tokio::test]
async fn top_level_functions_are_mutually_recursive_when_split_by_type_decl() {
    assert_even_odd_tuple(
        r#"
        fn even (n: i32) -> bool = if n == 0 then true else odd (n - 1);
        type Marker = Marker;
        fn odd (n: i32) -> bool = if n == 0 then false else even (n - 1);
        (even 10, odd 10, even 11, odd 11)
        "#,
    )
    .await;
}

#[tokio::test]
async fn adt_declarations_can_reference_prior_adts() {
    assert_i32_result(
        r#"
        type Later = Later;
        type Box = Box Later;
        match Box Later with { case Box Later -> 1; }
        "#,
        1,
    )
    .await;
}

// Regression: ADT bodies used to resolve field types against only the ADTs
// registered earlier in source order, so `Box Later` failed before `Later`.
#[tokio::test]
async fn adt_declarations_can_reference_later_adts() {
    assert_i32_result(
        r#"
        type Box = Box Later;
        type Later = Later;
        match Box Later with { case Box Later -> 1; }
        "#,
        1,
    )
    .await;
}

#[tokio::test]
async fn function_annotations_can_reference_prior_adts() {
    assert_i32_result(
        r#"
        type Later = Later;
        fn id_later (x: Later) -> Later = x;
        match id_later Later with { case Later -> 1; }
        "#,
        1,
    )
    .await;
}

// Regression: function annotations used to resolve type names before later
// local ADT declarations were known.
#[tokio::test]
async fn function_annotations_can_reference_later_adts() {
    assert_i32_result(
        r#"
        fn id_later (x: Later) -> Later = x;
        type Later = Later;
        match id_later Later with { case Later -> 1; }
        "#,
        1,
    )
    .await;
}

#[tokio::test]
async fn function_constraints_can_reference_prior_classes() {
    assert_i32_result(
        r#"
        class C a where { c : a; }
        fn f<a> (x: a) -> a where C a = x;
        1
        "#,
        1,
    )
    .await;
}

// Regression: `where` constraints on function declarations used to require the
// class declaration to have appeared earlier in the file.
#[tokio::test]
async fn function_constraints_can_reference_later_classes() {
    assert_i32_result(
        r#"
        fn f<a> (x: a) -> a where C a = x;
        class C a where { c : a; }
        1
        "#,
        1,
    )
    .await;
}

#[tokio::test]
async fn instances_can_reference_prior_classes() {
    assert_i32_result(
        r#"
        class C a where { c : a; }
        instance C i32 where { c = 7; }
        let x : i32 = c in x
        "#,
        7,
    )
    .await;
}

// Regression: instance headers used to be validated before later class
// declarations were registered.
#[tokio::test]
async fn instances_can_reference_later_classes() {
    assert_i32_result(
        r#"
        instance C i32 where { c = 7; }
        class C a where { c : a; }
        let x : i32 = c in x
        "#,
        7,
    )
    .await;
}
