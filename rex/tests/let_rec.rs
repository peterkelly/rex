mod common;

use rex::{
    engine::{Builder, Value},
    typesystem::{BuiltinTypeId, Type, TypeKind},
};

#[tokio::test]
async fn let_rec_self_recursive_factorial() {
    let (_heap, handle, ty) = common::eval_source(
        Builder::with_prelude(()).unwrap(),
        r#"
        let rec
            fact = \n ->
                if n == 0
                    then
                        1
                    else
                        n * fact (n - 1)
        in
            fact 6
    "#,
    )
    .await
    .unwrap();
    common::assert_i32_or_var(&ty);
    let expected = Value::I32(720);
    common::assert_handles_eq(&handle, &expected);
}

#[tokio::test]
async fn let_rec_self_recursive_fibonacci() {
    let (_heap, handle, ty) = common::eval_source(
        Builder::with_prelude(()).unwrap(),
        r#"
        let rec
            fib = \n ->
                if n <= 1
                    then n
                else
                    fib (n - 1) + fib (n - 2)
        in
            fib 8
    "#,
    )
    .await
    .unwrap();
    common::assert_i32_or_var(&ty);
    let expected = Value::I32(21);
    common::assert_handles_eq(&handle, &expected);
}

#[tokio::test]
async fn let_rec_mutual_even_odd() {
    let (_heap, handle, ty) = common::eval_source(
        Builder::with_prelude(()).unwrap(),
        r#"
        let rec
            even = \n -> if n == 0 then true else odd (n - 1),
            odd = \n -> if n == 0 then false else even (n - 1)
        in
            (even 10, odd 10, even 11, odd 11)
    "#,
    )
    .await
    .unwrap();
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::Bool),
            Type::builtin(BuiltinTypeId::Bool),
            Type::builtin(BuiltinTypeId::Bool),
            Type::builtin(BuiltinTypeId::Bool),
        ])
    );

    let expected = Value::Tuple(vec![
        Value::Bool(true),
        Value::Bool(false),
        Value::Bool(false),
        Value::Bool(true),
    ]);
    common::assert_handles_eq(&handle, &expected);
}

#[tokio::test]
async fn let_rec_mutual_three_function_group() {
    let (_heap, handle, ty) = common::eval_source(
        Builder::with_prelude(()).unwrap(),
        r#"
        let rec
            step0 = \n -> if n == 0 then 0 else step1 (n - 1),
            step1 = \n -> if n == 0 then 1 else step2 (n - 1),
            step2 = \n -> if n == 0 then 2 else step0 (n - 1)
        in
            (step0 3, step1 3, step2 3)
    "#,
    )
    .await
    .unwrap();
    let TypeKind::Tuple(items) = ty.as_ref() else {
        panic!("expected tuple type, got {ty}");
    };
    assert_eq!(items.len(), 3);
    for item in items {
        common::assert_i32_or_var(item);
    }

    let expected = Value::Tuple(vec![Value::I32(0), Value::I32(1), Value::I32(2)]);
    common::assert_handles_eq(&handle, &expected);
}

#[tokio::test]
async fn let_rec_function_is_still_polymorphic() {
    let (_heap, handle, ty) = common::eval_source(
        Builder::with_prelude(()).unwrap(),
        r#"
        let rec
            id = \x -> x
        in
            (id 1, id true)
    "#,
    )
    .await
    .unwrap();
    let TypeKind::Tuple(items) = ty.as_ref() else {
        panic!("expected tuple type, got {ty}");
    };
    assert_eq!(items.len(), 2);
    common::assert_i32_or_var(&items[0]);
    assert_eq!(items[1], Type::builtin(BuiltinTypeId::Bool));
    let expected = Value::Tuple(vec![Value::I32(1), Value::Bool(true)]);
    common::assert_handles_eq(&handle, &expected);
}

#[tokio::test]
async fn let_rec_allows_sequential_value_bindings() {
    let (_heap, handle, ty) = common::eval_source(
        Builder::with_prelude(()).unwrap(),
        r#"
        let rec
            x = 1,
            y = x + 1
        in
            y
    "#,
    )
    .await
    .unwrap();
    common::assert_i32_or_var(&ty);
    let expected = Value::I32(2);
    common::assert_handles_eq(&handle, &expected);
}

#[tokio::test]
async fn let_rec_value_may_reference_earlier_function() {
    let (_heap, handle, ty) = common::eval_source(
        Builder::with_prelude(()).unwrap(),
        r#"
        let rec
            inc = \n -> n + 1,
            y = inc 4
        in
            y
    "#,
    )
    .await
    .unwrap();
    common::assert_i32_or_var(&ty);
    let expected = Value::I32(5);
    common::assert_handles_eq(&handle, &expected);
}

#[tokio::test]
async fn let_rec_function_may_reference_later_value() {
    let (_heap, handle, ty) = common::eval_source(
        Builder::with_prelude(()).unwrap(),
        r#"
        let rec
            f = \_ -> x,
            x = 41
        in
            f 0
    "#,
    )
    .await
    .unwrap();
    common::assert_i32_or_var(&ty);
    let expected = Value::I32(41);
    common::assert_handles_eq(&handle, &expected);
}

#[tokio::test]
async fn let_rec_rejects_self_referential_data_cycles() {
    common::assert_invalid_let_rec_value_dependency(
        r#"
        let rec
            xs = Cons 1 xs
        in
            xs
    "#,
        "xs",
        "xs",
    )
    .await
}

#[tokio::test]
async fn let_rec_rejects_mutual_data_cycles() {
    common::assert_invalid_let_rec_value_dependency(
        r#"
        let rec
            a = Cons 1 b,
            b = Cons 2 a
        in
            (a, b)
    "#,
        "a",
        "b",
    )
    .await
}

#[tokio::test]
async fn let_rec_rejects_forward_value_reference() {
    common::assert_invalid_let_rec_value_dependency(
        r#"
        let rec
            x = y,
            y = 1
        in
            x
    "#,
        "x",
        "y",
    )
    .await
}

#[tokio::test]
async fn let_rec_rejects_value_reference_to_later_function() {
    common::assert_invalid_let_rec_value_dependency(
        r#"
        let rec
            x = f 1,
            f = \n -> n
        in
            x
    "#,
        "x",
        "f",
    )
    .await
}

#[tokio::test]
async fn let_rec_rejects_value_calling_function_that_reaches_uninitialized_function() {
    common::assert_invalid_let_rec_value_dependency(
        r#"
        let rec
            f = \n -> g n,
            x = f 4,
            g = \n -> n + 1
        in
            x
    "#,
        "x",
        "g",
    )
    .await
}

#[tokio::test]
async fn let_rec_rejects_value_calling_function_that_reaches_uninitialized_value() {
    common::assert_invalid_let_rec_value_dependency(
        r#"
        let rec
            f = \_ -> x,
            x = f 0
        in
            x
    "#,
        "x",
        "x",
    )
    .await
}
