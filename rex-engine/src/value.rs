//! Core value representation for Rex.

use std::collections::BTreeMap;
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use rex_ast::expr::{Symbol, sym, sym_eq};
use rex_ts::{Type, TypedExpr};
use uuid::Uuid;

use crate::EngineError;
use crate::Env;
use crate::engine::{NativeFn, OverloadedFn};

#[derive(Clone, Debug, PartialEq)]
pub struct Closure {
    pub env: Env,
    pub param: Symbol,
    pub param_ty: Type,
    pub typ: Type,
    pub body: Arc<TypedExpr>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
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
    Tuple(Vec<Value>),
    Array(Vec<Value>),
    Dict(BTreeMap<Symbol, Value>),
    Adt(Symbol, Vec<Value>),
    Closure(Closure),
    Native(NativeFn),
    Overloaded(OverloadedFn),
}

impl Value {
    pub(crate) fn type_name(&self) -> &'static str {
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

impl Display for Value {
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
                    write!(f, "{}", x)?;
                    if i + 1 < xs.len() {
                        write!(f, ", ")?;
                    }
                }
                write!(f, ")")
            }
            Value::Array(xs) => {
                write!(f, "<array ")?;
                for (i, x) in xs.iter().enumerate() {
                    write!(f, "{}", x)?;
                    if i + 1 < xs.len() {
                        write!(f, ", ")?;
                    }
                }
                write!(f, ">")
            }
            Value::Dict(kvs) => {
                write!(f, "{{")?;
                for (i, (k, v)) in kvs.iter().enumerate() {
                    write!(f, "{} = {}", k, v)?;
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
                        write!(f, "{}", x)?;
                        if i + 1 < list.len() {
                            write!(f, ", ")?;
                        }
                    }
                    write!(f, "]")?;
                    return Ok(());
                }
                write!(f, "{}", name)?;
                for arg in args {
                    write!(f, " {}", arg)?;
                }
                Ok(())
            }
            Value::Closure(..) => write!(f, "<closure>"),
            Value::Native(native) => write!(f, "<native:{}>", native.name()),
            Value::Overloaded(over) => write!(f, "<overloaded:{}>", over.name()),
        }
    }
}

fn list_to_vec_opt(value: &Value) -> Option<Vec<Value>> {
    let mut out = Vec::new();
    let mut cur = value;
    loop {
        match cur {
            Value::Adt(tag, args) if sym_eq(tag, "Empty") && args.is_empty() => return Some(out),
            Value::Adt(tag, args) if sym_eq(tag, "Cons") && args.len() == 2 => {
                out.push(args[0].clone());
                cur = &args[1];
            }
            _ => return None,
        }
    }
}

