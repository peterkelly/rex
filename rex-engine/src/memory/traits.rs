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
    heap::{Cell, Handle, Heap, Pointer, wrong_heap_pointer},
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

pub(crate) trait IntoPointer {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError>;
}

pub(crate) trait FromPointer: Sized {
    fn from_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError>;
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

impl IntoPointer for Cell {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.with_locked(|heap| Ok(heap.alloc_ptr_cell(self)?.into_pointer()))
    }
}

impl IntoPointer for &Cell {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.with_locked(|heap| Ok(heap.alloc_ptr_cell(self.clone())?.into_pointer()))
    }
}

impl IntoPointer for Pointer {
    fn into_pointer(self, _heap: &Heap) -> Result<Pointer, EngineError> {
        Ok(self)
    }
}

impl IntoPointer for &Pointer {
    fn into_pointer(self, _heap: &Heap) -> Result<Pointer, EngineError> {
        Ok(*self)
    }
}

impl IntoPointer for bool {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.with_locked(|heap| Ok(heap.alloc_ptr_bool(self)?.into_pointer()))
    }
}

impl IntoPointer for u8 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.with_locked(|heap| Ok(heap.alloc_ptr_u8(self)?.into_pointer()))
    }
}

impl IntoPointer for u16 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.with_locked(|heap| Ok(heap.alloc_ptr_u16(self)?.into_pointer()))
    }
}

impl IntoPointer for u32 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.with_locked(|heap| Ok(heap.alloc_ptr_u32(self)?.into_pointer()))
    }
}

impl IntoPointer for u64 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.with_locked(|heap| Ok(heap.alloc_ptr_u64(self)?.into_pointer()))
    }
}

impl IntoPointer for i8 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.with_locked(|heap| Ok(heap.alloc_ptr_i8(self)?.into_pointer()))
    }
}

impl IntoPointer for i16 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.with_locked(|heap| Ok(heap.alloc_ptr_i16(self)?.into_pointer()))
    }
}

impl IntoPointer for i32 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.with_locked(|heap| Ok(heap.alloc_ptr_i32(self)?.into_pointer()))
    }
}

impl IntoPointer for i64 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.with_locked(|heap| Ok(heap.alloc_ptr_i64(self)?.into_pointer()))
    }
}

impl IntoPointer for f32 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.with_locked(|heap| Ok(heap.alloc_ptr_f32(self)?.into_pointer()))
    }
}

impl IntoPointer for f64 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.with_locked(|heap| Ok(heap.alloc_ptr_f64(self)?.into_pointer()))
    }
}

impl IntoPointer for String {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.with_locked(|heap| Ok(heap.alloc_ptr_string(self)?.into_pointer()))
    }
}

impl IntoPointer for &str {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.with_locked(|heap| Ok(heap.alloc_ptr_string(self.to_string())?.into_pointer()))
    }
}

impl<T: IntoPointer + 'static> IntoPointer for Vec<T> {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        if TypeId::of::<T>() == TypeId::of::<u8>() {
            let boxed: Box<dyn Any> = Box::new(self);
            let bytes = boxed
                .downcast::<Vec<u8>>()
                .map_err(|_| EngineError::Internal("Vec<u8> TypeId downcast failed".into()))?;
            return heap.alloc_ptr_binary_list(*bytes);
        }

        let mut roots = Vec::new();
        for value in self {
            let pointer = value.into_pointer(heap)?;
            roots.push(heap.temp_roots(vec![pointer])?);
        }
        let ptrs = roots
            .iter()
            .map(|root| root.get(0))
            .collect::<Result<Vec<_>, _>>()?;
        heap.alloc_ptr_list(ptrs)
    }
}

impl<T: IntoPointer> IntoPointer for Option<T> {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        match self {
            Some(v) => {
                let ptr = v.into_pointer(heap)?;
                heap.with_locked(|heap| {
                    Ok(heap
                        .alloc_ptr_adt(Symbol::intern("Some"), vec![ptr])?
                        .into_pointer())
                })
            }
            None => heap.with_locked(|heap| {
                Ok(heap
                    .alloc_ptr_adt(Symbol::intern("None"), vec![])?
                    .into_pointer())
            }),
        }
    }
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

