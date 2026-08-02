//! Conversion traits between Rust values and Rex heap values.

use std::any::{Any, TypeId};

use chrono::{DateTime, Utc};
use rex_ast::Symbol;
use uuid::Uuid;

use crate::EngineError;

use super::heap::{Handle, Heap};

/// Convert a Rust value into a heap-allocated Rex runtime value.
///
/// Typed host exports use this trait to turn Rust return values into Rex
/// values. Embedders can also call it directly when building values for dynamic
/// native functions or tests.
///
/// The implementation must allocate any runtime data in the supplied [`Heap`]
/// and return a rooted [`Handle`] that remains valid across later allocations,
/// collections, thread transfers, and async suspension.
pub trait IntoRex {
    /// Allocate `self` into `heap` and return the resulting Rex value handle.
    fn into_rex(self, heap: &Heap) -> Result<Handle, EngineError>;
}

/// Decode a Rex runtime value into a Rust value.
///
/// Typed host exports use this trait to turn Rex arguments into Rust function
/// parameters. Implementations should validate that the [`Handle`] has the
/// expected shape and return an [`EngineError`] when the runtime value cannot be
/// represented as `Self`.
pub trait FromRex: Sized {
    /// Read `handle` as `Self`.
    fn from_rex(handle: &Handle) -> Result<Self, EngineError>;
}

impl IntoRex for Handle {
    fn into_rex(self, heap: &Heap) -> Result<Handle, EngineError> {
        self.ensure_heap(heap)?;
        Ok(self)
    }
}

impl FromRex for Handle {
    fn from_rex(handle: &Handle) -> Result<Self, EngineError> {
        Ok(handle.clone())
    }
}

macro_rules! impl_rex_scalar {
    ($t:ty, $alloc:ident, $read:ident) => {
        impl IntoRex for $t {
            fn into_rex(self, heap: &Heap) -> Result<Handle, EngineError> {
                heap.$alloc(self)
            }
        }

        impl FromRex for $t {
            fn from_rex(handle: &Handle) -> Result<Self, EngineError> {
                handle.$read()
            }
        }
    };
}

impl_rex_scalar!(bool, alloc_bool, as_bool);
impl_rex_scalar!(u8, alloc_u8, as_u8);
impl_rex_scalar!(u16, alloc_u16, as_u16);
impl_rex_scalar!(u32, alloc_u32, as_u32);
impl_rex_scalar!(u64, alloc_u64, as_u64);
impl_rex_scalar!(i8, alloc_i8, as_i8);
impl_rex_scalar!(i16, alloc_i16, as_i16);
impl_rex_scalar!(i32, alloc_i32, as_i32);
impl_rex_scalar!(i64, alloc_i64, as_i64);
impl_rex_scalar!(f32, alloc_f32, as_f32);
impl_rex_scalar!(f64, alloc_f64, as_f64);
impl_rex_scalar!(String, alloc_string, as_string);
impl_rex_scalar!(Uuid, alloc_uuid, as_uuid);
impl_rex_scalar!(DateTime<Utc>, alloc_datetime, as_datetime);

impl IntoRex for &str {
    fn into_rex(self, heap: &Heap) -> Result<Handle, EngineError> {
        heap.alloc_string(self.to_string())
    }
}

impl<T: IntoRex + 'static> IntoRex for Vec<T> {
    fn into_rex(self, heap: &Heap) -> Result<Handle, EngineError> {
        if TypeId::of::<T>() == TypeId::of::<u8>() {
            let boxed: Box<dyn Any> = Box::new(self);
            let bytes = boxed
                .downcast::<Vec<u8>>()
                .map_err(|_| EngineError::Internal("Vec<u8> TypeId downcast failed".into()))?;
            return heap.alloc_binary_list(*bytes);
        }

        let values = self
            .into_iter()
            .map(|value| value.into_rex(heap))
            .collect::<Result<Vec<_>, _>>()?;
        heap.alloc_list(values)
    }
}

