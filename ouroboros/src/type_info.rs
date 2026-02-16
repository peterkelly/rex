use std::fmt::{self, Display, Formatter};

use crate::{
    field::UnnamedField,
    float_range_check,
    product::{Array, Record, Tuple},
    signed_int_range_check,
    sum::{Enum, Optional, Union},
    symbolic::Symbolic,
    unsigned_int_range_check, Alias, Error, Fallible, Func, Generic, Ptr, Result,
};

pub trait TypeInfo {
    fn tname() -> TypeName;
    fn t() -> Type;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeName {
    pub n: &'static str,
    pub g: Vec<TypeName>,
}

impl TypeName {
    pub fn new(n: &'static str) -> Self {
        Self { n, g: vec![] }
    }

    pub fn with_generics(n: &'static str, generics: impl Into<Vec<TypeName>>) -> Self {
        Self {
            n,
            g: generics.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Type {
    // Basic types
    Unit,
    Bool,
    I8,
    I16,
    I32,
    I64,
    I128,
    U8,
    U16,
    U32,
    U64,
    U128,
    F32,
    F64,
    String,

    // Product types
    Array(Array),
    Record(Record),
    Tuple(Tuple),
    Func(Func),

    // Sum types
    Enum(Enum),
    Fallible(Fallible),
    Optional(Optional),
    Union(Union),

    // Special types
    Ptr(Ptr),
    Symbolic(Symbolic),
    Generic(Generic),
    Alias(Alias),
}

impl Type {
    pub fn n(&self) -> &str {
        match self {
            Self::Unit => "()",
            Self::Bool => "bool",
            Self::I8 => "i8",
            Self::I16 => "i16",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::I128 => "i128",
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::U128 => "u128",
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::String => "string",
            Self::Array(_) => "array",
            Self::Func(_) => "λ",
            Self::Record(rec) => &rec.n,
            Self::Tuple(_) => "tuple",
            Self::Enum(enm) => &enm.n,
            Self::Fallible(_) => "fallible",
            Self::Optional(_) => "optional",
            Self::Union(union) => &union.n,
            Self::Ptr(_) => "@",
            Self::Symbolic(sym) => &sym.n,
            Self::Generic(gen) => &gen.n,
            Self::Alias(alias) => &alias.n,
        }
    }

    pub fn is_compat(&self, value: Option<&serde_json::Value>) -> Result<()> {
        match self {
            // Unit values must be present and must be null.
            Self::Unit => match value {
                Some(value) if value.is_null() => Ok(()),
                _ => Err(Error::InvalidUnit {
                    got: value.cloned().unwrap_or(serde_json::Value::Null),
                }),
            },

            // Bool values must be present and must be a truthy/falsy.
            Self::Bool => match value {
                Some(value) if value.is_boolean() => Ok(()),
                _ => Err(Error::InvalidBool {
                    got: value.cloned().unwrap_or(serde_json::Value::Null),
                }),
            },

            // Int values must be present and must be an integral value in the
            // integer range (JSON only supports 64-bit integers so we need to
            // do explicit range checks).
            Self::I8 => signed_int_range_check!(value as i8 else InvalidI8),
            Self::I16 => signed_int_range_check!(value as i16 else InvalidI16),
            Self::I32 => signed_int_range_check!(value as i32 else InvalidI32),
            Self::I64 => signed_int_range_check!(value as i64 else InvalidI64),
            Self::I128 => signed_int_range_check!(value as i128 else InvalidI128),

            Self::U8 => unsigned_int_range_check!(value as u8 else InvalidU8),
            Self::U16 => unsigned_int_range_check!(value as u16 else InvalidU16),
            Self::U32 => unsigned_int_range_check!(value as u32 else InvalidU32),
            Self::U64 => unsigned_int_range_check!(value as u64 else InvalidU64),
            Self::U128 => unsigned_int_range_check!(value as u128 else InvalidU128),

            // Floating-point values must be present and must be a numeric value
            // value within the floating point range (JSON only supports 64-bit
            // floats and integers so we need to do explicit range checks).
            Self::F32 => float_range_check!(value as f32 else InvalidF32),
            Self::F64 => float_range_check!(value as f64 else InvalidF64),

            // String values must be present and must be string kinded.
            Self::String => match value {
                Some(value) if value.is_string() => Ok(()),
                _ => Err(Error::InvalidString {
                    got: value.cloned().unwrap_or(serde_json::Value::Null),
                }),
            },

            // Array values must be present and must be array kinded. All items
            // in the array are also checked for compatibility.
            Self::Array(arr) => arr.is_compat(value),

            // Function values must be present and must be object kinded with at
            // least one key, "λ", that is string kinded. This key is the name
            // of the λ-value (and λ-values always have function types).
            Self::Func(func) => func.is_compat(value),

            // Record values must be present and must be object kinded for named
            // records and array kinded for unnamed records. All fields in the
            // record are also checked for compatibility.
            Self::Record(rec) => rec.is_compat(value),

            // Tuple values must be present and must be array kinded. All items
            // in the tuple are also checked for compatibility.
            Self::Tuple(tup) => tup.is_compat(value),

            // Enum values must be present and must be string kinded or integer
            // kinded. The string kinded value must be one of the enum variants
            // and the integer kinded value must be one of the enum variant
            // constant-values.
            Self::Enum(enm) => enm.is_compat(value),

            Self::Fallible(fall) => fall.is_compat(value),

            // Optional values must be present and must be null or compatible
            // with the inner type of the optional.
            Self::Optional(opt) => opt.is_compat(value),

            Self::Union(union) => union.is_compat(value),

            // Symbolic values must be present and must be string kinded.
            Self::Ptr(p) => p.is_compat(value),

            // Symbolic values must be present and must be string kinded.
            Self::Symbolic(sym) => sym.is_compat(value),

            // Generic values are compatibility with everything.
            Self::Generic(gen) => gen.is_compat(value),

            // Alias values are checked for compatibility by checking for
            // compatibility against the inner type of the alias.
            Self::Alias(alias) => alias.is_compat(value),
        }
    }
}

impl From<Array> for Type {
    fn from(t: Array) -> Self {
        Self::Array(t)
    }
}

impl From<Func> for Type {
    fn from(t: Func) -> Self {
        Self::Func(t)
    }
}

impl From<Record> for Type {
    fn from(t: Record) -> Self {
        Self::Record(t)
    }
}

impl From<Tuple> for Type {
    fn from(t: Tuple) -> Self {
        Self::Tuple(t)
    }
}

impl From<Enum> for Type {
    fn from(t: Enum) -> Self {
        Self::Enum(t)
    }
}

impl From<Fallible> for Type {
    fn from(t: Fallible) -> Self {
        Self::Fallible(t)
    }
}

impl From<Optional> for Type {
    fn from(t: Optional) -> Self {
        Self::Optional(t)
    }
}

impl From<Union> for Type {
    fn from(t: Union) -> Self {
        Self::Union(t)
    }
}

impl From<Ptr> for Type {
    fn from(t: Ptr) -> Self {
        Self::Ptr(t)
    }
}

impl From<Symbolic> for Type {
    fn from(t: Symbolic) -> Self {
        Self::Symbolic(t)
    }
}

impl From<Generic> for Type {
    fn from(t: Generic) -> Self {
        Self::Generic(t)
    }
}

impl From<Alias> for Type {
    fn from(t: Alias) -> Self {
        Self::Alias(t)
    }
}

impl TypeInfo for () {
    fn tname() -> TypeName {
        TypeName { n: "()", g: vec![] }
    }
    fn t() -> Type {
        Type::Unit
    }
}

impl TypeInfo for bool {
    fn tname() -> TypeName {
        TypeName {
            n: "bool",
            g: vec![],
        }
    }

    fn t() -> Type {
        Type::Bool
    }
}

impl TypeInfo for i8 {
    fn tname() -> TypeName {
        TypeName {
            n: "int8",
            g: vec![],
        }
    }

    fn t() -> Type {
        Type::I8
    }
}

impl TypeInfo for i16 {
    fn tname() -> TypeName {
        TypeName {
            n: "int16",
            g: vec![],
        }
    }

    fn t() -> Type {
        Type::I16
    }
}

impl TypeInfo for i32 {
    fn tname() -> TypeName {
        TypeName {
            n: "int32",
            g: vec![],
        }
    }

    fn t() -> Type {
        Type::I32
    }
}

impl TypeInfo for i64 {
    fn tname() -> TypeName {
        TypeName {
            n: "int64",
            g: vec![],
        }
    }

    fn t() -> Type {
        Type::I64
    }
}

impl TypeInfo for i128 {
    fn tname() -> TypeName {
        TypeName {
            n: "int128",
            g: vec![],
        }
    }

    fn t() -> Type {
        Type::I128
    }
}

impl TypeInfo for u8 {
    fn tname() -> TypeName {
        TypeName {
            n: "uint8",
            g: vec![],
        }
    }

    fn t() -> Type {
        Type::U8
    }
}

impl TypeInfo for u16 {
    fn tname() -> TypeName {
        TypeName {
            n: "uint16",
            g: vec![],
        }
    }

    fn t() -> Type {
        Type::U16
    }
}

impl TypeInfo for u32 {
    fn tname() -> TypeName {
        TypeName {
            n: "uint32",
            g: vec![],
        }
    }

    fn t() -> Type {
        Type::U32
    }
}

impl TypeInfo for u64 {
    fn tname() -> TypeName {
        TypeName {
            n: "uint64",
            g: vec![],
        }
    }

    fn t() -> Type {
        Type::U64
    }
}

impl TypeInfo for u128 {
    fn tname() -> TypeName {
        TypeName {
            n: "uint128",
            g: vec![],
        }
    }

    fn t() -> Type {
        Type::U128
    }
}

impl TypeInfo for f32 {
    fn tname() -> TypeName {
        TypeName {
            n: "float32",
            g: vec![],
        }
    }

    fn t() -> Type {
        Type::F32
    }
}

impl TypeInfo for f64 {
    fn tname() -> TypeName {
        TypeName {
            n: "float64",
            g: vec![],
        }
    }

    fn t() -> Type {
        Type::F64
    }
}

impl TypeInfo for String {
    fn tname() -> TypeName {
        TypeName {
            n: "string",
            g: vec![],
        }
    }

    fn t() -> Type {
        Type::String
    }
}

impl<T: TypeInfo> TypeInfo for Vec<T> {
    fn tname() -> TypeName {
        TypeName {
            n: "array",
            g: vec![T::tname()],
        }
    }

    fn t() -> Type {
        Type::Array(Array::new(T::t()))
    }
}

impl<T: TypeInfo> TypeInfo for [T] {
    fn tname() -> TypeName {
        TypeName {
            n: "array",
            g: vec![T::tname()],
        }
    }

    fn t() -> Type {
        Type::Array(Array::new(T::t()))
    }
}

impl<T: TypeInfo, E: TypeInfo> TypeInfo for std::result::Result<T, E> {
    fn tname() -> TypeName {
        TypeName {
            n: "fallible",
            g: vec![T::tname(), E::tname()],
        }
    }

    fn t() -> Type {
        Type::Fallible(Fallible::new(T::t(), E::t()))
    }
}

impl<T: TypeInfo> TypeInfo for Option<T> {
    fn tname() -> TypeName {
        TypeName {
            n: "optional",
            g: vec![T::tname()],
        }
    }

    fn t() -> Type {
        Type::Optional(Optional::new(T::t()))
    }
}

impl<T: TypeInfo> TypeInfo for Box<T> {
    fn tname() -> TypeName {
        T::tname()
    }

    fn t() -> Type {
        T::t()
    }
}

impl<T: TypeInfo> TypeInfo for &T {
    fn tname() -> TypeName {
        T::tname()
    }

    fn t() -> Type {
        T::t()
    }
}

impl Display for Type {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unit => "()".fmt(f),
            Self::Bool => "bool".fmt(f),
            Self::I8 => "i8".fmt(f),
            Self::I16 => "i16".fmt(f),
            Self::I32 => "i32".fmt(f),
            Self::I64 => "i64".fmt(f),
            Self::I128 => "i128".fmt(f),
            Self::U8 => "u8".fmt(f),
            Self::U16 => "u16".fmt(f),
            Self::U32 => "u32".fmt(f),
            Self::U64 => "u64".fmt(f),
            Self::U128 => "u128".fmt(f),
            Self::F32 => "f32".fmt(f),
            Self::F64 => "f64".fmt(f),
            Self::String => "string".fmt(f),
            Self::Array(arr) => arr.fmt(f),
            Self::Func(func) => func.fmt(f),
            Self::Record(rec) => rec.fmt(f),
            Self::Tuple(tup) => tup.fmt(f),
            Self::Enum(enm) => enm.fmt(f),
            Self::Fallible(fall) => fall.fmt(f),
            Self::Optional(opt) => opt.fmt(f),
            Self::Union(union) => union.fmt(f),
            Self::Ptr(p) => p.fmt(f),
            Self::Symbolic(sym) => sym.fmt(f),
            Self::Generic(gen) => gen.fmt(f),
            Self::Alias(alias) => alias.fmt(f),
        }
    }
}

macro_rules! impl_tuple {
    ($($args: ident),*) => {
        impl<$($args),*> TypeInfo for ($($args),*)
        where
            $($args: TypeInfo),*
        {

            fn tname() -> TypeName {
                TypeName {
                    n: "tuple",
                    g: vec![$($args::tname()),*],
                }
            }

            fn t() -> Type {
                Type::Tuple(Tuple::new([
                    $(UnnamedField::new($args::t())),*
                ]))
            }
        }
    };
}

// implement for the unary-tuple case because macro rules are weird

impl<T0: TypeInfo> TypeInfo for (T0,) {
    fn tname() -> TypeName {
        TypeName {
            n: "tuple",
            g: vec![T0::tname()],
        }
    }

    fn t() -> Type {
        Type::Tuple(Tuple::new([UnnamedField::new(T0::t())]))
    }
}

impl_tuple!(T0, T1);
impl_tuple!(T0, T1, T2);
impl_tuple!(T0, T1, T2, T3);
impl_tuple!(T0, T1, T2, T3, T4);
impl_tuple!(T0, T1, T2, T3, T4, T5);
impl_tuple!(T0, T1, T2, T3, T4, T5, T6);
impl_tuple!(T0, T1, T2, T3, T4, T5, T6, T7);
impl_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8);
impl_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9);
impl_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10);
impl_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11);
impl_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12);
impl_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13);
impl_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14);
impl_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15);
