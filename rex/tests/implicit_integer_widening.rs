mod common;

use std::fmt::Debug;

use rex::{
    engine::{Builder, EngineError, FromRex},
    typesystem::{BuiltinTypeId, Type},
};

async fn eval(source: &str) -> Result<(rex::engine::Heap, rex::engine::Handle, Type), EngineError> {
    common::eval_source(Builder::with_prelude(()).unwrap(), source).await
}

async fn assert_i32(source: &str, expected: i32) {
    let (_heap, handle, ty) = eval(source).await.unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32), "{source}");
    assert_eq!(i32::from_rex(&handle).unwrap(), expected, "{source}");
}

async fn assert_widens_to<T>(
    src: &str,
    dst: &str,
    value: &str,
    expected: T,
    expected_ty: BuiltinTypeId,
) where
    T: FromRex + PartialEq + Debug,
{
    let source = format!("let x: {dst} = ({value} is {src}) in x");
    let (_heap, handle, ty) = eval(&source).await.unwrap();
    assert_eq!(ty, Type::builtin(expected_ty), "{source}");
    assert_eq!(T::from_rex(&handle).unwrap(), expected, "{source}");
}

async fn assert_type_error(source: &str) {
    let err = match eval(source).await {
        Ok((_heap, handle, ty)) => panic!(
            "expected type error, got {} with type {ty}",
            handle.display().unwrap()
        ),
        Err(err) => err,
    };
    assert!(
        matches!(err, EngineError::Type(_)),
        "expected type error, got {err}"
    );
}

#[tokio::test]
async fn widens_each_supported_source_destination_pair() {
    assert_widens_to::<i16>("i8", "i16", "-7", -7, BuiltinTypeId::I16).await;
    assert_widens_to::<i32>("i8", "i32", "-7", -7, BuiltinTypeId::I32).await;
    assert_widens_to::<i64>("i8", "i64", "-7", -7, BuiltinTypeId::I64).await;
    assert_widens_to::<i32>("i16", "i32", "-20000", -20000, BuiltinTypeId::I32).await;
    assert_widens_to::<i64>("i16", "i64", "-20000", -20000, BuiltinTypeId::I64).await;
    assert_widens_to::<i64>("i32", "i64", "-2000000000", -2000000000, BuiltinTypeId::I64).await;

    assert_widens_to::<u16>("u8", "u16", "250", 250, BuiltinTypeId::U16).await;
    assert_widens_to::<u32>("u8", "u32", "250", 250, BuiltinTypeId::U32).await;
    assert_widens_to::<u64>("u8", "u64", "250", 250, BuiltinTypeId::U64).await;
    assert_widens_to::<i16>("u8", "i16", "250", 250, BuiltinTypeId::I16).await;
    assert_widens_to::<i32>("u8", "i32", "250", 250, BuiltinTypeId::I32).await;
    assert_widens_to::<i64>("u8", "i64", "250", 250, BuiltinTypeId::I64).await;

    assert_widens_to::<u32>("u16", "u32", "60000", 60000, BuiltinTypeId::U32).await;
    assert_widens_to::<u64>("u16", "u64", "60000", 60000, BuiltinTypeId::U64).await;
    assert_widens_to::<i32>("u16", "i32", "60000", 60000, BuiltinTypeId::I32).await;
    assert_widens_to::<i64>("u16", "i64", "60000", 60000, BuiltinTypeId::I64).await;

    assert_widens_to::<u64>("u32", "u64", "4000000000", 4000000000, BuiltinTypeId::U64).await;
    assert_widens_to::<i64>("u32", "i64", "4000000000", 4000000000, BuiltinTypeId::I64).await;
}

#[tokio::test]
async fn widens_function_argument_when_target_type_is_known() {
    assert_i32(
        r#"
fn a : i32 -> i32 -> i32 = \x y -> x * y;
fn b : i8 -> i8 = \x -> x + 1;

a 4 (b 5)
"#,
        24,
    )
    .await;
}

#[tokio::test]
async fn widens_annotation_when_target_type_is_known() {
    assert_i32("let x: i32 = (7 is i8) in x", 7).await;
}

#[tokio::test]
async fn widens_record_field_when_constructor_expects_record_type() {
    assert_i32(
        r#"
type Box = Box { x: i32 };
let box: Box = Box { x = (7 is i8) } in box.x
"#,
        7,
    )
    .await;
}

#[tokio::test]
async fn widens_unsigned_integer_to_signed_target_only_when_all_values_fit() {
    assert_i32(
        r#"
fn needs_i32 : i32 -> i32 = \x -> x;

needs_i32 (255 is u8)
"#,
        255,
    )
    .await;
}

fn supported_widening_pair(src: &str, dst: &str) -> bool {
    matches!(
        (src, dst),
        ("i8", "i16" | "i32" | "i64")
            | ("i16", "i32" | "i64")
            | ("i32", "i64")
            | ("u8", "u16" | "u32" | "u64" | "i16" | "i32" | "i64")
            | ("u16", "u32" | "u64" | "i32" | "i64")
            | ("u32", "u64" | "i64")
    )
}

#[tokio::test]
async fn does_not_insert_narrowing_conversion() {
    assert_type_error(
        r#"
fn needs_i8 : i8 -> i8 = \x -> x;

needs_i8 (300 is i16)
"#,
    )
    .await;
}

#[tokio::test]
async fn rejects_every_unsupported_integer_source_destination_pair() {
    const INTEGER_TYPES: [&str; 8] = ["u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64"];

    for src in INTEGER_TYPES {
        for dst in INTEGER_TYPES {
            if src == dst || supported_widening_pair(src, dst) {
                continue;
            }

            let source = format!("let x: {dst} = (7 is {src}) in x");
            assert_type_error(&source).await;
        }
    }
}

#[tokio::test]
async fn does_not_promote_integers_to_floats_or_demote_floats_to_integers() {
    for source in [
        "let x: f32 = (7 is i32) in x",
        "let x: f64 = (7 is u32) in x",
        "let x: i32 = (7.0 is f32) in x",
        "let x: u32 = (7.0 is f64) in x",
    ] {
        assert_type_error(source).await;
    }
}

#[tokio::test]
async fn does_not_make_mixed_numeric_operators_guess_a_common_type() {
    assert_type_error("(1 is i8) + (2 is i32)").await;
}
