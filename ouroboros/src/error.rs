use crate::{
    Alias, Array, Enum, Fallible, Fields, Func, Optional, Ptr, Record, Symbolic, Tuple, Union,
};

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("unexpected null")]
    UnexpectedNull,

    #[error("unexpected value {got}")]
    UnexpectedValue { got: serde_json::Value },

    #[error("expected unit, got `{got}`")]
    InvalidUnit { got: serde_json::Value },

    #[error("expected boolean, got `{got}`")]
    InvalidBool { got: serde_json::Value },

    #[error("expected i8, got `{got}`")]
    InvalidI8 { got: serde_json::Value },

    #[error("expected i16, got `{got}`")]
    InvalidI16 { got: serde_json::Value },

    #[error("expected i32, got `{got}`")]
    InvalidI32 { got: serde_json::Value },

    #[error("expected i64, got `{got}`")]
    InvalidI64 { got: serde_json::Value },

    #[error("expected i128, got `{got}`")]
    InvalidI128 { got: serde_json::Value },

    #[error("expected u8, got `{got}`")]
    InvalidU8 { got: serde_json::Value },

    #[error("expected u16, got `{got}`")]
    InvalidU16 { got: serde_json::Value },

    #[error("expected u32, got `{got}`")]
    InvalidU32 { got: serde_json::Value },

    #[error("expected u64, got `{got}`")]
    InvalidU64 { got: serde_json::Value },

    #[error("expected u128, got `{got}`")]
    InvalidU128 { got: serde_json::Value },

    #[error("expected f32, got `{got}`")]
    InvalidF32 { got: serde_json::Value },

    #[error("expected f64, got `{got}`")]
    InvalidF64 { got: serde_json::Value },

    #[error("expected string, got `{got}`")]
    InvalidString { got: serde_json::Value },

    #[error("invalid `{expected}` array, {e}")]
    InvalidArray { expected: Array, e: Box<Error> },

    #[error("invalid `{expected}` func, {e}")]
    InvalidFunc { expected: Func, e: Box<Error> },

    #[error("invalid `{expected}` record, {e}")]
    InvalidRecord { expected: Record, e: Box<Error> },

    #[error("invalid `{expected}` tuple, {e}")]
    InvalidTuple { expected: Tuple, e: Box<Error> },

    #[error("invalid `{expected}` enum, {e}")]
    InvalidEnum { expected: Enum, e: Box<Error> },

    #[error("invalid `{expected}` fallible, {e}")]
    InvalidFallible { expected: Fallible, e: Box<Error> },

    #[error("invalid `{expected}` optional, {e}")]
    InvalidOptional { expected: Optional, e: Box<Error> },

    #[error("invalid `{expected}` union, {e}")]
    InvalidUnion { expected: Union, e: Box<Error> },

    #[error("invalid `{expected}` ptr, {e}")]
    InvalidPtr { expected: Ptr, e: Box<Error> },

    #[error("invalid `{expected}` symbol, {e}")]
    InvalidSymbolic { expected: Symbolic, e: Box<Error> },

    #[error("invalid `{expected}` alias, {e}")]
    InvalidAlias { expected: Alias, e: Box<Error> },

    #[error("invalid `{expected}` fields, {e}")]
    InvalidFields { expected: Fields, e: Box<Error> },

    #[error("invalid field {index}, {e}")]
    InvalidUnnamedField { index: usize, e: Box<Error> },

    #[error("invalid field {index}, {e}")]
    InvalidNamedField { index: String, e: Box<Error> },
}
