//! Conversion traits between Rust values and owned Rex values.

use std::{
    any::{Any, TypeId},
    collections::{BTreeMap, HashMap},
};

use blake3::Hash;
use chrono::{DateTime, Utc};
use rex_ast::Symbol;
use uuid::Uuid;

use crate::{EngineError, Value};

/// Convert a Rust value into Rex's owned host representation.
///
/// This conversion is independent of the evaluator and heap. Typed host
/// exports use it before their result crosses back into the Rex runtime.
pub trait IntoRex {
    fn into_rex(self) -> Result<Value, EngineError>;
}

/// Decode Rex's owned host representation into a Rust value.
///
/// The input is consumed so strings, byte buffers, and collection storage can
/// be moved into the Rust result without cloning heap-backed data.
pub trait FromRex: Sized {
    fn from_rex(value: Value) -> Result<Self, EngineError>;
}

fn mismatch(expected: impl Into<String>, got: &Value) -> EngineError {
    EngineError::NativeType {
        expected: expected.into(),
        got: got.value_type_name().into(),
    }
}

macro_rules! impl_rex_scalar {
    ($t:ty, $variant:ident) => {
        impl IntoRex for $t {
            fn into_rex(self) -> Result<Value, EngineError> {
                Ok(Value::$variant(self))
            }
        }

        impl FromRex for $t {
            fn from_rex(value: Value) -> Result<Self, EngineError> {
                match value {
                    Value::$variant(value) => Ok(value),
                    other => Err(mismatch(stringify!($t), &other)),
                }
            }
        }
    };
}

impl_rex_scalar!(bool, Bool);
impl_rex_scalar!(u8, U8);
impl_rex_scalar!(u16, U16);
impl_rex_scalar!(u32, U32);
impl_rex_scalar!(u64, U64);
impl_rex_scalar!(i8, I8);
impl_rex_scalar!(i16, I16);
impl_rex_scalar!(i32, I32);
impl_rex_scalar!(i64, I64);
impl_rex_scalar!(f32, F32);
impl_rex_scalar!(f64, F64);
impl_rex_scalar!(String, String);
impl_rex_scalar!(Uuid, Uuid);
impl_rex_scalar!(Hash, Hash);
impl_rex_scalar!(DateTime<Utc>, DateTime);

impl IntoRex for &str {
    fn into_rex(self) -> Result<Value, EngineError> {
        Ok(Value::String(self.to_owned()))
    }
}

impl<T: IntoRex + 'static> IntoRex for Vec<T> {
    fn into_rex(self) -> Result<Value, EngineError> {
        if TypeId::of::<T>() == TypeId::of::<u8>() {
            let boxed: Box<dyn Any> = Box::new(self);
            let bytes = boxed
                .downcast::<Vec<u8>>()
                .map_err(|_| EngineError::Internal("Vec<u8> TypeId downcast failed".into()))?;
            return Ok(Value::Bytes(*bytes));
        }

        self.into_iter()
            .map(IntoRex::into_rex)
            .collect::<Result<Vec<_>, _>>()
            .map(Value::List)
    }
}

impl<T: FromRex + 'static> FromRex for Vec<T> {
    fn from_rex(value: Value) -> Result<Self, EngineError> {
        if TypeId::of::<T>() == TypeId::of::<u8>() {
            let Value::Bytes(bytes) = value else {
                return Err(mismatch("bytes", &value));
            };
            let boxed: Box<dyn Any> = Box::new(bytes);
            return boxed
                .downcast::<Vec<T>>()
                .map(|values| *values)
                .map_err(|_| EngineError::Internal("Vec<u8> TypeId downcast failed".into()));
        }

        let Value::List(items) = value else {
            return Err(mismatch("list", &value));
        };
        items.into_iter().map(T::from_rex).collect()
    }
}

impl<T: IntoRex> IntoRex for BTreeMap<String, T> {
    fn into_rex(self) -> Result<Value, EngineError> {
        self.into_iter()
            .map(|(name, value)| Ok((name, value.into_rex()?)))
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map(Value::Dict)
    }
}

impl<T: FromRex> FromRex for BTreeMap<String, T> {
    fn from_rex(value: Value) -> Result<Self, EngineError> {
        let Value::Dict(fields) = value else {
            return Err(mismatch("dict", &value));
        };
        fields
            .into_iter()
            .map(|(name, value)| Ok((name, T::from_rex(value)?)))
            .collect()
    }
}

impl<T: IntoRex> IntoRex for HashMap<String, T> {
    fn into_rex(self) -> Result<Value, EngineError> {
        self.into_iter()
            .map(|(name, value)| Ok((name, value.into_rex()?)))
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map(Value::Dict)
    }
}

