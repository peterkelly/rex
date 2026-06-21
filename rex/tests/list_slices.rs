mod common;

use rex::{
    engine::{Builder, EngineError, Handle, Module, ValueDisplayOptions},
    typesystem::{BuiltinTypeId, Scheme, Type},
};

fn i32_list_type() -> Type {
    Type::list(Type::builtin(BuiltinTypeId::I32))
}

fn u8_list_type() -> Type {
    Type::list(Type::builtin(BuiltinTypeId::U8))
}

fn bool_type() -> Type {
    Type::builtin(BuiltinTypeId::Bool)
}

fn checked_usize_arg(name: &str, value: i32) -> Result<usize, EngineError> {
    usize::try_from(value)
        .map_err(|_| EngineError::Custom(format!("{name}: negative argument {value}")))
}

fn data_values(heap: &rex::engine::Heap) -> Result<Vec<Handle>, EngineError> {
    (0..10).map(|value| heap.alloc_i32(100 + value)).collect()
}

fn binary_values() -> Vec<u8> {
    (100..110).collect()
}

fn data_u8_values(heap: &rex::engine::Heap) -> Result<Vec<Handle>, EngineError> {
    binary_values()
        .into_iter()
        .map(|value| heap.alloc_u8(value))
        .collect()
}

fn builder_with_list_shape_helpers() -> Builder<()> {
    let mut builder = Builder::with_prelude(()).unwrap();
    let i32_ty = Type::builtin(BuiltinTypeId::I32);
    let list_ty = i32_list_type();
    let u8_list_ty = u8_list_type();

    common::inject_globals(&mut builder, |module: &mut Module<()>| {
        let make_slice_scheme = Scheme::new(
            vec![],
            vec![],
            Type::fun(i32_ty.clone(), Type::fun(i32_ty.clone(), list_ty.clone())),
        );
        module.export_native("make_slice", make_slice_scheme, 2, |engine, _, args| {
            let start = checked_usize_arg("make_slice", args[0].as_i32()?)?;
            let end = checked_usize_arg("make_slice", args[1].as_i32()?)?;
            let data = engine.heap().alloc_data(data_values(engine.heap())?)?;
            engine.heap().alloc_list_slice(start, end, data)
        })?;

        let make_hybrid_scheme = Scheme::new(
            vec![],
            vec![],
            Type::fun(
                i32_ty.clone(),
                Type::fun(i32_ty.clone(), Type::fun(i32_ty.clone(), list_ty.clone())),
            ),
        );
        module.export_native("make_hybrid", make_hybrid_scheme, 3, |engine, _, args| {
            let cons_len = checked_usize_arg("make_hybrid", args[0].as_i32()?)?;
            let slice_start = checked_usize_arg("make_hybrid", args[1].as_i32()?)?;
            let slice_end = checked_usize_arg("make_hybrid", args[2].as_i32()?)?;
            let data = engine.heap().alloc_data(data_values(engine.heap())?)?;
            let mut tail = engine
                .heap()
                .alloc_list_slice(slice_start, slice_end, data)?;
            for value in (0..cons_len).rev() {
                let head = engine.heap().alloc_i32(value as i32)?;
                tail = engine.heap().alloc_cons(head, tail)?;
            }
            Ok(tail)
        })?;

        let make_binary_slice_scheme = Scheme::new(
            vec![],
            vec![],
            Type::fun(
                i32_ty.clone(),
                Type::fun(i32_ty.clone(), u8_list_ty.clone()),
            ),
        );
        module.export_native(
            "make_binary_slice",
            make_binary_slice_scheme,
            2,
            |engine, _, args| {
                let start = checked_usize_arg("make_binary_slice", args[0].as_i32()?)?;
                let end = checked_usize_arg("make_binary_slice", args[1].as_i32()?)?;
                let data = engine.heap().alloc_binary_data(binary_values())?;
                engine.heap().alloc_list_slice(start, end, data)
            },
        )?;

        let make_data_u8_slice_scheme = Scheme::new(
            vec![],
            vec![],
            Type::fun(
                i32_ty.clone(),
                Type::fun(i32_ty.clone(), u8_list_ty.clone()),
            ),
        );
        module.export_native(
            "make_data_u8_slice",
            make_data_u8_slice_scheme,
            2,
            |engine, _, args| {
                let start = checked_usize_arg("make_data_u8_slice", args[0].as_i32()?)?;
                let end = checked_usize_arg("make_data_u8_slice", args[1].as_i32()?)?;
                let data = engine.heap().alloc_data(data_u8_values(engine.heap())?)?;
                engine.heap().alloc_list_slice(start, end, data)
            },
        )?;

        let make_binary_hybrid_scheme = Scheme::new(
            vec![],
            vec![],
            Type::fun(
                i32_ty.clone(),
                Type::fun(
                    i32_ty.clone(),
                    Type::fun(i32_ty.clone(), u8_list_ty.clone()),
                ),
            ),
        );
        module.export_native(
            "make_binary_hybrid",
            make_binary_hybrid_scheme,
            3,
            |engine, _, args| {
                let cons_len = checked_usize_arg("make_binary_hybrid", args[0].as_i32()?)?;
                let slice_start = checked_usize_arg("make_binary_hybrid", args[1].as_i32()?)?;
                let slice_end = checked_usize_arg("make_binary_hybrid", args[2].as_i32()?)?;
                let data = engine.heap().alloc_binary_data(binary_values())?;
                let mut tail = engine
                    .heap()
                    .alloc_list_slice(slice_start, slice_end, data)?;
                for value in (0..cons_len).rev() {
                    let value = u8::try_from(value).map_err(|_| {
                        EngineError::Custom(format!(
                            "make_binary_hybrid: cons prefix too large {value}"
                        ))
                    })?;
                    let head = engine.heap().alloc_u8(value)?;
                    tail = engine.heap().alloc_cons(head, tail)?;
                }
                Ok(tail)
            },
        )?;

        let make_data_u8_hybrid_scheme = Scheme::new(
            vec![],
            vec![],
            Type::fun(
                i32_ty.clone(),
                Type::fun(
                    i32_ty.clone(),
                    Type::fun(i32_ty.clone(), u8_list_ty.clone()),
                ),
            ),
        );
        module.export_native(
            "make_data_u8_hybrid",
            make_data_u8_hybrid_scheme,
            3,
            |engine, _, args| {
                let cons_len = checked_usize_arg("make_data_u8_hybrid", args[0].as_i32()?)?;
                let slice_start = checked_usize_arg("make_data_u8_hybrid", args[1].as_i32()?)?;
                let slice_end = checked_usize_arg("make_data_u8_hybrid", args[2].as_i32()?)?;
                let data = engine.heap().alloc_data(data_u8_values(engine.heap())?)?;
                let mut tail = engine
                    .heap()
                    .alloc_list_slice(slice_start, slice_end, data)?;
                for value in (0..cons_len).rev() {
                    let value = u8::try_from(value).map_err(|_| {
                        EngineError::Custom(format!(
                            "make_data_u8_hybrid: cons prefix too large {value}"
                        ))
                    })?;
                    let head = engine.heap().alloc_u8(value)?;
                    tail = engine.heap().alloc_cons(head, tail)?;
                }
                Ok(tail)
            },
        )?;

        Ok(())
    })
    .unwrap();

    builder
}

