mod common;

use rex::{
    engine::{Builder, EngineError, Handle, Module, ValueDisplayOptions},
    typesystem::{BuiltinTypeId, Scheme, Type},
};

fn i32_list_type() -> Type {
    Type::list(Type::builtin(BuiltinTypeId::I32))
}

fn checked_usize_arg(name: &str, value: i32) -> Result<usize, EngineError> {
    usize::try_from(value)
        .map_err(|_| EngineError::Custom(format!("{name}: negative argument {value}")))
}

fn data_values(heap: &rex::engine::Heap) -> Result<Vec<Handle>, EngineError> {
    (0..10).map(|value| heap.alloc_i32(100 + value)).collect()
}

fn builder_with_list_shape_helpers() -> Builder<()> {
    let mut builder = Builder::with_prelude(()).unwrap();
    let i32_ty = Type::builtin(BuiltinTypeId::I32);
    let list_ty = i32_list_type();

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
