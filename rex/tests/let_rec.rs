mod common;

use rex::{
    engine::{Engine, Value},
    typesystem::{BuiltinTypeId, Type, TypeKind},
};

#[tokio::test]
async fn let_rec_self_recursive_factorial() {
    let (heap, handle, ty) = common::eval_source(
        Engine::with_prelude(()).unwrap(),
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
    let expected = heap.alloc_i32(720).unwrap();
    common::assert_handles_eq(&handle, &expected);
}

#[tokio::test]
async fn let_rec_self_recursive_fibonacci() {
    let (heap, handle, ty) = common::eval_source(
        Engine::with_prelude(()).unwrap(),
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
    let expected = heap.alloc_i32(21).unwrap();
    common::assert_handles_eq(&handle, &expected);
}

#[tokio::test]
async fn let_rec_mutual_even_odd() {
    let (heap, handle, ty) = common::eval_source(
        Engine::with_prelude(()).unwrap(),
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

    let t0 = heap.alloc_bool(true).unwrap();
    let t1 = heap.alloc_bool(false).unwrap();
    let t2 = heap.alloc_bool(false).unwrap();
    let t3 = heap.alloc_bool(true).unwrap();
    let expected = heap.alloc_tuple(vec![t0, t1, t2, t3]).unwrap();
    common::assert_handles_eq(&handle, &expected);
}

#[tokio::test]
async fn let_rec_mutual_three_function_group() {
    let (heap, handle, ty) = common::eval_source(
        Engine::with_prelude(()).unwrap(),
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

    let a = heap.alloc_i32(0).unwrap();
    let b = heap.alloc_i32(1).unwrap();
    let c = heap.alloc_i32(2).unwrap();
    let expected = heap.alloc_tuple(vec![a, b, c]).unwrap();
    common::assert_handles_eq(&handle, &expected);
}

#[tokio::test]
async fn let_rec_function_is_still_polymorphic() {
    let (heap, handle, ty) = common::eval_source(
        Engine::with_prelude(()).unwrap(),
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
    let one = heap.alloc_i32(1).unwrap();
    let tru = heap.alloc_bool(true).unwrap();
    let expected = heap.alloc_tuple(vec![one, tru]).unwrap();
    common::assert_handles_eq(&handle, &expected);
}

#[tokio::test]
async fn let_rec_allows_self_referential_data_cycles() {
    let (_heap, handle, ty) = common::eval_source(
        Engine::with_prelude(()).unwrap(),
        r#"
        let rec
            xs = Cons 1 xs
        in
            xs
    "#,
    )
    .await
    .unwrap();
    assert_eq!(ty, Type::list(Type::builtin(BuiltinTypeId::I32)));
    let Value::Adt(tag, args) = handle.value().unwrap() else {
        panic!(
            "expected list constructor, got {}",
            handle.type_name().unwrap()
        );
    };
    assert_eq!(tag.as_ref(), "Cons");
    assert_eq!(args.len(), 2);
    common::assert_handles_eq(&handle, &args[1]);
}

#[tokio::test]
async fn let_rec_allows_mutual_data_cycles() {
    let (_heap, handle, ty) = common::eval_source(
        Engine::with_prelude(()).unwrap(),
        r#"
        let rec
            a = Cons 1 b,
            b = Cons 2 a
        in
            (a, b)
    "#,
    )
    .await
    .unwrap();
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::list(Type::builtin(BuiltinTypeId::I32)),
            Type::list(Type::builtin(BuiltinTypeId::I32)),
        ])
    );
    let Value::Tuple(items) = handle.value().unwrap() else {
        panic!("expected tuple, got {}", handle.type_name().unwrap());
    };
    assert_eq!(items.len(), 2);
    let a_handle = items[0].clone();
    let b_handle = items[1].clone();

    let Value::Adt(_, a_args) = a_handle.value().unwrap() else {
        panic!(
            "expected list constructor, got {}",
            a_handle.type_name().unwrap()
        );
    };
    assert_eq!(a_args.len(), 2);

    let Value::Adt(_, b_args) = b_handle.value().unwrap() else {
        panic!(
            "expected list constructor, got {}",
            b_handle.type_name().unwrap()
        );
    };
    assert_eq!(b_args.len(), 2);
    common::assert_handles_eq(&a_args[1], &b_handle);
    common::assert_handles_eq(&b_args[1], &a_handle);
}
