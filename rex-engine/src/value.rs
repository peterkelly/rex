//! Core value representation for Rex.

use std::collections::BTreeMap;
use std::fmt::{self, Display, Formatter};
use std::marker::PhantomData;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use rex_ast::expr::{Symbol, sym, sym_eq};
use rex_ts::{Type, TypedExpr};
use uuid::Uuid;

use crate::EngineError;
use crate::Env;
use crate::engine::{NativeCallable, NativeFn, OverloadedFn};

#[derive(Default)]
pub struct Heap;

impl Heap {
    pub fn new() -> Self {
        Self
    }

    pub fn alloc_bool<'h>(&'h self, value: bool) -> Result<Pointer<'h>, EngineError> {
        Ok(Pointer {
            value: Box::new(Value::Bool(value)),
            _heap: PhantomData,
        })
    }

    pub fn alloc_u8<'h>(&'h self, value: u8) -> Result<Pointer<'h>, EngineError> {
        Ok(Pointer {
            value: Box::new(Value::U8(value)),
            _heap: PhantomData,
        })
    }

    pub fn alloc_u16<'h>(&'h self, value: u16) -> Result<Pointer<'h>, EngineError> {
        Ok(Pointer {
            value: Box::new(Value::U16(value)),
            _heap: PhantomData,
        })
    }

    pub fn alloc_u32<'h>(&'h self, value: u32) -> Result<Pointer<'h>, EngineError> {
        Ok(Pointer {
            value: Box::new(Value::U32(value)),
            _heap: PhantomData,
        })
    }

    pub fn alloc_u64<'h>(&'h self, value: u64) -> Result<Pointer<'h>, EngineError> {
        Ok(Pointer {
            value: Box::new(Value::U64(value)),
            _heap: PhantomData,
        })
    }

    pub fn alloc_i8<'h>(&'h self, value: i8) -> Result<Pointer<'h>, EngineError> {
        Ok(Pointer {
            value: Box::new(Value::I8(value)),
            _heap: PhantomData,
        })
    }

    pub fn alloc_i16<'h>(&'h self, value: i16) -> Result<Pointer<'h>, EngineError> {
        Ok(Pointer {
            value: Box::new(Value::I16(value)),
            _heap: PhantomData,
        })
    }

    pub fn alloc_i32<'h>(&'h self, value: i32) -> Result<Pointer<'h>, EngineError> {
        Ok(Pointer {
            value: Box::new(Value::I32(value)),
            _heap: PhantomData,
        })
    }

    pub fn alloc_i64<'h>(&'h self, value: i64) -> Result<Pointer<'h>, EngineError> {
        Ok(Pointer {
            value: Box::new(Value::I64(value)),
            _heap: PhantomData,
        })
    }

    pub fn alloc_f32<'h>(&'h self, value: f32) -> Result<Pointer<'h>, EngineError> {
        Ok(Pointer {
            value: Box::new(Value::F32(value)),
            _heap: PhantomData,
        })
    }

    pub fn alloc_f64<'h>(&'h self, value: f64) -> Result<Pointer<'h>, EngineError> {
        Ok(Pointer {
            value: Box::new(Value::F64(value)),
            _heap: PhantomData,
        })
    }

    pub fn alloc_string<'h>(&'h self, value: String) -> Result<Pointer<'h>, EngineError> {
        Ok(Pointer {
            value: Box::new(Value::String(value)),
            _heap: PhantomData,
        })
    }

    pub fn alloc_uuid<'h>(&'h self, value: Uuid) -> Result<Pointer<'h>, EngineError> {
        Ok(Pointer {
            value: Box::new(Value::Uuid(value)),
            _heap: PhantomData,
        })
    }

    pub fn alloc_datetime<'h>(&'h self, value: DateTime<Utc>) -> Result<Pointer<'h>, EngineError> {
        Ok(Pointer {
            value: Box::new(Value::DateTime(value)),
            _heap: PhantomData,
        })
    }

    pub fn alloc_value<'h>(&'h self, value: Value<'h>) -> Result<Pointer<'h>, EngineError> {
        Ok(Pointer {
            value: Box::new(value),
            _heap: PhantomData,
        })
    }

    pub fn alloc_tuple<'h>(&'h self, values: Vec<Pointer<'h>>) -> Result<Pointer<'h>, EngineError> {
        Ok(Pointer {
            value: Box::new(Value::Tuple(values)),
            _heap: PhantomData,
        })
    }

    pub fn alloc_array<'h>(&'h self, values: Vec<Pointer<'h>>) -> Result<Pointer<'h>, EngineError> {
        Ok(Pointer {
            value: Box::new(Value::Array(values)),
            _heap: PhantomData,
        })
    }

    pub fn alloc_dict<'h>(
        &'h self,
        values: BTreeMap<Symbol, Pointer<'h>>,
    ) -> Result<Pointer<'h>, EngineError> {
        Ok(Pointer {
            value: Box::new(Value::Dict(values)),
            _heap: PhantomData,
        })
    }

    pub fn alloc_adt<'h>(
        &'h self,
        name: Symbol,
        args: Vec<Pointer<'h>>,
    ) -> Result<Pointer<'h>, EngineError> {
        Ok(Pointer {
            value: Box::new(Value::Adt(name, args)),
            _heap: PhantomData,
        })
    }

    pub fn alloc_closure<'h>(
        &'h self,
        env: Env<'h>,
        param: Symbol,
        param_ty: Type,
        typ: Type,
        body: Arc<TypedExpr>,
    ) -> Result<Pointer<'h>, EngineError> {
        Ok(Pointer {
            value: Box::new(Value::Closure(Closure {
                env,
                param,
                param_ty,
                typ,
                body,
            })),
            _heap: PhantomData,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn alloc_native<'h>(
        &'h self,
        name: Symbol,
        arity: usize,
        typ: Type,
        func: NativeCallable<'h>,
        gas_cost: u64,
        applied: Vec<Pointer<'h>>,
        applied_types: Vec<Type>,
    ) -> Result<Pointer<'h>, EngineError> {
        Ok(Pointer {
            value: Box::new(Value::Native(NativeFn::from_parts(
                name,
                arity,
                typ,
                func,
                gas_cost,
                applied,
                applied_types,
            ))),
            _heap: PhantomData,
        })
    }

    pub fn alloc_overloaded<'h>(
        &'h self,
        name: Symbol,
        typ: Type,
        applied: Vec<Pointer<'h>>,
        applied_types: Vec<Type>,
    ) -> Result<Pointer<'h>, EngineError> {
        Ok(Pointer {
            value: Box::new(Value::Overloaded(OverloadedFn::from_parts(
                name,
                typ,
                applied,
                applied_types,
            ))),
            _heap: PhantomData,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Closure<'h> {
    pub env: Env<'h>,
    pub param: Symbol,
    pub param_ty: Type,
    pub typ: Type,
    pub body: Arc<TypedExpr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pointer<'h> {
    value: Box<Value<'h>>,
    _heap: PhantomData<&'h Heap>,
}

impl<'h> Pointer<'h> {
    pub fn get_value(&self, _heap: &'h Heap) -> Result<Value<'h>, EngineError> {
        Ok(self.value.as_ref().clone())
    }

    pub fn as_value(&self) -> &Value<'h> {
        self.value.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value<'h> {
    Bool(bool),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    String(String),
    Uuid(Uuid),
    DateTime(DateTime<Utc>),
    Tuple(Vec<Pointer<'h>>),
    Array(Vec<Pointer<'h>>),
    Dict(BTreeMap<Symbol, Pointer<'h>>),
    Adt(Symbol, Vec<Pointer<'h>>),
    Closure(Closure<'h>),
    Native(NativeFn<'h>),
    Overloaded(OverloadedFn<'h>),
}

impl<'h> Value<'h> {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Bool(..) => "bool",
            Value::U8(..) => "u8",
            Value::U16(..) => "u16",
            Value::U32(..) => "u32",
            Value::U64(..) => "u64",
            Value::I8(..) => "i8",
            Value::I16(..) => "i16",
            Value::I32(..) => "i32",
            Value::I64(..) => "i64",
            Value::F32(..) => "f32",
            Value::F64(..) => "f64",
            Value::String(..) => "string",
            Value::Uuid(..) => "uuid",
            Value::DateTime(..) => "datetime",
            Value::Tuple(..) => "tuple",
            Value::Array(..) => "array",
            Value::Dict(..) => "dict",
            Value::Adt(name, ..) if sym_eq(name, "Empty") || sym_eq(name, "Cons") => "list",
            Value::Adt(..) => "adt",
            Value::Closure(..) => "closure",
            Value::Native(..) => "native",
            Value::Overloaded(..) => "overloaded",
        }
    }
}

impl Display for Value<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Value::Bool(v) => write!(f, "{}", v),
            Value::U8(v) => write!(f, "{}u8", v),
            Value::U16(v) => write!(f, "{}u16", v),
            Value::U32(v) => write!(f, "{}u32", v),
            Value::U64(v) => write!(f, "{}u64", v),
            Value::I8(v) => write!(f, "{}i8", v),
            Value::I16(v) => write!(f, "{}i16", v),
            Value::I32(v) => write!(f, "{}i32", v),
            Value::I64(v) => write!(f, "{}i64", v),
            Value::F32(v) => write!(f, "{}f32", v),
            Value::F64(v) => write!(f, "{}f64", v),
            Value::String(v) => write!(f, "{:?}", v),
            Value::Uuid(v) => write!(f, "{}", v),
            Value::DateTime(v) => write!(f, "{}", v),
            Value::Tuple(xs) => {
                write!(f, "(")?;
                for (i, x) in xs.iter().enumerate() {
                    write!(f, "{}", x.as_value())?;
                    if i + 1 < xs.len() {
                        write!(f, ", ")?;
                    }
                }
                write!(f, ")")
            }
            Value::Array(xs) => {
                write!(f, "<array ")?;
                for (i, x) in xs.iter().enumerate() {
                    write!(f, "{}", x.as_value())?;
                    if i + 1 < xs.len() {
                        write!(f, ", ")?;
                    }
                }
                write!(f, ">")
            }
            Value::Dict(kvs) => {
                write!(f, "{{")?;
                for (i, (k, v)) in kvs.iter().enumerate() {
                    write!(f, "{} = {}", k, v.as_value())?;
                    if i + 1 < kvs.len() {
                        write!(f, ", ")?;
                    }
                }
                write!(f, "}}")
            }
            Value::Adt(name, args) => {
                if let Some(list) = list_to_vec_opt(self) {
                    write!(f, "[")?;
                    for (i, x) in list.iter().enumerate() {
                        write!(f, "{}", x.as_value())?;
                        if i + 1 < list.len() {
                            write!(f, ", ")?;
                        }
                    }
                    write!(f, "]")?;
                    return Ok(());
                }
                write!(f, "{}", name)?;
                for arg in args {
                    write!(f, " {}", arg.as_value())?;
                }
                Ok(())
            }
            Value::Closure(..) => write!(f, "<closure>"),
            Value::Native(native) => write!(f, "<native:{}>", native.name()),
            Value::Overloaded(over) => write!(f, "<overloaded:{}>", over.name()),
        }
    }
}

fn list_to_vec_opt<'h>(value: &Value<'h>) -> Option<Vec<Pointer<'h>>> {
    let mut out = Vec::new();
    let mut cur = value;
    loop {
        match cur {
            Value::Adt(tag, args) if sym_eq(tag, "Empty") && args.is_empty() => return Some(out),
            Value::Adt(tag, args) if sym_eq(tag, "Cons") && args.len() == 2 => {
                out.push(args[0].clone());
                cur = args[1].as_value();
            }
            _ => return None,
        }
    }
}

pub(crate) fn list_to_vec<'h>(
    value: &Value<'h>,
    name: &str,
) -> Result<Vec<Pointer<'h>>, EngineError> {
    let mut out = Vec::new();
    let mut cur = value;
    loop {
        match cur {
            Value::Adt(tag, args) if sym_eq(tag, "Empty") && args.is_empty() => return Ok(out),
            Value::Adt(tag, args) if sym_eq(tag, "Cons") && args.len() == 2 => {
                out.push(args[0].clone());
                cur = args[1].as_value();
            }
            _ => {
                return Err(EngineError::NativeType {
                    name: sym(name),
                    expected: "list".into(),
                    got: value.type_name().into(),
                });
            }
        }
    }
}

