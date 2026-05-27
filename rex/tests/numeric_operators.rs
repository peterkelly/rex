use std::fmt::Debug;

use rex::{
    engine::{Engine, EngineError, FromRex, Handle},
    parser::parse as parse_rex,
    typesystem::{BuiltinTypeId, Type, TypeError},
};

async fn eval(source: &str) -> Result<(Handle, Type), EngineError> {
    let mut compiler = Engine::with_prelude(()).unwrap().into_compiler();
    let parsed = parse_rex(source).unwrap();
    let program = compiler
        .compile_program(&parsed, Default::default())
        .await
        .map_err(|err| err.into_engine_error())?;
    let ty = program.result_type().clone();
    let value = compiler
        .into_evaluator()
        .run(program, Default::default())
        .await
        .map_err(|err| err.into_engine_error())?;
    Ok((value, ty))
}

async fn assert_value<T>(source: &str, expected: T, expected_ty: BuiltinTypeId)
where
    T: FromRex + PartialEq + Debug,
{
    let (value, ty) = eval(source).await.unwrap();
    assert_eq!(ty, Type::builtin(expected_ty), "{source}");
    assert_eq!(value.to_rust::<T>().unwrap(), expected, "{source}");
}

async fn assert_runtime_error(source: &str, expected: &str) {
    let err = eval(source).await.unwrap_err();
    assert_eq!(err.to_string(), expected, "{source}");
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

macro_rules! assert_unsigned_op_failures {
    ($name:literal, $max:literal) => {{
        let overflow = format!("integer overflow ({})", $name);
        let underflow = format!("integer underflow ({})", $name);

        assert_runtime_error(
            &format!("({} is {}) + (1 is {})", $max, $name, $name),
            &overflow,
        )
        .await;
        assert_runtime_error(&format!("(0 is {}) - (1 is {})", $name, $name), &underflow).await;
        assert_runtime_error(
            &format!("({} is {}) * (2 is {})", $max, $name, $name),
            &overflow,
        )
        .await;
        assert_runtime_error(&format!("(1 is {}) / (0 is {})", $name, $name), &overflow).await;
        assert_runtime_error(&format!("(1 is {}) % (0 is {})", $name, $name), &overflow).await;
    }};
}

macro_rules! assert_signed_op_failures {
    ($name:literal, $high:literal, $min_half:literal) => {{
        let overflow = format!("integer overflow ({})", $name);
        let underflow = format!("integer underflow ({})", $name);

        assert_runtime_error(
            &format!("({} is {}) + ({} is {})", $high, $name, $high, $name),
            &overflow,
        )
        .await;
        assert_runtime_error(
            &format!("(-{} is {}) - ({} is {})", $high, $name, $high, $name),
            &underflow,
        )
        .await;
        assert_runtime_error(
            &format!("({} is {}) * (2 is {})", $high, $name, $name),
            &overflow,
        )
        .await;
        assert_runtime_error(
            &format!(
                "(((-{} is {}) * (2 is {})) / (-1 is {}))",
                $min_half, $name, $name, $name
            ),
            &overflow,
        )
        .await;
        assert_runtime_error(&format!("(1 is {}) % (0 is {})", $name, $name), &overflow).await;
    }};
}

#[tokio::test]
async fn primitive_integer_operator_successes_cover_all_widths() {
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
async fn primitive_integer_operator_failures_cover_all_widths() {
    assert_unsigned_op_failures!("u8", "255");
    assert_unsigned_op_failures!("u16", "65535");
    assert_unsigned_op_failures!("u32", "4294967295");
    assert_unsigned_op_failures!("u64", "18446744073709551615");
    assert_signed_op_failures!("i8", "100", "64");
    assert_signed_op_failures!("i16", "20000", "16384");
    assert_signed_op_failures!("i32", "2000000000", "1073741824");
    assert_signed_op_failures!("i64", "5000000000000000000", "4611686018427387904");
}

#[tokio::test]
async fn primitive_float_operators_cover_all_widths() {
    assert_value::<f32>("6.0 + 3.0", 9.0f32, BuiltinTypeId::F32).await;
    assert_value::<f32>("6.0 - 3.0", 3.0f32, BuiltinTypeId::F32).await;
    assert_value::<f32>("6.0 * 3.0", 18.0f32, BuiltinTypeId::F32).await;
    assert_value::<f32>("7.0 / 2.0", 3.5f32, BuiltinTypeId::F32).await;

    assert_value::<f64>(
        r#"
let
  add_float: f64 -> f64 -> f64 = \x y -> x + y
in
  add_float 6.0 3.0
"#,
        9.0f64,
        BuiltinTypeId::F64,
    )
    .await;
    assert_value::<f64>("(7.0 is f64) / (2.0 is f64)", 3.5f64, BuiltinTypeId::F64).await;

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