async fn assert_eval(code: &str, expected: &str, expected_ty: Type) {
    let (_heap, handle, ty) = common::eval_source(builder_with_list_shape_helpers(), code)
        .await
        .unwrap_or_else(|err| panic!("expected ok, got error: {err}"));
    assert_eq!(ty, expected_ty);
    let opts = ValueDisplayOptions {
        include_numeric_suffixes: true,
        ..ValueDisplayOptions::default()
    };
    assert_eq!(handle.display_with(opts).unwrap(), expected);
}

async fn assert_vec_u8_eval(code: &str, expected: &[u8]) {
    let (_heap, handle, ty) = common::eval_source(builder_with_list_shape_helpers(), code)
        .await
        .unwrap_or_else(|err| panic!("expected ok, got error: {err}"));
    assert_eq!(ty, u8_list_type());
    assert_eq!(
        <Vec<u8> as rex::engine::FromRex>::from_rex(&handle).expect("Vec<u8> should decode"),
        expected
    );
}

async fn assert_err_contains(code: &str, needle: &str) {
    let err = match common::eval_source(builder_with_list_shape_helpers(), code).await {
        Ok((_heap, handle, ty)) => {
            panic!(
                "expected evaluation to fail, got {} with type {ty}",
                handle.display().unwrap()
            )
        }
        Err(err) => err,
    };
    let rendered = format!("{err}");
    assert!(
        rendered.contains(needle),
        "expected error containing {needle:?}, got: {rendered}"
    );
}

#[tokio::test]
async fn first_last_and_slice_work_on_vector_backed_lists() {
    assert_eval(
        "first 3 [0, 1, 2, 3, 4]",
        "[0i32, 1i32, 2i32]",
        i32_list_type(),
    )
    .await;
    assert_eval("last 2 [0, 1, 2, 3, 4]", "[3i32, 4i32]", i32_list_type()).await;
    assert_eval(
        "slice 1 4 [0, 1, 2, 3, 4]",
        "[1i32, 2i32, 3i32]",
        i32_list_type(),
    )
    .await;
    assert_eval("slice 2 2 [0, 1, 2, 3, 4]", "[]", i32_list_type()).await;
}