pub(crate) fn list_from_vec<'h>(
    heap: &'h Heap,
    values: Vec<Pointer<'h>>,
) -> Result<Value<'h>, EngineError> {
    let mut list = heap.alloc_adt(sym("Empty"), vec![])?;
    for v in values.into_iter().rev() {
        list = heap.alloc_adt(sym("Cons"), vec![v, list])?;
    }
    list.get_value(heap)
}

pub trait IntoValue<'h> {
    fn into_value(self, heap: &'h Heap) -> Result<Value<'h>, EngineError>;
}

pub trait FromValue<'h>: Sized {
    fn from_value(value: &Value<'h>, name: &str) -> Result<Self, EngineError>;
}

pub trait RexType {
    fn rex_type() -> Type;
}

impl<'h> IntoValue<'h> for Value<'h> {
    fn into_value(self, _heap: &'h Heap) -> Result<Value<'h>, EngineError> {
        Ok(self)
    }
}

impl<'h> IntoValue<'h> for bool {
    fn into_value(self, heap: &'h Heap) -> Result<Value<'h>, EngineError> {
        heap.alloc_bool(self)?.get_value(heap)
    }
}

impl<'h> IntoValue<'h> for u8 {
    fn into_value(self, heap: &'h Heap) -> Result<Value<'h>, EngineError> {
        heap.alloc_u8(self)?.get_value(heap)
    }
}

impl<'h> IntoValue<'h> for u16 {
    fn into_value(self, heap: &'h Heap) -> Result<Value<'h>, EngineError> {
        heap.alloc_u16(self)?.get_value(heap)
    }
}

impl<'h> IntoValue<'h> for u32 {
    fn into_value(self, heap: &'h Heap) -> Result<Value<'h>, EngineError> {
        heap.alloc_u32(self)?.get_value(heap)
    }
}

impl<'h> IntoValue<'h> for u64 {
    fn into_value(self, heap: &'h Heap) -> Result<Value<'h>, EngineError> {
        heap.alloc_u64(self)?.get_value(heap)
    }
}

impl<'h> IntoValue<'h> for i8 {
    fn into_value(self, heap: &'h Heap) -> Result<Value<'h>, EngineError> {
        heap.alloc_i8(self)?.get_value(heap)
    }
}

impl<'h> IntoValue<'h> for i16 {
    fn into_value(self, heap: &'h Heap) -> Result<Value<'h>, EngineError> {
        heap.alloc_i16(self)?.get_value(heap)
    }
}

impl<'h> IntoValue<'h> for i32 {
    fn into_value(self, heap: &'h Heap) -> Result<Value<'h>, EngineError> {
        heap.alloc_i32(self)?.get_value(heap)
    }
}

impl<'h> IntoValue<'h> for i64 {
    fn into_value(self, heap: &'h Heap) -> Result<Value<'h>, EngineError> {
        heap.alloc_i64(self)?.get_value(heap)
    }
}

impl<'h> IntoValue<'h> for f32 {
    fn into_value(self, heap: &'h Heap) -> Result<Value<'h>, EngineError> {
        heap.alloc_f32(self)?.get_value(heap)
    }
}

impl<'h> IntoValue<'h> for f64 {
    fn into_value(self, heap: &'h Heap) -> Result<Value<'h>, EngineError> {
        heap.alloc_f64(self)?.get_value(heap)
    }
}

impl<'h> IntoValue<'h> for String {
    fn into_value(self, heap: &'h Heap) -> Result<Value<'h>, EngineError> {
        heap.alloc_string(self)?.get_value(heap)
    }
}

impl<'h> IntoValue<'h> for &str {
    fn into_value(self, heap: &'h Heap) -> Result<Value<'h>, EngineError> {
        heap.alloc_string(self.to_string())?.get_value(heap)
    }
}

impl<'h, T: IntoValue<'h>> IntoValue<'h> for Vec<T> {
    fn into_value(self, heap: &'h Heap) -> Result<Value<'h>, EngineError> {
        let values = self
            .into_iter()
            .map(|v| v.into_value(heap))
            .collect::<Result<Vec<_>, _>>()?;
        let ptrs = values
            .into_iter()
            .map(|v| heap.alloc_value(v))
            .collect::<Result<Vec<_>, _>>()?;
        heap.alloc_array(ptrs)?.get_value(heap)
    }
}

impl<'h, T: IntoValue<'h>> IntoValue<'h> for Option<T> {
    fn into_value(self, heap: &'h Heap) -> Result<Value<'h>, EngineError> {
        match self {
            Some(v) => {
                let value = v.into_value(heap)?;
                let ptr = heap.alloc_value(value)?;
                heap.alloc_adt(sym("Some"), vec![ptr])?.get_value(heap)
            }
            None => heap.alloc_adt(sym("None"), vec![])?.get_value(heap),
        }
    }
}

impl<'h> IntoValue<'h> for Uuid {
    fn into_value(self, heap: &'h Heap) -> Result<Value<'h>, EngineError> {
        heap.alloc_uuid(self)?.get_value(heap)
    }
}

impl<'h> IntoValue<'h> for DateTime<Utc> {
    fn into_value(self, heap: &'h Heap) -> Result<Value<'h>, EngineError> {
        heap.alloc_datetime(self)?.get_value(heap)
    }
}

impl RexType for bool {
    fn rex_type() -> Type {
        Type::con("bool", 0)
    }
}

impl RexType for u8 {
    fn rex_type() -> Type {
        Type::con("u8", 0)
    }
}

impl RexType for u16 {
    fn rex_type() -> Type {
        Type::con("u16", 0)
    }
}

impl RexType for u32 {
    fn rex_type() -> Type {
        Type::con("u32", 0)
    }
}

impl RexType for u64 {
    fn rex_type() -> Type {
        Type::con("u64", 0)
    }
}

impl RexType for i8 {
    fn rex_type() -> Type {
        Type::con("i8", 0)
    }
}

impl RexType for i16 {
    fn rex_type() -> Type {
        Type::con("i16", 0)
    }
}

impl RexType for i32 {
    fn rex_type() -> Type {
        Type::con("i32", 0)
    }
}

impl RexType for i64 {
    fn rex_type() -> Type {
        Type::con("i64", 0)
    }
}

impl RexType for f32 {
    fn rex_type() -> Type {
        Type::con("f32", 0)
    }
}

impl RexType for f64 {
    fn rex_type() -> Type {
        Type::con("f64", 0)
    }
}

impl RexType for String {
    fn rex_type() -> Type {
        Type::con("string", 0)
    }
}

impl RexType for &str {
    fn rex_type() -> Type {
        Type::con("string", 0)
    }
}

impl RexType for Uuid {
    fn rex_type() -> Type {
        Type::con("uuid", 0)
    }
}

impl RexType for DateTime<Utc> {
    fn rex_type() -> Type {
        Type::con("datetime", 0)
    }
}

impl<T: RexType> RexType for Vec<T> {
    fn rex_type() -> Type {
        Type::app(Type::con("Array", 1), T::rex_type())
    }
}

impl<T: RexType> RexType for Option<T> {
    fn rex_type() -> Type {
        Type::app(Type::con("Option", 1), T::rex_type())
    }
}

impl<'h> FromValue<'h> for bool {
    fn from_value(value: &Value<'h>, name: &str) -> Result<Self, EngineError> {
        match value {
            Value::Bool(v) => Ok(*v),
            _ => Err(EngineError::NativeType {
                name: sym(name),
                expected: "bool".into(),
                got: value.type_name().into(),
            }),
        }
    }
}

macro_rules! impl_from_value_num {
    ($t:ty, $variant:ident, $label:literal) => {
        impl<'h> FromValue<'h> for $t {
            fn from_value(value: &Value<'h>, name: &str) -> Result<Self, EngineError> {
                match value {
                    Value::$variant(v) => Ok(*v as $t),
                    _ => Err(EngineError::NativeType {
                        name: sym(name),
                        expected: $label.into(),
                        got: value.type_name().into(),
                    }),
                }
            }
        }
    };
}

impl_from_value_num!(u8, U8, "u8");
impl_from_value_num!(u16, U16, "u16");
impl_from_value_num!(u32, U32, "u32");
impl_from_value_num!(u64, U64, "u64");
impl_from_value_num!(i8, I8, "i8");
impl_from_value_num!(i16, I16, "i16");
impl_from_value_num!(i32, I32, "i32");
impl_from_value_num!(i64, I64, "i64");
impl_from_value_num!(f32, F32, "f32");
impl_from_value_num!(f64, F64, "f64");

impl<'h> FromValue<'h> for String {
    fn from_value(value: &Value<'h>, name: &str) -> Result<Self, EngineError> {
        match value {
            Value::String(v) => Ok(v.clone()),
            _ => Err(EngineError::NativeType {
                name: sym(name),
                expected: "string".into(),
                got: value.type_name().into(),
            }),
        }
    }
}

impl<'h> FromValue<'h> for Uuid {
    fn from_value(value: &Value<'h>, name: &str) -> Result<Self, EngineError> {
        match value {
            Value::Uuid(v) => Ok(*v),
            _ => Err(EngineError::NativeType {
                name: sym(name),
                expected: "uuid".into(),
                got: value.type_name().into(),
            }),
        }
    }
}

impl<'h> FromValue<'h> for DateTime<Utc> {
    fn from_value(value: &Value<'h>, name: &str) -> Result<Self, EngineError> {
        match value {
            Value::DateTime(v) => Ok(*v),
            _ => Err(EngineError::NativeType {
                name: sym(name),
                expected: "datetime".into(),
                got: value.type_name().into(),
            }),
        }
    }
}

impl<'h> FromValue<'h> for Value<'h> {
    fn from_value(value: &Value<'h>, _name: &str) -> Result<Self, EngineError> {
        Ok(value.clone())
    }
}

impl<'h, T> FromValue<'h> for Vec<T>
where
    T: FromValue<'h>,
{
    fn from_value(value: &Value<'h>, name: &str) -> Result<Self, EngineError> {
        match value {
            Value::Array(xs) => {
                let mut ys = Vec::with_capacity(xs.len());
                for x in xs {
                    ys.push(T::from_value(x.as_value(), name)?);
                }
                Ok(ys)
            }
            _ => Err(EngineError::NativeType {
                name: sym(name),
                expected: "vec".into(),
                got: value.type_name().into(),
            }),
        }
    }
}

impl<'h, T> FromValue<'h> for Option<T>
where
    T: FromValue<'h>,
{
    fn from_value(value: &Value<'h>, name: &str) -> Result<Self, EngineError> {
        match value {
            Value::Adt(n, xs) if sym_eq(n, "Some") && xs.len() == 1 => {
                Ok(Some(T::from_value(xs[0].as_value(), name)?))
            }
            Value::Adt(n, xs) if sym_eq(n, "None") && xs.is_empty() => Ok(None),
            _ => Err(EngineError::NativeType {
                name: sym(name),
                expected: "vec".into(),
                got: value.type_name().into(),
            }),
        }
    }
}

impl RexType for () {
    fn rex_type() -> Type {
        Type::tuple(vec![])
    }
}

impl<'h> IntoValue<'h> for () {
    fn into_value(self, heap: &'h Heap) -> Result<Value<'h>, EngineError> {
        heap.alloc_tuple(vec![])?.get_value(heap)
    }
}

impl<'h> FromValue<'h> for () {
    fn from_value(value: &Value<'h>, name: &str) -> Result<Self, EngineError> {
        match value {
            Value::Tuple(items) if items.is_empty() => Ok(()),
            _ => Err(EngineError::NativeType {
                name: sym(name),
                expected: "tuple".into(),
                got: value.type_name().into(),
            }),
        }
    }
}

macro_rules! impl_tuple_traits {
    ($($name:ident),+) => {
        impl<$($name: RexType),+> RexType for ($($name,)+) {
            fn rex_type() -> Type {
                Type::tuple(vec![$($name::rex_type()),+])
            }
        }

        impl<'h, $($name: IntoValue<'h>),+> IntoValue<'h> for ($($name,)+) {
            #[allow(non_snake_case)]
            fn into_value(self, heap: &'h Heap) -> Result<Value<'h>, EngineError> {
                let ($($name,)+) = self;
                let values = vec![$($name.into_value(heap)?),+];
                let ptrs = values
                    .into_iter()
                    .map(|v| heap.alloc_value(v))
                    .collect::<Result<Vec<_>, _>>()?;
                heap.alloc_tuple(ptrs)?.get_value(heap)
            }
        }

        impl<'h, $($name: FromValue<'h>),+> FromValue<'h> for ($($name,)+) {
            #[allow(non_snake_case)]
            fn from_value(value: &Value<'h>, name: &str) -> Result<Self, EngineError> {
                match value {
                    Value::Tuple(items) => match items.as_slice() {
                        [$($name),+] => {
                            Ok(($(<$name as FromValue>::from_value($name.as_value(), name)?),+,))
                        }
                        _ => Err(EngineError::NativeType {
                            name: sym(name),
                            expected: "tuple".into(),
                            got: value.type_name().into(),
                        }),
                    },
                    _ => Err(EngineError::NativeType {
                        name: sym(name),
                        expected: "tuple".into(),
                        got: value.type_name().into(),
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