impl<T: FromRex> FromRex for HashMap<String, T> {
    fn from_rex(value: Value) -> Result<Self, EngineError> {
        let Value::Dict(fields) = value else {
            return Err(mismatch("dict", &value));
        };
        fields
            .into_iter()
            .map(|(name, value)| Ok((name, T::from_rex(value)?)))
            .collect()
    }
}

impl<T: IntoRex> IntoRex for Option<T> {
    fn into_rex(self) -> Result<Value, EngineError> {
        match self {
            Some(value) => Ok(Value::Adt(Symbol::intern("Some"), vec![value.into_rex()?])),
            None => Ok(Value::Adt(Symbol::intern("None"), vec![])),
        }
    }
}

impl<T: FromRex> FromRex for Option<T> {
    fn from_rex(value: Value) -> Result<Self, EngineError> {
        match value {
            Value::Adt(tag, mut args) if tag.as_ref() == "Some" && args.len() == 1 => {
                let value = args.pop().ok_or_else(|| {
                    EngineError::Internal("validated Some value had no argument".into())
                })?;
                Ok(Some(T::from_rex(value)?))
            }
            Value::Adt(tag, args) if tag.as_ref() == "None" && args.is_empty() => Ok(None),
            other => Err(mismatch("option", &other)),
        }
    }
}

impl<T: IntoRex, E: IntoRex> IntoRex for Result<T, E> {
    fn into_rex(self) -> Result<Value, EngineError> {
        match self {
            Ok(value) => Ok(Value::Adt(Symbol::intern("Ok"), vec![value.into_rex()?])),
            Err(error) => Ok(Value::Adt(Symbol::intern("Err"), vec![error.into_rex()?])),
        }
    }
}

impl<T: FromRex, E: FromRex> FromRex for Result<T, E> {
    fn from_rex(value: Value) -> Result<Self, EngineError> {
        match value {
            Value::Adt(tag, mut args) if tag.as_ref() == "Ok" && args.len() == 1 => {
                let value = args.pop().ok_or_else(|| {
                    EngineError::Internal("validated Ok value had no argument".into())
                })?;
                Ok(Ok(T::from_rex(value)?))
            }
            Value::Adt(tag, mut args) if tag.as_ref() == "Err" && args.len() == 1 => {
                let value = args.pop().ok_or_else(|| {
                    EngineError::Internal("validated Err value had no argument".into())
                })?;
                Ok(Err(E::from_rex(value)?))
            }
            other => Err(mismatch("result", &other)),
        }
    }
}

impl IntoRex for () {
    fn into_rex(self) -> Result<Value, EngineError> {
        Ok(Value::Tuple(Vec::new()))
    }
}

impl FromRex for () {
    fn from_rex(value: Value) -> Result<Self, EngineError> {
        match value {
            Value::Tuple(items) if items.is_empty() => Ok(()),
            other => Err(mismatch("unit", &other)),
        }
    }
}

macro_rules! impl_tuple_traits {
    ($($name:ident),+) => {
        impl<$($name: IntoRex),+> IntoRex for ($($name,)+) {
            #[allow(non_snake_case)]
            fn into_rex(self) -> Result<Value, EngineError> {
                let ($($name,)+) = self;
                Ok(Value::Tuple(vec![$($name.into_rex()?),+]))
            }
        }

        impl<$($name: FromRex),+> FromRex for ($($name,)+) {
            #[allow(non_snake_case)]
            fn from_rex(value: Value) -> Result<Self, EngineError> {
                let Value::Tuple(items) = value else {
                    return Err(mismatch("tuple", &value));
                };
                let mut items = items.into_iter();
                $(let $name = items.next().ok_or_else(|| EngineError::NativeType {
                    expected: "tuple".into(),
                    got: "tuple with too few items".into(),
                })?;)+
                if items.next().is_some() {
                    return Err(EngineError::NativeType {
                        expected: "tuple".into(),
                        got: "tuple with too many items".into(),
                    });
                }
                Ok(($(<$name as FromRex>::from_rex($name)?),+,))
            }
        }
    };
}

impl_tuple_traits!(A0);
impl_tuple_traits!(A0, A1);
impl_tuple_traits!(A0, A1, A2);
impl_tuple_traits!(A0, A1, A2, A3);
impl_tuple_traits!(A0, A1, A2, A3, A4);
impl_tuple_traits!(A0, A1, A2, A3, A4, A5);
impl_tuple_traits!(A0, A1, A2, A3, A4, A5, A6);
impl_tuple_traits!(A0, A1, A2, A3, A4, A5, A6, A7);