#[tokio::test]
async fn first_last_and_slice_work_on_existing_list_slices() {
    let list_ty = i32_list_type();
    assert_eval(
        "first 3 (make_slice 2 8)",
        "[102i32, 103i32, 104i32]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "last 2 (make_slice 2 8)",
        "[106i32, 107i32]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "slice 1 4 (make_slice 2 8)",
        "[103i32, 104i32, 105i32]",
        list_ty.clone(),
    )
    .await;
    assert_eval("slice 6 6 (make_slice 2 8)", "[]", list_ty).await;
}

#[tokio::test]
async fn slice_of_slice_uses_visible_offsets() {
    assert_eval(
        "slice 1 4 (slice 2 8 [0, 1, 2, 3, 4, 5, 6, 7, 8, 9])",
        "[3i32, 4i32, 5i32]",
        i32_list_type(),
    )
    .await;
}

#[tokio::test]
async fn first_last_and_slice_work_on_hybrid_cons_then_slice_lists() {
    let list_ty = i32_list_type();
    assert_eval(
        "first 3 (make_hybrid 2 3 8)",
        "[0i32, 1i32, 103i32]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "first 2 (make_hybrid 2 3 8)",
        "[0i32, 1i32]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "last 3 (make_hybrid 2 3 8)",
        "[105i32, 106i32, 107i32]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "last 6 (make_hybrid 2 3 8)",
        "[1i32, 103i32, 104i32, 105i32, 106i32, 107i32]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "slice 1 5 (make_hybrid 2 3 8)",
        "[1i32, 103i32, 104i32, 105i32]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "slice 2 5 (make_hybrid 2 3 8)",
        "[103i32, 104i32, 105i32]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "slice 0 7 (make_hybrid 2 3 8)",
        "[0i32, 1i32, 103i32, 104i32, 105i32, 106i32, 107i32]",
        list_ty.clone(),
    )
    .await;
    assert_eval("slice 7 7 (make_hybrid 2 3 8)", "[]", list_ty).await;
}

#[tokio::test]
async fn binary_slices_cover_start_middle_and_end_of_backing_data() {
    let list_ty = u8_list_type();
    assert_eval(
        "make_binary_slice 0 4",
        "[100u8, 101u8, 102u8, 103u8]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "make_binary_slice 3 7",
        "[103u8, 104u8, 105u8, 106u8]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "make_binary_slice 6 10",
        "[106u8, 107u8, 108u8, 109u8]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "make_binary_hybrid 2 0 4",
        "[0u8, 1u8, 100u8, 101u8, 102u8, 103u8]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "make_binary_hybrid 2 3 7",
        "[0u8, 1u8, 103u8, 104u8, 105u8, 106u8]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "make_binary_hybrid 2 6 10",
        "[0u8, 1u8, 106u8, 107u8, 108u8, 109u8]",
        list_ty,
    )
    .await;
}

#[tokio::test]
async fn first_last_and_slice_work_on_binary_backed_lists() {
    let list_ty = u8_list_type();
    assert_eval(
        "first 2 (make_binary_slice 0 4)",
        "[100u8, 101u8]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "slice 1 3 (make_binary_slice 0 4)",
        "[101u8, 102u8]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "last 2 (make_binary_slice 0 4)",
        "[102u8, 103u8]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "first 2 (make_binary_slice 3 7)",
        "[103u8, 104u8]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "slice 1 3 (make_binary_slice 3 7)",
        "[104u8, 105u8]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "last 2 (make_binary_slice 3 7)",
        "[105u8, 106u8]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "first 2 (make_binary_slice 6 10)",
        "[106u8, 107u8]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "slice 2 4 (make_binary_slice 6 10)",
        "[108u8, 109u8]",
        list_ty.clone(),
    )
    .await;
    assert_eval("last 2 (make_binary_slice 6 10)", "[108u8, 109u8]", list_ty).await;
}

#[tokio::test]
async fn slice_crosses_hybrid_cons_prefix_and_binary_tail() {
    let list_ty = u8_list_type();
    assert_eval(
        "slice 1 4 (make_binary_hybrid 2 0 4)",
        "[1u8, 100u8, 101u8]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "slice 2 6 (make_binary_hybrid 2 0 4)",
        "[100u8, 101u8, 102u8, 103u8]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "first 4 (make_binary_hybrid 2 3 8)",
        "[0u8, 1u8, 103u8, 104u8]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "last 3 (make_binary_hybrid 2 3 8)",
        "[105u8, 106u8, 107u8]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "slice 0 3 (make_binary_hybrid 2 3 8)",
        "[0u8, 1u8, 103u8]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "slice 1 5 (make_binary_hybrid 2 3 8)",
        "[1u8, 103u8, 104u8, 105u8]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "slice 2 5 (make_binary_hybrid 2 3 8)",
        "[103u8, 104u8, 105u8]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "slice 6 7 (make_binary_hybrid 2 3 8)",
        "[107u8]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "slice 1 4 (make_binary_hybrid 2 6 10)",
        "[1u8, 106u8, 107u8]",
        list_ty.clone(),
    )
    .await;
    assert_eval(
        "slice 2 6 (make_binary_hybrid 2 6 10)",
        "[106u8, 107u8, 108u8, 109u8]",
        list_ty.clone(),
    )
    .await;
    assert_eval("slice 7 7 (make_binary_hybrid 2 3 8)", "[]", list_ty).await;
}