pub(crate) fn list_to_vec(value: &Value, name: &str) -> Result<Vec<Value>, EngineError> {
    let mut out = Vec::new();
    let mut cur = value;
    loop {
        match cur {
            Value::Adt(tag, args) if sym_eq(tag, "Empty") && args.is_empty() => return Ok(out),
            Value::Adt(tag, args) if sym_eq(tag, "Cons") && args.len() == 2 => {
                out.push(args[0].clone());
                cur = &args[1];
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

pub(crate) fn list_from_vec(values: Vec<Value>) -> Value {
    let mut list = Value::Adt(sym("Empty"), vec![]);
    for v in values.into_iter().rev() {
        list = Value::Adt(sym("Cons"), vec![v, list]);
    }
    list
}

pub trait IntoValue {
    fn into_value(self) -> Value;
}

pub trait FromValue: Sized {
    fn from_value(value: &Value, name: &str) -> Result<Self, EngineError>;
}

pub trait RexType {
    fn rex_type() -> Type;
}

impl IntoValue for Value {
    fn into_value(self) -> Value {
        self
    }
}

impl IntoValue for bool {
    fn into_value(self) -> Value {
        Value::Bool(self)
    }
}

impl IntoValue for u8 {
    fn into_value(self) -> Value {
        Value::U8(self)
    }
}

impl IntoValue for u16 {
    fn into_value(self) -> Value {
        Value::U16(self)
    }
}

impl IntoValue for u32 {
    fn into_value(self) -> Value {
        Value::U32(self)
    }
}

impl IntoValue for u64 {
    fn into_value(self) -> Value {
        Value::U64(self)
    }
}

impl IntoValue for i8 {
    fn into_value(self) -> Value {
        Value::I8(self)
    }
}

impl IntoValue for i16 {
    fn into_value(self) -> Value {
        Value::I16(self)
    }
}

impl IntoValue for i32 {
    fn into_value(self) -> Value {
        Value::I32(self)
    }
}

impl IntoValue for i64 {
    fn into_value(self) -> Value {
        Value::I64(self)
    }
}

impl IntoValue for f32 {
    fn into_value(self) -> Value {
        Value::F32(self)
    }
}

impl IntoValue for f64 {
    fn into_value(self) -> Value {
        Value::F64(self)
    }
}

impl IntoValue for String {
    fn into_value(self) -> Value {
        Value::String(self)
    }
}

impl IntoValue for &str {
    fn into_value(self) -> Value {
        Value::String(self.to_string())
    }
}

impl<T: IntoValue> IntoValue for Vec<T> {
    fn into_value(self) -> Value {
        Value::Array(self.into_iter().map(IntoValue::into_value).collect())
    }
}

impl<T: IntoValue> IntoValue for Option<T> {
    fn into_value(self) -> Value {
        match self {
            Some(v) => Value::Adt(sym("Some"), vec![v.into_value()]),
            None => Value::Adt(sym("None"), vec![]),
        }
    }
}

impl IntoValue for Uuid {
    fn into_value(self) -> Value {
        Value::Uuid(self)
    }
}

impl IntoValue for DateTime<Utc> {
    fn into_value(self) -> Value {
        Value::DateTime(self)
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

impl FromValue for bool {
    fn from_value(value: &Value, name: &str) -> Result<Self, EngineError> {
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
        impl FromValue for $t {
            fn from_value(value: &Value, name: &str) -> Result<Self, EngineError> {
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

impl FromValue for String {
    fn from_value(value: &Value, name: &str) -> Result<Self, EngineError> {
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

impl FromValue for Uuid {
    fn from_value(value: &Value, name: &str) -> Result<Self, EngineError> {
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

impl FromValue for DateTime<Utc> {
    fn from_value(value: &Value, name: &str) -> Result<Self, EngineError> {
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

impl FromValue for Value {
    fn from_value(value: &Value, _name: &str) -> Result<Self, EngineError> {
        Ok(value.clone())
    }
}

impl<T> FromValue for Vec<T>
where
    T: FromValue,
{
    fn from_value(value: &Value, name: &str) -> Result<Self, EngineError> {
        match value {
            Value::Array(xs) => {
                let mut ys = Vec::with_capacity(xs.len());
                for x in xs {
                    ys.push(T::from_value(x, name)?);
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

impl<T> FromValue for Option<T>
where
    T: FromValue,
{
    fn from_value(value: &Value, name: &str) -> Result<Self, EngineError> {
        match value {
            Value::Adt(n, xs) if sym_eq(n, "Some") && xs.len() == 1 => {
                Ok(Some(T::from_value(&xs[0], name)?))
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

impl IntoValue for () {
    fn into_value(self) -> Value {
        Value::Tuple(vec![])
    }
}

impl FromValue for () {
    fn from_value(value: &Value, name: &str) -> Result<Self, EngineError> {
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

        impl<$($name: IntoValue),+> IntoValue for ($($name,)+) {
            #[allow(non_snake_case)]
            fn into_value(self) -> Value {
                let ($($name,)+) = self;
                Value::Tuple(vec![$($name.into_value()),+])
            }
        }

        impl<$($name: FromValue),+> FromValue for ($($name,)+) {
            #[allow(non_snake_case)]
            fn from_value(value: &Value, name: &str) -> Result<Self, EngineError> {
                match value {
                    Value::Tuple(items) => match items.as_slice() {
                        [$($name),+] => {
                            Ok(($(<$name as FromValue>::from_value($name, name)?),+,))
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
