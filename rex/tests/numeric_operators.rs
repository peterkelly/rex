use std::fmt::Debug;

use rex::{
    engine::{Engine, EngineError, FromRex, Handle},
    typesystem::{BuiltinTypeId, Type, TypeError},
};

async fn eval(source: &str) -> Result<(Handle, Type), EngineError> {
    Engine::with_prelude(())
        .unwrap()
        .into_evaluator()
        .eval_snippet(source)
        .await
        .map_err(|err| err.into_engine_error())
}

async fn assert_value<T>(source: &str, expected: T, expected_ty: BuiltinTypeId)
where
    T: FromRex + PartialEq + Debug,
{
    let (value, ty) = eval(source).await.unwrap();
    assert_eq!(ty, Type::builtin(expected_ty), "{source}");
    assert_eq!(value.to_rust::<T>().unwrap(), expected, "{source}");
}

fn strip_type_span(mut err: TypeError) -> TypeError {
    while let TypeError::Spanned { error, .. } = err {
        err = *error;
    }
    err
}

macro_rules! assert_integer_ops {
    ($name:literal, $rust_ty:ty, $builtin:expr) => {{
        assert_value::<$rust_ty>(
            &format!("(6 is {}) + (3 is {})", $name, $name),
            9 as $rust_ty,
            $builtin,
        )
        .await;
        assert_value::<$rust_ty>(
            &format!("(6 is {}) - (3 is {})", $name, $name),
            3 as $rust_ty,
            $builtin,
        )
        .await;
        assert_value::<$rust_ty>(
            &format!("(6 is {}) * (3 is {})", $name, $name),
            18 as $rust_ty,
            $builtin,
        )
        .await;
        assert_value::<$rust_ty>(
            &format!("(7 is {}) / (3 is {})", $name, $name),
            2 as $rust_ty,
            $builtin,
        )
        .await;
        assert_value::<$rust_ty>(
            &format!("(7 is {}) % (3 is {})", $name, $name),
            1 as $rust_ty,
            $builtin,
        )
        .await;
    }};
}

#[tokio::test]
async fn primitive_integer_operators_cover_all_widths() {
    assert_value::<i32>("1 / 2", 0i32, BuiltinTypeId::I32).await;

    assert_integer_ops!("u8", u8, BuiltinTypeId::U8);
    assert_integer_ops!("u16", u16, BuiltinTypeId::U16);
    assert_integer_ops!("u32", u32, BuiltinTypeId::U32);
    assert_integer_ops!("u64", u64, BuiltinTypeId::U64);
    assert_integer_ops!("i8", i8, BuiltinTypeId::I8);
    assert_integer_ops!("i16", i16, BuiltinTypeId::I16);
    assert_integer_ops!("i32", i32, BuiltinTypeId::I32);
    assert_integer_ops!("i64", i64, BuiltinTypeId::I64);
}

#[tokio::test]
async fn primitive_float_operators_cover_all_widths() {
    assert_value::<f32>("6.0 + 3.0", 9.0f32, BuiltinTypeId::F32).await;
    assert_value::<f32>("6.0 - 3.0", 3.0f32, BuiltinTypeId::F32).await;
    assert_value::<f32>("6.0 * 3.0", 18.0f32, BuiltinTypeId::F32).await;
    assert_value::<f32>("7.0 / 2.0", 3.5f32, BuiltinTypeId::F32).await;

    assert_value::<f64>(
        "(prim_to_f64 6.0) + (prim_to_f64 3.0)",
        9.0f64,
        BuiltinTypeId::F64,
    )
    .await;
    assert_value::<f64>(
        "(prim_to_f64 6.0) - (prim_to_f64 3.0)",
        3.0f64,
        BuiltinTypeId::F64,
    )
    .await;
    assert_value::<f64>(
        "(prim_to_f64 6.0) * (prim_to_f64 3.0)",
        18.0f64,
        BuiltinTypeId::F64,
    )
    .await;
    assert_value::<f64>(
        "(prim_to_f64 7.0) / (prim_to_f64 2.0)",
        3.5f64,
        BuiltinTypeId::F64,
    )
    .await;
}

#[tokio::test]
async fn unsigned_subtraction_works_for_annotated_function() {
    assert_value::<u32>(
        r#"
fn sub_unsigned: u32 -> u32 -> u32 = \a b -> a - b;

sub_unsigned 5 3
"#,
        2u32,
        BuiltinTypeId::U32,
    )
    .await;
}

#[tokio::test]
async fn unsigned_subtraction_does_not_enable_negative_literals() {
    let err = eval("let x: u32 = -3 in x").await.unwrap_err();
    let EngineError::Type(err) = err else {
        panic!("expected type error");
    };
    assert!(matches!(
        strip_type_span(err),
        TypeError::NoInstance(class, ty)
            if class.as_ref() == "AdditiveGroup" && ty == "u32"
    ));
}