#[tokio::test]
async fn binary_list_equality_uses_visible_elements_across_runtime_shapes() {
    assert_eval(
        r#"
        (
          make_binary_slice 0 4 == make_data_u8_slice 0 4,
          make_binary_slice 3 7 == make_data_u8_slice 3 7,
          make_binary_slice 6 10 == make_data_u8_slice 6 10,
          make_binary_hybrid 2 0 4 == make_data_u8_hybrid 2 0 4,
          make_binary_hybrid 2 3 8 == make_data_u8_hybrid 2 3 8,
          make_binary_hybrid 2 6 10 == make_data_u8_hybrid 2 6 10,
          make_binary_hybrid 2 3 8
            == Cons (0 is u8) (Cons (1 is u8) (make_binary_slice 3 8)),
          make_binary_hybrid 2 3 8
            == Cons (0 is u8) (Cons (1 is u8) (make_data_u8_slice 3 8)),
          make_binary_slice 3 7 == slice 2 6 (make_binary_slice 1 9),
          make_binary_slice 0 4 == slice 0 4 (make_binary_slice 0 10),
          make_binary_slice 6 10 == slice 6 10 (make_binary_slice 0 10),
          slice 1 4 (make_binary_hybrid 2 0 4)
            == Cons (1 is u8) (make_binary_slice 0 2),
          slice 1 5 (make_binary_hybrid 2 3 8)
            == Cons (1 is u8) (make_binary_slice 3 6),
          slice 1 4 (make_binary_hybrid 2 6 10)
            == Cons (1 is u8) (make_binary_slice 6 8),
          make_binary_slice 0 4 == make_binary_slice 1 5
        )
        "#,
        "(true, true, true, true, true, true, true, true, true, true, true, true, true, true, false)",
        Type::tuple(vec![bool_type(); 15]),
    )
    .await;
}

#[tokio::test]
async fn sliced_binary_and_hybrid_lists_pattern_match_and_decode_as_vec_u8() {
    assert_eval(
        r#"
        let xs = slice 2 5 (make_binary_hybrid 2 3 8) in
        match xs with {
          case a::b::c::[] -> [a, b, c];
          case _ -> [];
        }
        "#,
        "[103u8, 104u8, 105u8]",
        u8_list_type(),
    )
    .await;

    assert_vec_u8_eval("slice 1 3 (make_binary_slice 0 4)", &[101, 102]).await;
    assert_vec_u8_eval("slice 1 3 (make_binary_slice 3 7)", &[104, 105]).await;
    assert_vec_u8_eval("slice 2 4 (make_binary_slice 6 10)", &[108, 109]).await;
    assert_vec_u8_eval("slice 1 4 (make_binary_hybrid 2 0 4)", &[1, 100, 101]).await;
    assert_vec_u8_eval("slice 1 5 (make_binary_hybrid 2 3 8)", &[1, 103, 104, 105]).await;
    assert_vec_u8_eval("slice 2 5 (make_binary_hybrid 2 3 8)", &[103, 104, 105]).await;
    assert_vec_u8_eval("slice 1 4 (make_binary_hybrid 2 6 10)", &[1, 106, 107]).await;
}

#[tokio::test]
async fn sliced_hybrid_lists_pattern_match_as_lists() {
    assert_eval(
        r#"
        let xs = slice 2 5 (make_hybrid 2 3 8) in
        match xs with {
          case a::b::c::[] -> [a, b, c];
          case _ -> [];
        }
        "#,
        "[103i32, 104i32, 105i32]",
        i32_list_type(),
    )
    .await;
}

#[tokio::test]
async fn first_last_and_slice_reject_out_of_range_bounds() {
    assert_err_contains("first 6 [0, 1, 2, 3, 4]", "out of bounds").await;
    assert_err_contains("last 8 (make_slice 2 8)", "out of bounds").await;
    assert_err_contains("slice (negate 1) 2 [0, 1, 2]", "out of bounds").await;
    assert_err_contains("slice 1 4 [0, 1, 2]", "out of bounds").await;
    assert_err_contains("slice 4 2 (make_hybrid 2 3 8)", "invalid slice range").await;
}