impl<T: FromRex + 'static> FromRex for Vec<T> {
    fn from_rex(handle: &Handle) -> Result<Self, EngineError> {
        if TypeId::of::<T>() == TypeId::of::<u8>() {
            let bytes = handle.as_binary_list()?;
            let boxed: Box<dyn Any> = Box::new(bytes);
            return boxed
                .downcast::<Vec<T>>()
                .map(|values| *values)
                .map_err(|_| EngineError::Internal("Vec<u8> TypeId downcast failed".into()));
        }

        let items = handle.as_list()?;
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            out.push(T::from_rex(&item)?);
        }
        Ok(out)
    }
}

impl<T: IntoRex> IntoRex for Option<T> {
    fn into_rex(self, heap: &Heap) -> Result<Handle, EngineError> {
        match self {
            Some(value) => heap.alloc_adt(Symbol::intern("Some"), vec![value.into_rex(heap)?]),
            None => heap.alloc_adt(Symbol::intern("None"), vec![]),
        }
    }
}

impl<T: FromRex> FromRex for Option<T> {
    fn from_rex(handle: &Handle) -> Result<Self, EngineError> {
        let (tag, args) = handle.as_adt()?;
        if tag.as_ref() == "Some" && args.len() == 1 {
            return Ok(Some(T::from_rex(&args[0])?));
        }
        if tag.as_ref() == "None" && args.is_empty() {
            return Ok(None);
        }
        Err(EngineError::NativeType {
            expected: "option".into(),
            got: handle.type_name()?.into(),
        })
    }
}

impl<T: IntoRex, E: IntoRex> IntoRex for Result<T, E> {
    fn into_rex(self, heap: &Heap) -> Result<Handle, EngineError> {
        match self {
            Ok(value) => heap.alloc_adt(Symbol::intern("Ok"), vec![value.into_rex(heap)?]),
            Err(error) => heap.alloc_adt(Symbol::intern("Err"), vec![error.into_rex(heap)?]),
        }
    }
}

impl<T: FromRex, E: FromRex> FromRex for Result<T, E> {
    fn from_rex(handle: &Handle) -> Result<Self, EngineError> {
        let (tag, args) = handle.as_adt()?;
        if tag.as_ref() == "Ok" && args.len() == 1 {
            return Ok(Ok(T::from_rex(&args[0])?));
        }
        if tag.as_ref() == "Err" && args.len() == 1 {
            return Ok(Err(E::from_rex(&args[0])?));
        }
        Err(EngineError::NativeType {
            expected: "result".into(),
            got: handle.type_name()?.into(),
        })
    }
}

impl IntoRex for () {
    fn into_rex(self, heap: &Heap) -> Result<Handle, EngineError> {
        heap.alloc_tuple(Vec::new())
    }
}

impl FromRex for () {
    fn from_rex(handle: &Handle) -> Result<Self, EngineError> {
        if handle.as_tuple()?.is_empty() {
            Ok(())
        } else {
            Err(EngineError::NativeType {
                expected: "tuple".into(),
                got: handle.type_name()?.into(),
            })
        }
    }
}

macro_rules! impl_tuple_traits {
    ($($name:ident),+) => {
        impl<$($name: IntoRex),+> IntoRex for ($($name,)+) {
            #[allow(non_snake_case)]
            fn into_rex(self, heap: &Heap) -> Result<Handle, EngineError> {
                let ($($name,)+) = self;
                $(let $name = $name.into_rex(heap)?;)+
                heap.alloc_tuple(vec![$($name),+])
            }
        }

        impl<$($name: FromRex),+> FromRex for ($($name,)+) {
            #[allow(non_snake_case)]
            fn from_rex(handle: &Handle) -> Result<Self, EngineError> {
                let items = handle.as_tuple()?;
                match items.as_slice() {
                    [$($name),+] => {
                        Ok(($(<$name as FromRex>::from_rex(&$name)?),+,))
                    }
                    _ => Err(EngineError::NativeType {
                        expected: "tuple".into(),
                        got: handle.type_name()?.into(),
                    }),
                }
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