macro_rules! impl_rex_via_pointer {
    ($t:ty) => {
        impl IntoRex for $t {
            fn into_rex(self, heap: &Heap) -> Result<Handle, EngineError> {
                handle_from_pointer(heap, self.into_pointer(heap)?)
            }
        }

        impl FromRex for $t {
            fn from_rex(handle: &Handle) -> Result<Self, EngineError> {
                let pointer = handle.pointer()?;
                Self::from_pointer(handle.heap(), &pointer)
            }
        }
    };
}

impl_rex_via_pointer!(bool);
impl_rex_via_pointer!(u8);
impl_rex_via_pointer!(u16);
impl_rex_via_pointer!(u32);
impl_rex_via_pointer!(u64);
impl_rex_via_pointer!(i8);
impl_rex_via_pointer!(i16);
impl_rex_via_pointer!(i32);
impl_rex_via_pointer!(i64);
impl_rex_via_pointer!(f32);
impl_rex_via_pointer!(f64);
impl_rex_via_pointer!(String);
impl_rex_via_pointer!(Uuid);
impl_rex_via_pointer!(DateTime<Utc>);

impl IntoRex for &str {
    fn into_rex(self, heap: &Heap) -> Result<Handle, EngineError> {
        handle_from_pointer(heap, self.into_pointer(heap)?)
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

impl IntoPointer for Uuid {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.with_locked(|heap| Ok(heap.alloc_ptr_uuid(self)?.into_pointer()))
    }
}

impl IntoPointer for DateTime<Utc> {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.with_locked(|heap| Ok(heap.alloc_ptr_datetime(self)?.into_pointer()))
    }
}

impl FromPointer for bool {
    fn from_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError> {
        heap.with_locked(|heap| heap.pointer_as_bool(pointer))
    }
}

macro_rules! impl_from_pointer_num {
    ($t:ty, $pointer_as:ident) => {
        impl FromPointer for $t {
            fn from_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError> {
                heap.with_locked(|heap| heap.$pointer_as(pointer).map(|v| v as $t))
            }
        }
    };
}

impl_from_pointer_num!(u8, pointer_as_u8);
impl_from_pointer_num!(u16, pointer_as_u16);
impl_from_pointer_num!(u32, pointer_as_u32);
impl_from_pointer_num!(u64, pointer_as_u64);
impl_from_pointer_num!(i8, pointer_as_i8);
impl_from_pointer_num!(i16, pointer_as_i16);
impl_from_pointer_num!(i32, pointer_as_i32);
impl_from_pointer_num!(i64, pointer_as_i64);
impl_from_pointer_num!(f32, pointer_as_f32);
impl_from_pointer_num!(f64, pointer_as_f64);

impl FromPointer for String {
    fn from_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError> {
        heap.with_locked(|heap| heap.pointer_as_string(pointer))
    }
}

impl FromPointer for Uuid {
    fn from_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError> {
        heap.with_locked(|heap| heap.pointer_as_uuid(pointer))
    }
}

impl FromPointer for DateTime<Utc> {
    fn from_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError> {
        heap.with_locked(|heap| heap.pointer_as_datetime(pointer))
    }
}

impl FromPointer for Cell {
    fn from_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError> {
        heap.clone_cell(pointer)
    }
}

impl<T> FromPointer for Vec<T>
where
    T: FromPointer + 'static,
{
    fn from_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError> {
        if TypeId::of::<T>() == TypeId::of::<u8>() {
            let bytes = heap.with_locked(|heap| collect_list_u8(heap, pointer))?;
            let boxed: Box<dyn Any> = Box::new(bytes);
            return boxed
                .downcast::<Vec<T>>()
                .map(|values| *values)
                .map_err(|_| EngineError::Internal("Vec<u8> TypeId downcast failed".into()));
        }

        let xs = heap.pointer_as_list(pointer)?;
        let mut ys = Vec::with_capacity(xs.len());
        for x in &xs {
            ys.push(T::from_pointer(heap, x)?);
        }
        Ok(ys)
    }
}

