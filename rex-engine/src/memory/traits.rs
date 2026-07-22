//! Conversion traits between Rust values and Rex heap values.

use std::{
    any::{Any, TypeId},
    convert::Infallible,
};

use chrono::{DateTime, Utc};
use rex_ast::Symbol;
use uuid::Uuid;

use crate::EngineError;

use super::{
    heap::{Handle, Heap, Pointer, wrong_heap_pointer},
    lists::collect_list_u8,
};

pub(crate) trait Collection {
    fn map_pointers<E>(
        &mut self,
        map: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E>;

    fn trace_pointers(&mut self, out: &mut Vec<Pointer>) {
        let result: Result<(), Infallible> = self.map_pointers(&mut |pointer| {
            out.push(pointer);
            Ok(pointer)
        });
        match result {
            Ok(()) => {}
            Err(never) => match never {},
        }
    }
}

/// Convert a Rust value into a heap-allocated Rex runtime value.
///
/// Typed host exports use this trait to turn Rust return values into Rex
/// values. Embedders can also call it directly when building values for dynamic
/// native functions or tests.
///
/// The implementation must allocate any runtime data in the supplied [`Heap`]
/// and return a rooted [`Handle`] that remains valid across evaluator heap
/// movement.
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

pub(super) fn handle_from_pointer(heap: &Heap, pointer: Pointer) -> Result<Handle, EngineError> {
    heap.handle(pointer)
}

impl IntoRex for Handle {
    fn into_rex(self, heap: &Heap) -> Result<Handle, EngineError> {
        let pointer = self.pointer()?;
        if pointer.heap_id != heap.id {
            return Err(wrong_heap_pointer(
                pointer.heap_id,
                heap.id,
                pointer.index,
                pointer.generation,
            ));
        }
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
            return handle_from_pointer(heap, heap.alloc_ptr_binary_list(*bytes)?);
        }

        let values = self
            .into_iter()
            .map(|value| value.into_rex(heap))
            .collect::<Result<Vec<_>, _>>()?;
        let pointers = values
            .iter()
            .map(Handle::pointer)
            .collect::<Result<Vec<_>, _>>()?;
        handle_from_pointer(heap, heap.alloc_ptr_list(pointers)?)
    }
}

impl<T: FromRex + 'static> FromRex for Vec<T> {
    fn from_rex(handle: &Handle) -> Result<Self, EngineError> {
        let heap = handle.heap();
        let pointer = handle.pointer()?;
        if TypeId::of::<T>() == TypeId::of::<u8>() {
            let bytes = heap.with_locked(|heap| collect_list_u8(heap, &pointer))?;
            let boxed: Box<dyn Any> = Box::new(bytes);
            return boxed
                .downcast::<Vec<T>>()
                .map(|values| *values)
                .map_err(|_| EngineError::Internal("Vec<u8> TypeId downcast failed".into()));
        }

        let pointers = heap.pointer_as_list(&pointer)?;
        let mut out = Vec::with_capacity(pointers.len());
        for pointer in pointers {
            let child = heap.handle(pointer)?;
            out.push(T::from_rex(&child)?);
        }
        Ok(out)
    }
}

impl<T: IntoRex> IntoRex for Option<T> {
    fn into_rex(self, heap: &Heap) -> Result<Handle, EngineError> {
        match self {
            Some(value) => {
                let value = value.into_rex(heap)?;
                let value_ptr = value.pointer()?;
                let ptr = heap.with_locked(|heap| {
                    Ok(heap
                        .alloc_ptr_adt(Symbol::intern("Some"), vec![value_ptr])?
                        .into_pointer())
                })?;
                handle_from_pointer(heap, ptr)
            }
            None => {
                let ptr = heap.with_locked(|heap| {
                    Ok(heap
                        .alloc_ptr_adt(Symbol::intern("None"), vec![])?
                        .into_pointer())
                })?;
                handle_from_pointer(heap, ptr)
            }
        }
    }
}

impl<T: FromRex> FromRex for Option<T> {
    fn from_rex(handle: &Handle) -> Result<Self, EngineError> {
        let heap = handle.heap();
        let pointer = handle.pointer()?;
        let (tag, args) = heap.with_locked(|heap| heap.pointer_as_adt(&pointer))?;
        if tag.as_ref() == "Some" && args.len() == 1 {
            let value = heap.handle(args[0])?;
            return Ok(Some(T::from_rex(&value)?));
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
            Ok(value) => {
                let value = value.into_rex(heap)?;
                let value_ptr = value.pointer()?;
                let ptr = heap.with_locked(|heap| {
                    Ok(heap
                        .alloc_ptr_adt(Symbol::intern("Ok"), vec![value_ptr])?
                        .into_pointer())
                })?;
                handle_from_pointer(heap, ptr)
            }
            Err(error) => {
                let error = error.into_rex(heap)?;
                let error_ptr = error.pointer()?;
                let ptr = heap.with_locked(|heap| {
                    Ok(heap
                        .alloc_ptr_adt(Symbol::intern("Err"), vec![error_ptr])?
                        .into_pointer())
                })?;
                handle_from_pointer(heap, ptr)
            }
        }
    }
}

impl<T: FromRex, E: FromRex> FromRex for Result<T, E> {
    fn from_rex(handle: &Handle) -> Result<Self, EngineError> {
        let heap = handle.heap();
        let pointer = handle.pointer()?;
        let (tag, args) = heap.with_locked(|heap| heap.pointer_as_adt(&pointer))?;
        if tag.as_ref() == "Ok" && args.len() == 1 {
            let value = heap.handle(args[0])?;
            return Ok(Ok(T::from_rex(&value)?));
        }
        if tag.as_ref() == "Err" && args.len() == 1 {
            let error = heap.handle(args[0])?;
            return Ok(Err(E::from_rex(&error)?));
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
                let ptrs = vec![$($name.pointer()?),+];
                let tuple_ptr = heap.with_locked(|heap| Ok(heap.alloc_ptr_tuple(ptrs)?.into_pointer()))?;
                handle_from_pointer(heap, tuple_ptr)
            }
        }

        impl<$($name: FromRex),+> FromRex for ($($name,)+) {
            #[allow(non_snake_case)]
            fn from_rex(handle: &Handle) -> Result<Self, EngineError> {
                let heap = handle.heap();
                let pointer = handle.pointer()?;
                let items = heap.with_locked(|heap| heap.pointer_as_tuple(&pointer))?;
                match items.as_slice() {
                    [$($name),+] => {
                        $(let $name = heap.handle(*$name)?;)+
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