impl<T> FromPointer for Option<T>
where
    T: FromPointer,
{
    fn from_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError> {
        let (tag, args) = heap.with_locked(|heap| heap.pointer_as_adt(pointer))?;
        if tag.as_ref() == "Some" && args.len() == 1 {
            return Ok(Some(T::from_pointer(heap, &args[0])?));
        }
        if tag.as_ref() == "None" && args.is_empty() {
            return Ok(None);
        }
        Err(EngineError::NativeType {
            expected: "vec".into(),
            got: heap.with_locked(|heap| heap.type_name(pointer))?.into(),
        })
    }
}

impl<T: IntoPointer, E: IntoPointer> IntoPointer for Result<T, E> {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        match self {
            Ok(v) => {
                let ptr = v.into_pointer(heap)?;
                heap.with_locked(|heap| {
                    Ok(heap
                        .alloc_ptr_adt(Symbol::intern("Ok"), vec![ptr])?
                        .into_pointer())
                })
            }
            Err(e) => {
                let ptr = e.into_pointer(heap)?;
                heap.with_locked(|heap| {
                    Ok(heap
                        .alloc_ptr_adt(Symbol::intern("Err"), vec![ptr])?
                        .into_pointer())
                })
            }
        }
    }
}

impl<T, E> FromPointer for Result<T, E>
where
    T: FromPointer,
    E: FromPointer,
{
    fn from_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError> {
        let (tag, args) = heap.with_locked(|heap| heap.pointer_as_adt(pointer))?;
        if tag.as_ref() == "Ok" && args.len() == 1 {
            return Ok(Ok(T::from_pointer(heap, &args[0])?));
        }
        if tag.as_ref() == "Err" && args.len() == 1 {
            return Ok(Err(E::from_pointer(heap, &args[0])?));
        }
        Err(EngineError::NativeType {
            expected: "result".into(),
            got: heap.with_locked(|heap| heap.type_name(pointer))?.into(),
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

impl IntoPointer for () {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.with_locked(|heap| Ok(heap.alloc_ptr_tuple(vec![])?.into_pointer()))
    }
}

impl FromPointer for () {
    fn from_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError> {
        let items = heap.with_locked(|heap| heap.pointer_as_tuple(pointer))?;
        if items.is_empty() {
            Ok(())
        } else {
            Err(EngineError::NativeType {
                expected: "tuple".into(),
                got: heap.with_locked(|heap| heap.type_name(pointer))?.into(),
            })
        }
    }
}

impl IntoRex for () {
    fn into_rex(self, heap: &Heap) -> Result<Handle, EngineError> {
        handle_from_pointer(heap, self.into_pointer(heap)?)
    }
}

impl FromRex for () {
    fn from_rex(handle: &Handle) -> Result<Self, EngineError> {
        let pointer = handle.pointer()?;
        Self::from_pointer(handle.heap(), &pointer)
    }
}

macro_rules! impl_tuple_traits {
    ($($name:ident),+) => {
        impl<$($name: IntoPointer),+> IntoPointer for ($($name,)+) {
            #[allow(non_snake_case)]
            fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
                let ($($name,)+) = self;
                let roots = vec![$({
                    let pointer = $name.into_pointer(heap)?;
                    heap.temp_roots(vec![pointer])?
                }),+];
                let ptrs = roots
                    .iter()
                    .map(|root| root.get(0))
                    .collect::<Result<Vec<_>, _>>()?;
                heap.with_locked(|heap| Ok(heap.alloc_ptr_tuple(ptrs)?.into_pointer()))
            }
        }

        impl<$($name: FromPointer),+> FromPointer for ($($name,)+) {
            #[allow(non_snake_case)]
            fn from_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError> {
                let items = heap.with_locked(|heap| heap.pointer_as_tuple(pointer))?;
                match items.as_slice() {
                    [$($name),+] => {
                        Ok(($(<$name as FromPointer>::from_pointer(heap, $name)?),+,))
                    }
                    _ => Err(EngineError::NativeType {
                        expected: "tuple".into(),
                        got: heap.with_locked(|heap| heap.type_name(pointer))?.into(),
                    }),
                }
            }
        }

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
