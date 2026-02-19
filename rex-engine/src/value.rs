//! Core value representation for Rex.

use std::collections::{BTreeMap, HashSet};
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rex_ast::expr::{Symbol, sym, sym_eq};
use rex_ts::{Type, TypedExpr};
use uuid::Uuid;

use crate::EngineError;
use crate::Env;
use crate::engine::{NativeFn, OverloadedFn};

#[derive(Default)]
struct HeapState {
    slots: Vec<HeapSlot>,
    free_list: Vec<u32>,
}

#[derive(Clone)]
struct HeapSlot {
    generation: u32,
    value: Option<Arc<Value>>,
}

pub struct Heap {
    id: u64,
    state: Mutex<HeapState>,
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

impl Heap {
    /// Create a heap for a lexical scope.
    pub fn scoped<R>(f: impl FnOnce(&Heap) -> R) -> R {
        let heap = Heap::new();
        f(&heap)
    }

    pub fn new() -> Self {
        static NEXT_HEAP_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_HEAP_ID.fetch_add(1, Ordering::Relaxed);
        Self {
            id,
            state: Mutex::new(HeapState::default()),
        }
    }

    fn invalid_pointer(heap_id: u64, index: u32, generation: u32) -> EngineError {
        EngineError::Internal(format!(
            "invalid heap pointer (heap_id={}, index={}, generation={})",
            heap_id, index, generation
        ))
    }

    fn wrong_heap_pointer(
        pointer_heap_id: u64,
        heap_id: u64,
        index: u32,
        generation: u32,
    ) -> EngineError {
        EngineError::Internal(format!(
            "heap pointer belongs to different heap (pointer_heap_id={}, heap_id={}, index={}, generation={})",
            pointer_heap_id, heap_id, index, generation
        ))
    }

    fn alloc_slot(&self, value: Value) -> Result<Pointer, EngineError> {
        let (index, generation) = self.alloc_slot_raw(value)?;
        Ok(Pointer {
            heap_id: self.id,
            index,
            generation,
        })
    }

    pub fn get(&self, pointer: &Pointer) -> Result<ValueRef, EngineError> {
        if pointer.heap_id != self.id {
            return Err(Self::wrong_heap_pointer(
                pointer.heap_id,
                self.id,
                pointer.index,
                pointer.generation,
            ));
        }
        self.read_slot(pointer.index, pointer.generation)
            .map(ValueRef::from_arc)
    }

    pub fn type_name(&self, pointer: &Pointer) -> Result<&'static str, EngineError> {
        self.get(pointer)
            .map(|value| self.type_name_of_value(value.as_ref()))
    }

    pub(crate) fn type_name_of_value(&self, value: &Value) -> &'static str {
        value.value_type_name()
    }

    pub fn pointer_as_bool(&self, pointer: &Pointer) -> Result<bool, EngineError> {
        self.get(pointer)?.as_ref().value_as_bool()
    }

    pub fn pointer_as_u8(&self, pointer: &Pointer) -> Result<u8, EngineError> {
        self.get(pointer)?.as_ref().value_as_u8()
    }

    pub fn pointer_as_u16(&self, pointer: &Pointer) -> Result<u16, EngineError> {
        self.get(pointer)?.as_ref().value_as_u16()
    }

    pub fn pointer_as_u32(&self, pointer: &Pointer) -> Result<u32, EngineError> {
        self.get(pointer)?.as_ref().value_as_u32()
    }

    pub fn pointer_as_u64(&self, pointer: &Pointer) -> Result<u64, EngineError> {
        self.get(pointer)?.as_ref().value_as_u64()
    }

    pub fn pointer_as_i8(&self, pointer: &Pointer) -> Result<i8, EngineError> {
        self.get(pointer)?.as_ref().value_as_i8()
    }

    pub fn pointer_as_i16(&self, pointer: &Pointer) -> Result<i16, EngineError> {
        self.get(pointer)?.as_ref().value_as_i16()
    }

    pub fn pointer_as_i32(&self, pointer: &Pointer) -> Result<i32, EngineError> {
        self.get(pointer)?.as_ref().value_as_i32()
    }

    pub fn pointer_as_i64(&self, pointer: &Pointer) -> Result<i64, EngineError> {
        self.get(pointer)?.as_ref().value_as_i64()
    }

    pub fn pointer_as_f32(&self, pointer: &Pointer) -> Result<f32, EngineError> {
        self.get(pointer)?.as_ref().value_as_f32()
    }

    pub fn pointer_as_f64(&self, pointer: &Pointer) -> Result<f64, EngineError> {
        self.get(pointer)?.as_ref().value_as_f64()
    }

    pub fn pointer_as_string(&self, pointer: &Pointer) -> Result<String, EngineError> {
        self.get(pointer)?.as_ref().value_as_string()
    }

    pub fn pointer_as_uuid(&self, pointer: &Pointer) -> Result<Uuid, EngineError> {
        self.get(pointer)?.as_ref().value_as_uuid()
    }

    pub fn pointer_as_datetime(&self, pointer: &Pointer) -> Result<DateTime<Utc>, EngineError> {
        self.get(pointer)?.as_ref().value_as_datetime()
    }

    pub fn pointer_as_tuple(&self, pointer: &Pointer) -> Result<Vec<Pointer>, EngineError> {
        self.get(pointer)?.as_ref().value_as_tuple()
    }

    pub fn pointer_as_array(&self, pointer: &Pointer) -> Result<Vec<Pointer>, EngineError> {
        self.get(pointer)?.as_ref().value_as_array()
    }

    pub fn pointer_as_dict(
        &self,
        pointer: &Pointer,
    ) -> Result<BTreeMap<Symbol, Pointer>, EngineError> {
        self.get(pointer)?.as_ref().value_as_dict()
    }

    pub fn pointer_as_adt(&self, pointer: &Pointer) -> Result<(Symbol, Vec<Pointer>), EngineError> {
        self.get(pointer)?.as_ref().value_as_adt()
    }

    pub fn pointer_as_uninitialized(&self, pointer: &Pointer) -> Result<Symbol, EngineError> {
        self.get(pointer)?.as_ref().value_as_uninitialized()
    }

    pub fn pointer_as_closure(&self, pointer: &Pointer) -> Result<Closure, EngineError> {
        self.get(pointer)?.as_ref().value_as_closure()
    }

    pub fn pointer_as_native(&self, pointer: &Pointer) -> Result<NativeFn, EngineError> {
        self.get(pointer)?.as_ref().value_as_native()
    }

    pub fn pointer_as_overloaded(&self, pointer: &Pointer) -> Result<OverloadedFn, EngineError> {
        self.get(pointer)?.as_ref().value_as_overloaded()
    }

    pub fn alloc_bool(&self, value: bool) -> Result<Pointer, EngineError> {
        self.alloc_slot(Value::Bool(value))
    }

    pub fn alloc_u8(&self, value: u8) -> Result<Pointer, EngineError> {
        self.alloc_slot(Value::U8(value))
    }

    pub fn alloc_u16(&self, value: u16) -> Result<Pointer, EngineError> {
        self.alloc_slot(Value::U16(value))
    }

    pub fn alloc_u32(&self, value: u32) -> Result<Pointer, EngineError> {
        self.alloc_slot(Value::U32(value))
    }

    pub fn alloc_u64(&self, value: u64) -> Result<Pointer, EngineError> {
        self.alloc_slot(Value::U64(value))
    }

    pub fn alloc_i8(&self, value: i8) -> Result<Pointer, EngineError> {
        self.alloc_slot(Value::I8(value))
    }

    pub fn alloc_i16(&self, value: i16) -> Result<Pointer, EngineError> {
        self.alloc_slot(Value::I16(value))
    }

    pub fn alloc_i32(&self, value: i32) -> Result<Pointer, EngineError> {
        self.alloc_slot(Value::I32(value))
    }

    pub fn alloc_i64(&self, value: i64) -> Result<Pointer, EngineError> {
        self.alloc_slot(Value::I64(value))
    }

    pub fn alloc_f32(&self, value: f32) -> Result<Pointer, EngineError> {
        self.alloc_slot(Value::F32(value))
    }

    pub fn alloc_f64(&self, value: f64) -> Result<Pointer, EngineError> {
        self.alloc_slot(Value::F64(value))
    }

    pub fn alloc_string(&self, value: String) -> Result<Pointer, EngineError> {
        self.alloc_slot(Value::String(value))
    }

    pub fn alloc_uuid(&self, value: Uuid) -> Result<Pointer, EngineError> {
        self.alloc_slot(Value::Uuid(value))
    }

    pub fn alloc_datetime(&self, value: DateTime<Utc>) -> Result<Pointer, EngineError> {
        self.alloc_slot(Value::DateTime(value))
    }

    pub fn alloc_value(&self, value: Value) -> Result<Pointer, EngineError> {
        self.alloc_slot(value)
    }

    pub(crate) fn alloc_uninitialized(&self, name: Symbol) -> Result<Pointer, EngineError> {
        self.alloc_slot(Value::Uninitialized(name))
    }

    pub fn alloc_tuple(&self, values: Vec<Pointer>) -> Result<Pointer, EngineError> {
        self.alloc_slot(Value::Tuple(values))
    }

    pub fn alloc_array(&self, values: Vec<Pointer>) -> Result<Pointer, EngineError> {
        self.alloc_slot(Value::Array(values))
    }

    pub fn alloc_dict(&self, values: BTreeMap<Symbol, Pointer>) -> Result<Pointer, EngineError> {
        self.alloc_slot(Value::Dict(values))
    }

    pub fn alloc_adt(&self, name: Symbol, args: Vec<Pointer>) -> Result<Pointer, EngineError> {
        self.alloc_slot(Value::Adt(name, args))
    }

    pub fn alloc_closure(
        &self,
        env: Env,
        param: Symbol,
        param_ty: Type,
        typ: Type,
        body: Arc<TypedExpr>,
    ) -> Result<Pointer, EngineError> {
        self.alloc_slot(Value::Closure(Closure {
            env,
            param,
            param_ty,
            typ,
            body,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn alloc_native(
        &self,
        native_id: u64,
        name: Symbol,
        arity: usize,
        typ: Type,
        gas_cost: u64,
        applied: Vec<Pointer>,
        applied_types: Vec<Type>,
    ) -> Result<Pointer, EngineError> {
        self.alloc_slot(Value::Native(NativeFn::from_parts(
            native_id,
            name,
            arity,
            typ,
            gas_cost,
            applied,
            applied_types,
        )))
    }

    pub fn alloc_overloaded(
        &self,
        name: Symbol,
        typ: Type,
        applied: Vec<Pointer>,
        applied_types: Vec<Type>,
    ) -> Result<Pointer, EngineError> {
        self.alloc_slot(Value::Overloaded(OverloadedFn::from_parts(
            name,
            typ,
            applied,
            applied_types,
        )))
    }

    pub(crate) fn overwrite(&self, pointer: &Pointer, value: Value) -> Result<(), EngineError> {
        if pointer.heap_id != self.id {
            return Err(Self::wrong_heap_pointer(
                pointer.heap_id,
                self.id,
                pointer.index,
                pointer.generation,
            ));
        }

        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::Internal("heap state poisoned".into()))?;
        let slot = state
            .slots
            .get_mut(pointer.index as usize)
            .ok_or_else(|| Heap::invalid_pointer(self.id, pointer.index, pointer.generation))?;
        if slot.generation != pointer.generation {
            return Err(Heap::invalid_pointer(
                self.id,
                pointer.index,
                pointer.generation,
            ));
        }
        slot.value = Some(Arc::new(value));
        Ok(())
    }

    fn alloc_slot_raw(&self, value: Value) -> Result<(u32, u32), EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::Internal("heap state poisoned".into()))?;

        if let Some(index) = state.free_list.pop() {
            let slot = state
                .slots
                .get_mut(index as usize)
                .ok_or_else(|| EngineError::Internal("heap free-list corruption".into()))?;
            slot.value = Some(Arc::new(value));
            return Ok((index, slot.generation));
        }

        let index = u32::try_from(state.slots.len())
            .map_err(|_| EngineError::Internal("heap exhausted: too many slots".into()))?;
        state.slots.push(HeapSlot {
            generation: 0,
            value: Some(Arc::new(value)),
        });
        Ok((index, 0))
    }

    fn read_slot(&self, index: u32, generation: u32) -> Result<Arc<Value>, EngineError> {
        let state = self
            .state
            .lock()
            .map_err(|_| EngineError::Internal("heap state poisoned".into()))?;
        let slot = state
            .slots
            .get(index as usize)
            .ok_or_else(|| Heap::invalid_pointer(self.id, index, generation))?;
        if slot.generation != generation {
            return Err(Heap::invalid_pointer(self.id, index, generation));
        }
        slot.value
            .as_ref()
            .cloned()
            .ok_or_else(|| Heap::invalid_pointer(self.id, index, generation))
    }
}

#[derive(Clone)]
pub struct ValueRef {
    value: Arc<Value>,
}

impl ValueRef {
    fn from_arc(value: Arc<Value>) -> Self {
        Self { value }
    }
}

impl AsRef<Value> for ValueRef {
    fn as_ref(&self) -> &Value {
        self.value.as_ref()
    }
}

impl Deref for ValueRef {
    type Target = Value;

    fn deref(&self) -> &Self::Target {
        self.value.as_ref()
    }
}

#[derive(Clone)]
pub struct Closure {
    pub env: Env,
    pub param: Symbol,
    pub param_ty: Type,
    pub typ: Type,
    pub body: Arc<TypedExpr>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Pointer {
    heap_id: u64,
    index: u32,
    generation: u32,
}

#[derive(Clone)]
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
    Tuple(Vec<Pointer>),
    Array(Vec<Pointer>),
    Dict(BTreeMap<Symbol, Pointer>),
    Adt(Symbol, Vec<Pointer>),
    Uninitialized(Symbol),
    Closure(Closure),
    Native(NativeFn),
    Overloaded(OverloadedFn),
}

impl Value {
    pub fn value_type_name(&self) -> &'static str {
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
            Value::Uninitialized(..) => "uninitialized",
            Value::Closure(..) => "closure",
            Value::Native(..) => "native",
            Value::Overloaded(..) => "overloaded",
        }
    }

    fn value_type_error(&self, expected: &'static str) -> EngineError {
        EngineError::NativeType {
            expected: expected.to_string(),
            got: self.value_type_name().to_string(),
        }
    }

    pub fn value_as_bool(&self) -> Result<bool, EngineError> {
        match self {
            Value::Bool(v) => Ok(*v),
            _ => Err(self.value_type_error("bool")),
        }
    }

    pub fn value_as_u8(&self) -> Result<u8, EngineError> {
        match self {
            Value::U8(v) => Ok(*v),
            _ => Err(self.value_type_error("u8")),
        }
    }

    pub fn value_as_u16(&self) -> Result<u16, EngineError> {
        match self {
            Value::U16(v) => Ok(*v),
            _ => Err(self.value_type_error("u16")),
        }
    }

    pub fn value_as_u32(&self) -> Result<u32, EngineError> {
        match self {
            Value::U32(v) => Ok(*v),
            _ => Err(self.value_type_error("u32")),
        }
    }

    pub fn value_as_u64(&self) -> Result<u64, EngineError> {
        match self {
            Value::U64(v) => Ok(*v),
            _ => Err(self.value_type_error("u64")),
        }
    }

    pub fn value_as_i8(&self) -> Result<i8, EngineError> {
        match self {
            Value::I8(v) => Ok(*v),
            _ => Err(self.value_type_error("i8")),
        }
    }

    pub fn value_as_i16(&self) -> Result<i16, EngineError> {
        match self {
            Value::I16(v) => Ok(*v),
            _ => Err(self.value_type_error("i16")),
        }
    }

    pub fn value_as_i32(&self) -> Result<i32, EngineError> {
        match self {
            Value::I32(v) => Ok(*v),
            _ => Err(self.value_type_error("i32")),
        }
    }

    pub fn value_as_i64(&self) -> Result<i64, EngineError> {
        match self {
            Value::I64(v) => Ok(*v),
            _ => Err(self.value_type_error("i64")),
        }
    }

    pub fn value_as_f32(&self) -> Result<f32, EngineError> {
        match self {
            Value::F32(v) => Ok(*v),
            _ => Err(self.value_type_error("f32")),
        }
    }

    pub fn value_as_f64(&self) -> Result<f64, EngineError> {
        match self {
            Value::F64(v) => Ok(*v),
            _ => Err(self.value_type_error("f64")),
        }
    }

    pub fn value_as_string(&self) -> Result<String, EngineError> {
        match self {
            Value::String(v) => Ok(v.clone()),
            _ => Err(self.value_type_error("string")),
        }
    }

    pub fn value_as_uuid(&self) -> Result<Uuid, EngineError> {
        match self {
            Value::Uuid(v) => Ok(*v),
            _ => Err(self.value_type_error("uuid")),
        }
    }

    pub fn value_as_datetime(&self) -> Result<DateTime<Utc>, EngineError> {
        match self {
            Value::DateTime(v) => Ok(*v),
            _ => Err(self.value_type_error("datetime")),
        }
    }

    pub fn value_as_tuple(&self) -> Result<Vec<Pointer>, EngineError> {
        match self {
            Value::Tuple(v) => Ok(v.clone()),
            _ => Err(self.value_type_error("tuple")),
        }
    }

    pub fn value_as_array(&self) -> Result<Vec<Pointer>, EngineError> {
        match self {
            Value::Array(v) => Ok(v.clone()),
            _ => Err(self.value_type_error("array")),
        }
    }

    pub fn value_as_dict(&self) -> Result<BTreeMap<Symbol, Pointer>, EngineError> {
        match self {
            Value::Dict(v) => Ok(v.clone()),
            _ => Err(self.value_type_error("dict")),
        }
    }

    pub fn value_as_adt(&self) -> Result<(Symbol, Vec<Pointer>), EngineError> {
        match self {
            Value::Adt(name, args) => Ok((name.clone(), args.clone())),
            _ => Err(self.value_type_error("adt")),
        }
    }

    pub fn value_as_uninitialized(&self) -> Result<Symbol, EngineError> {
        match self {
            Value::Uninitialized(name) => Ok(name.clone()),
            _ => Err(self.value_type_error("uninitialized")),
        }
    }

    pub fn value_as_closure(&self) -> Result<Closure, EngineError> {
        match self {
            Value::Closure(v) => Ok(v.clone()),
            _ => Err(self.value_type_error("closure")),
        }
    }

    pub fn value_as_native(&self) -> Result<NativeFn, EngineError> {
        match self {
            Value::Native(v) => Ok(v.clone()),
            _ => Err(self.value_type_error("native")),
        }
    }

    pub fn value_as_overloaded(&self) -> Result<OverloadedFn, EngineError> {
        match self {
            Value::Overloaded(v) => Ok(v.clone()),
            _ => Err(self.value_type_error("overloaded")),
        }
    }
}

type PointerKey = (u64, u32, u32);
type PointerPairKey = (PointerKey, PointerKey);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValueDisplayOptions {
    pub include_numeric_suffixes: bool,
    pub strip_internal_snippet_qualifiers: bool,
}

impl Default for ValueDisplayOptions {
    fn default() -> Self {
        Self::docs()
    }
}

impl ValueDisplayOptions {
    pub fn unsanitized() -> Self {
        Self {
            include_numeric_suffixes: true,
            strip_internal_snippet_qualifiers: false,
        }
    }

    pub fn docs() -> Self {
        Self {
            include_numeric_suffixes: false,
            strip_internal_snippet_qualifiers: true,
        }
    }
}

fn maybe_strip_snippet_qualifier(name: &str, opts: ValueDisplayOptions) -> String {
    if !opts.strip_internal_snippet_qualifiers || !name.starts_with("@snippet") {
        return name.to_string();
    }
    if let Some((_, tail)) = name.rsplit_once('.') {
        return tail.to_string();
    }
    name.to_string()
}

fn pointer_key(pointer: &Pointer) -> PointerKey {
    (pointer.heap_id, pointer.index, pointer.generation)
}

fn canonical_pointer_pair(lhs: PointerKey, rhs: PointerKey) -> PointerPairKey {
    if lhs <= rhs { (lhs, rhs) } else { (rhs, lhs) }
}

fn pointer_debug_inner(
    heap: &Heap,
    pointer: &Pointer,
    active: &mut HashSet<PointerKey>,
) -> Result<String, EngineError> {
    let key = pointer_key(pointer);
    if !active.insert(key) {
        return Ok(format!("<cycle:{}:{}>", pointer.index, pointer.generation));
    }
    let value = heap.get(pointer)?;
    let out = value_debug_inner(heap, &value, active);
    active.remove(&key);
    out
}

fn pointer_display_inner(
    heap: &Heap,
    pointer: &Pointer,
    active: &mut HashSet<PointerKey>,
    opts: ValueDisplayOptions,
) -> Result<String, EngineError> {
    let key = pointer_key(pointer);
    if !active.insert(key) {
        return Ok(format!("<cycle:{}:{}>", pointer.index, pointer.generation));
    }
    let value = heap.get(pointer)?;
    let out = value_display_inner(heap, &value, active, opts);
    active.remove(&key);
    out
}

fn env_debug_inner(
    heap: &Heap,
    env: &Env,
    active: &mut HashSet<PointerKey>,
) -> Result<String, EngineError> {
    let mut bindings = env.bindings().iter().collect::<Vec<_>>();
    bindings.sort_by(|(lhs, _), (rhs, _)| lhs.as_ref().cmp(rhs.as_ref()));

    let mut rendered = Vec::with_capacity(bindings.len());
    for (name, pointer) in bindings {
        rendered.push(format!(
            "{} = {}",
            name,
            pointer_debug_inner(heap, pointer, active)?
        ));
    }

    let frame = format!("{{{}}}", rendered.join(", "));
    match env.parent() {
        Some(parent) => Ok(format!(
            "{frame} :: {}",
            env_debug_inner(heap, parent, active)?
        )),
        None => Ok(frame),
    }
}

fn closure_debug_inner(
    heap: &Heap,
    closure: &Closure,
    active: &mut HashSet<PointerKey>,
) -> Result<String, EngineError> {
    Ok(format!(
        "Closure {{ env: {}, param: {}, param_ty: {}, typ: {}, body: {:?} }}",
        env_debug_inner(heap, &closure.env, active)?,
        closure.param,
        closure.param_ty,
        closure.typ,
        closure.body
    ))
}

fn value_debug_inner(
    heap: &Heap,
    value: &Value,
    active: &mut HashSet<PointerKey>,
) -> Result<String, EngineError> {
    Ok(match value {
        Value::Bool(v) => v.to_string(),
        Value::U8(v) => format!("{v}u8"),
        Value::U16(v) => format!("{v}u16"),
        Value::U32(v) => format!("{v}u32"),
        Value::U64(v) => format!("{v}u64"),
        Value::I8(v) => format!("{v}i8"),
        Value::I16(v) => format!("{v}i16"),
        Value::I32(v) => format!("{v}i32"),
        Value::I64(v) => format!("{v}i64"),
        Value::F32(v) => format!("{v}f32"),
        Value::F64(v) => format!("{v}f64"),
        Value::String(v) => format!("{v:?}"),
        Value::Uuid(v) => v.to_string(),
        Value::DateTime(v) => v.to_string(),
        Value::Tuple(values) => {
            let items = values
                .iter()
                .map(|pointer| pointer_debug_inner(heap, pointer, active))
                .collect::<Result<Vec<_>, _>>()?;
            format!("({})", items.join(", "))
        }
        Value::Array(values) => {
            let items = values
                .iter()
                .map(|pointer| pointer_debug_inner(heap, pointer, active))
                .collect::<Result<Vec<_>, _>>()?;
            format!("<array {}>", items.join(", "))
        }
        Value::Dict(values) => {
            let mut items = values.iter().collect::<Vec<_>>();
            items.sort_by(|(lhs, _), (rhs, _)| lhs.as_ref().cmp(rhs.as_ref()));
            let items = items
                .into_iter()
                .map(|(name, pointer)| {
                    Ok(format!(
                        "{} = {}",
                        name,
                        pointer_debug_inner(heap, pointer, active)?
                    ))
                })
                .collect::<Result<Vec<_>, EngineError>>()?;
            format!("{{{}}}", items.join(", "))
        }
        Value::Adt(name, args) => {
            if let Some(values) = list_to_vec_opt(heap, value)? {
                let items = values
                    .iter()
                    .map(|pointer| pointer_debug_inner(heap, pointer, active))
                    .collect::<Result<Vec<_>, _>>()?;
                format!("[{}]", items.join(", "))
            } else {
                let mut rendered = vec![name.to_string()];
                for pointer in args {
                    rendered.push(pointer_debug_inner(heap, pointer, active)?);
                }
                rendered.join(" ")
            }
        }
        Value::Uninitialized(name) => format!("<uninitialized:{name}>"),
        Value::Closure(closure) => closure_debug_inner(heap, closure, active)?,
        Value::Native(native) => format!("<native:{}>", native.name()),
        Value::Overloaded(over) => format!("<overloaded:{}>", over.name()),
    })
}

fn value_display_inner(
    heap: &Heap,
    value: &Value,
    active: &mut HashSet<PointerKey>,
    opts: ValueDisplayOptions,
) -> Result<String, EngineError> {
    Ok(match value {
        Value::Bool(v) => v.to_string(),
        Value::U8(v) => {
            if opts.include_numeric_suffixes {
                format!("{v}u8")
            } else {
                v.to_string()
            }
        }
        Value::U16(v) => {
            if opts.include_numeric_suffixes {
                format!("{v}u16")
            } else {
                v.to_string()
            }
        }
        Value::U32(v) => {
            if opts.include_numeric_suffixes {
                format!("{v}u32")
            } else {
                v.to_string()
            }
        }
        Value::U64(v) => {
            if opts.include_numeric_suffixes {
                format!("{v}u64")
            } else {
                v.to_string()
            }
        }
        Value::I8(v) => {
            if opts.include_numeric_suffixes {
                format!("{v}i8")
            } else {
                v.to_string()
            }
        }
        Value::I16(v) => {
            if opts.include_numeric_suffixes {
                format!("{v}i16")
            } else {
                v.to_string()
            }
        }
        Value::I32(v) => {
            if opts.include_numeric_suffixes {
                format!("{v}i32")
            } else {
                v.to_string()
            }
        }
        Value::I64(v) => {
            if opts.include_numeric_suffixes {
                format!("{v}i64")
            } else {
                v.to_string()
            }
        }
        Value::F32(v) => {
            if opts.include_numeric_suffixes {
                format!("{v}f32")
            } else {
                v.to_string()
            }
        }
        Value::F64(v) => {
            if opts.include_numeric_suffixes {
                format!("{v}f64")
            } else {
                v.to_string()
            }
        }
        Value::String(v) => format!("{v:?}"),
        Value::Uuid(v) => v.to_string(),
        Value::DateTime(v) => v.to_string(),
        Value::Tuple(values) => {
            let items = values
                .iter()
                .map(|pointer| pointer_display_inner(heap, pointer, active, opts))
                .collect::<Result<Vec<_>, _>>()?;
            format!("({})", items.join(", "))
        }
        Value::Array(values) => {
            let items = values
                .iter()
                .map(|pointer| pointer_display_inner(heap, pointer, active, opts))
                .collect::<Result<Vec<_>, _>>()?;
            format!("<array {}>", items.join(", "))
        }
        Value::Dict(values) => {
            let mut items = values.iter().collect::<Vec<_>>();
            items.sort_by(|(lhs, _), (rhs, _)| lhs.as_ref().cmp(rhs.as_ref()));
            let items = items
                .into_iter()
                .map(|(name, pointer)| {
                    Ok(format!(
                        "{} = {}",
                        name,
                        pointer_display_inner(heap, pointer, active, opts)?
                    ))
                })
                .collect::<Result<Vec<_>, EngineError>>()?;
            format!("{{{}}}", items.join(", "))
        }
        Value::Adt(name, args) => {
            if let Some(values) = list_to_vec_opt(heap, value)? {
                let items = values
                    .iter()
                    .map(|pointer| pointer_display_inner(heap, pointer, active, opts))
                    .collect::<Result<Vec<_>, _>>()?;
                format!("[{}]", items.join(", "))
            } else {
                let mut rendered = vec![maybe_strip_snippet_qualifier(name.as_ref(), opts)];
                for pointer in args {
                    rendered.push(pointer_display_inner(heap, pointer, active, opts)?);
                }
                rendered.join(" ")
            }
        }
        Value::Uninitialized(name) => format!("<uninitialized:{name}>"),
        Value::Closure(..) => "<closure>".to_string(),
        Value::Native(native) => format!("<native:{}>", native.name()),
        Value::Overloaded(over) => format!("<overloaded:{}>", over.name()),
    })
}

pub fn value_debug(heap: &Heap, value: &Value) -> Result<String, EngineError> {
    let mut active = HashSet::new();
    value_debug_inner(heap, value, &mut active)
}

pub fn pointer_display(heap: &Heap, pointer: &Pointer) -> Result<String, EngineError> {
    pointer_display_with(heap, pointer, ValueDisplayOptions::default())
}

pub fn pointer_display_with(
    heap: &Heap,
    pointer: &Pointer,
    opts: ValueDisplayOptions,
) -> Result<String, EngineError> {
    let mut active = HashSet::new();
    pointer_display_inner(heap, pointer, &mut active, opts)
}

pub fn closure_debug(heap: &Heap, closure: &Closure) -> Result<String, EngineError> {
    let mut active = HashSet::new();
    closure_debug_inner(heap, closure, &mut active)
}

fn pointer_eq_inner(
    heap: &Heap,
    lhs: &Pointer,
    rhs: &Pointer,
    seen: &mut HashSet<PointerPairKey>,
) -> Result<bool, EngineError> {
    let lhs_key = pointer_key(lhs);
    let rhs_key = pointer_key(rhs);
    if lhs_key == rhs_key {
        return Ok(true);
    }
    let pair = canonical_pointer_pair(lhs_key, rhs_key);
    if !seen.insert(pair) {
        return Ok(true);
    }
    let lhs_value = heap.get(lhs)?;
    let rhs_value = heap.get(rhs)?;
    value_eq_inner(heap, &lhs_value, &rhs_value, seen)
}

fn env_eq_inner(
    heap: &Heap,
    lhs: &Env,
    rhs: &Env,
    seen: &mut HashSet<PointerPairKey>,
) -> Result<bool, EngineError> {
    if lhs.bindings().len() != rhs.bindings().len() {
        return Ok(false);
    }
    for (name, lhs_pointer) in lhs.bindings() {
        let Some(rhs_pointer) = rhs.bindings().get(name) else {
            return Ok(false);
        };
        if !pointer_eq_inner(heap, lhs_pointer, rhs_pointer, seen)? {
            return Ok(false);
        }
    }
    match (lhs.parent(), rhs.parent()) {
        (Some(lhs_parent), Some(rhs_parent)) => env_eq_inner(heap, lhs_parent, rhs_parent, seen),
        (None, None) => Ok(true),
        _ => Ok(false),
    }
}

fn closure_eq_inner(
    heap: &Heap,
    lhs: &Closure,
    rhs: &Closure,
    seen: &mut HashSet<PointerPairKey>,
) -> Result<bool, EngineError> {
    if lhs.param != rhs.param
        || lhs.param_ty != rhs.param_ty
        || lhs.typ != rhs.typ
        || lhs.body != rhs.body
    {
        return Ok(false);
    }
    env_eq_inner(heap, &lhs.env, &rhs.env, seen)
}

fn value_eq_inner(
    heap: &Heap,
    lhs: &Value,
    rhs: &Value,
    seen: &mut HashSet<PointerPairKey>,
) -> Result<bool, EngineError> {
    match (lhs, rhs) {
        (Value::Bool(lhs), Value::Bool(rhs)) => Ok(lhs == rhs),
        (Value::U8(lhs), Value::U8(rhs)) => Ok(lhs == rhs),
        (Value::U16(lhs), Value::U16(rhs)) => Ok(lhs == rhs),
        (Value::U32(lhs), Value::U32(rhs)) => Ok(lhs == rhs),
        (Value::U64(lhs), Value::U64(rhs)) => Ok(lhs == rhs),
        (Value::I8(lhs), Value::I8(rhs)) => Ok(lhs == rhs),
        (Value::I16(lhs), Value::I16(rhs)) => Ok(lhs == rhs),
        (Value::I32(lhs), Value::I32(rhs)) => Ok(lhs == rhs),
        (Value::I64(lhs), Value::I64(rhs)) => Ok(lhs == rhs),
        (Value::F32(lhs), Value::F32(rhs)) => Ok(lhs == rhs),
        (Value::F64(lhs), Value::F64(rhs)) => Ok(lhs == rhs),
        (Value::String(lhs), Value::String(rhs)) => Ok(lhs == rhs),
        (Value::Uuid(lhs), Value::Uuid(rhs)) => Ok(lhs == rhs),
        (Value::DateTime(lhs), Value::DateTime(rhs)) => Ok(lhs == rhs),
        (Value::Tuple(lhs), Value::Tuple(rhs)) | (Value::Array(lhs), Value::Array(rhs)) => {
            if lhs.len() != rhs.len() {
                return Ok(false);
            }
            for (lhs, rhs) in lhs.iter().zip(rhs.iter()) {
                if !pointer_eq_inner(heap, lhs, rhs, seen)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        (Value::Dict(lhs), Value::Dict(rhs)) => {
            if lhs.len() != rhs.len() {
                return Ok(false);
            }
            for (name, lhs_pointer) in lhs {
                let Some(rhs_pointer) = rhs.get(name) else {
                    return Ok(false);
                };
                if !pointer_eq_inner(heap, lhs_pointer, rhs_pointer, seen)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        (Value::Adt(lhs_name, lhs_args), Value::Adt(rhs_name, rhs_args)) => {
            if lhs_name != rhs_name || lhs_args.len() != rhs_args.len() {
                return Ok(false);
            }
            for (lhs, rhs) in lhs_args.iter().zip(rhs_args.iter()) {
                if !pointer_eq_inner(heap, lhs, rhs, seen)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        (Value::Uninitialized(lhs), Value::Uninitialized(rhs)) => Ok(lhs == rhs),
        (Value::Closure(lhs), Value::Closure(rhs)) => closure_eq_inner(heap, lhs, rhs, seen),
        (Value::Native(lhs), Value::Native(rhs)) => Ok(lhs == rhs),
        (Value::Overloaded(lhs), Value::Overloaded(rhs)) => Ok(lhs == rhs),
        _ => Ok(false),
    }
}

pub fn value_eq(heap: &Heap, lhs: &Value, rhs: &Value) -> Result<bool, EngineError> {
    let mut seen = HashSet::new();
    value_eq_inner(heap, lhs, rhs, &mut seen)
}

pub fn pointer_eq(heap: &Heap, lhs: &Pointer, rhs: &Pointer) -> Result<bool, EngineError> {
    let mut seen = HashSet::new();
    pointer_eq_inner(heap, lhs, rhs, &mut seen)
}

pub fn closure_eq(heap: &Heap, lhs: &Closure, rhs: &Closure) -> Result<bool, EngineError> {
    let mut seen = HashSet::new();
    closure_eq_inner(heap, lhs, rhs, &mut seen)
}

fn list_to_vec_opt(heap: &Heap, value: &Value) -> Result<Option<Vec<Pointer>>, EngineError> {
    enum Cursor<'a> {
        Borrowed(&'a Value),
        Owned(ValueRef),
    }

    let mut out = Vec::new();
    let mut cursor = Cursor::Borrowed(value);
    loop {
        let cur = match &cursor {
            Cursor::Borrowed(v) => *v,
            Cursor::Owned(v) => v.as_ref(),
        };

        match cur {
            Value::Adt(tag, args) if sym_eq(tag, "Empty") && args.is_empty() => {
                return Ok(Some(out));
            }
            Value::Adt(tag, args) if sym_eq(tag, "Cons") && args.len() == 2 => {
                out.push(args[0]);
                cursor = Cursor::Owned(heap.get(&args[1])?);
            }
            _ => return Ok(None),
        }
    }
}

pub(crate) fn list_to_vec(heap: &Heap, value: &Value) -> Result<Vec<Pointer>, EngineError> {
    enum Cursor<'a> {
        Borrowed(&'a Value),
        Owned(ValueRef),
    }

    let mut out = Vec::new();
    let mut cursor = Cursor::Borrowed(value);
    loop {
        let cur = match &cursor {
            Cursor::Borrowed(v) => *v,
            Cursor::Owned(v) => v.as_ref(),
        };

        match cur {
            Value::Adt(tag, args) if sym_eq(tag, "Empty") && args.is_empty() => return Ok(out),
            Value::Adt(tag, args) if sym_eq(tag, "Cons") && args.len() == 2 => {
                out.push(args[0]);
                cursor = Cursor::Owned(heap.get(&args[1])?);
            }
            _ => {
                return Err(EngineError::NativeType {
                    expected: "list".into(),
                    got: heap.type_name_of_value(cur).into(),
                });
            }
        }
    }
}

pub trait IntoPointer {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError>;
}

pub trait FromPointer: Sized {
    fn from_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError>;
}

pub trait RexType {
    fn rex_type() -> Type;
}

pub trait RexConstructArg: Sized {
    fn rex_construct_type() -> Type;
    fn from_construct_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError>;
}

impl IntoPointer for Value {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_value(self)
    }
}

impl IntoPointer for &Value {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_value(self.clone())
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
        heap.alloc_bool(self)
    }
}

impl IntoPointer for u8 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_u8(self)
    }
}

impl IntoPointer for u16 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_u16(self)
    }
}

impl IntoPointer for u32 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_u32(self)
    }
}

impl IntoPointer for u64 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_u64(self)
    }
}

impl IntoPointer for i8 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_i8(self)
    }
}

impl IntoPointer for i16 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_i16(self)
    }
}

impl IntoPointer for i32 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_i32(self)
    }
}

impl IntoPointer for i64 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_i64(self)
    }
}

impl IntoPointer for f32 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_f32(self)
    }
}

impl IntoPointer for f64 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_f64(self)
    }
}

impl IntoPointer for String {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_string(self)
    }
}

impl IntoPointer for &str {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_string(self.to_string())
    }
}

impl<T: IntoPointer> IntoPointer for Vec<T> {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        let ptrs = self
            .into_iter()
            .map(|v| v.into_pointer(heap))
            .collect::<Result<Vec<_>, _>>()?;
        heap.alloc_array(ptrs)
    }
}

impl<T: IntoPointer> IntoPointer for Option<T> {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        match self {
            Some(v) => {
                let ptr = v.into_pointer(heap)?;
                heap.alloc_adt(sym("Some"), vec![ptr])
            }
            None => heap.alloc_adt(sym("None"), vec![]),
        }
    }
}

impl IntoPointer for Uuid {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_uuid(self)
    }
}

impl IntoPointer for DateTime<Utc> {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_datetime(self)
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

macro_rules! impl_rex_construct_arg_from_pointer {
    ($t:ty) => {
        impl RexConstructArg for $t {
            fn rex_construct_type() -> Type {
                <$t as RexType>::rex_type()
            }

            fn from_construct_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError> {
                <$t as FromPointer>::from_pointer(heap, pointer)
            }
        }
    };
}

impl_rex_construct_arg_from_pointer!(bool);
impl_rex_construct_arg_from_pointer!(u8);
impl_rex_construct_arg_from_pointer!(u16);
impl_rex_construct_arg_from_pointer!(u32);
impl_rex_construct_arg_from_pointer!(u64);
impl_rex_construct_arg_from_pointer!(i8);
impl_rex_construct_arg_from_pointer!(i16);
impl_rex_construct_arg_from_pointer!(i32);
impl_rex_construct_arg_from_pointer!(i64);
impl_rex_construct_arg_from_pointer!(f32);
impl_rex_construct_arg_from_pointer!(f64);
impl_rex_construct_arg_from_pointer!(String);
impl_rex_construct_arg_from_pointer!(Uuid);
impl_rex_construct_arg_from_pointer!(DateTime<Utc>);

impl<T> RexConstructArg for Option<T>
where
    T: RexConstructArg,
{
    fn rex_construct_type() -> Type {
        Type::option(T::rex_construct_type())
    }

    fn from_construct_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError> {
        let (tag, args) = heap.pointer_as_adt(pointer)?;
        if sym_eq(&tag, "Some") && args.len() == 1 {
            return Ok(Some(T::from_construct_pointer(heap, &args[0])?));
        }
        if sym_eq(&tag, "None") && args.is_empty() {
            return Ok(None);
        }
        Err(EngineError::NativeType {
            expected: "option".into(),
            got: heap.type_name(pointer)?.into(),
        })
    }
}

impl<T> RexConstructArg for Vec<T>
where
    T: RexConstructArg,
{
    fn rex_construct_type() -> Type {
        Type::list(T::rex_construct_type())
    }

    fn from_construct_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError> {
        let mut out = Vec::new();
        let mut cursor = *pointer;
        loop {
            let (tag, args) = heap.pointer_as_adt(&cursor)?;
            if sym_eq(&tag, "Empty") && args.is_empty() {
                return Ok(out);
            }
            if sym_eq(&tag, "Cons") && args.len() == 2 {
                out.push(T::from_construct_pointer(heap, &args[0])?);
                cursor = args[1];
                continue;
            }
            return Err(EngineError::NativeType {
                expected: "list".into(),
                got: heap.type_name(&cursor)?.into(),
            });
        }
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

impl FromPointer for bool {
    fn from_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError> {
        heap.pointer_as_bool(pointer)
    }
}

macro_rules! impl_from_pointer_num {
    ($t:ty, $pointer_as:ident) => {
        impl FromPointer for $t {
            fn from_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError> {
                heap.$pointer_as(pointer).map(|v| v as $t)
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
        heap.pointer_as_string(pointer)
    }
}

impl FromPointer for Uuid {
    fn from_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError> {
        heap.pointer_as_uuid(pointer)
    }
}

impl FromPointer for DateTime<Utc> {
    fn from_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError> {
        heap.pointer_as_datetime(pointer)
    }
}

impl FromPointer for Value {
    fn from_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError> {
        heap.get(pointer).map(|value| value.as_ref().clone())
    }
}

impl<T> FromPointer for Vec<T>
where
    T: FromPointer,
{
    fn from_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError> {
        let xs = heap.pointer_as_array(pointer)?;
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
        let (tag, args) = heap.pointer_as_adt(pointer)?;
        if sym_eq(&tag, "Some") && args.len() == 1 {
            return Ok(Some(T::from_pointer(heap, &args[0])?));
        }
        if sym_eq(&tag, "None") && args.is_empty() {
            return Ok(None);
        }
        Err(EngineError::NativeType {
            expected: "vec".into(),
            got: heap.type_name(pointer)?.into(),
        })
    }
}

impl<T: IntoPointer, E: IntoPointer> IntoPointer for Result<T, E> {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        match self {
            Ok(v) => {
                let ptr = v.into_pointer(heap)?;
                heap.alloc_adt(sym("Ok"), vec![ptr])
            }
            Err(e) => {
                let ptr = e.into_pointer(heap)?;
                heap.alloc_adt(sym("Err"), vec![ptr])
            }
        }
    }
}

impl<T: RexType, E: RexType> RexType for Result<T, E> {
    fn rex_type() -> Type {
        Type::app(
            Type::app(Type::con("Result", 2), E::rex_type()),
            T::rex_type(),
        )
    }
}

impl<T, E> FromPointer for Result<T, E>
where
    T: FromPointer,
    E: FromPointer,
{
    fn from_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError> {
        let (tag, args) = heap.pointer_as_adt(pointer)?;
        if sym_eq(&tag, "Ok") && args.len() == 1 {
            return Ok(Ok(T::from_pointer(heap, &args[0])?));
        }
        if sym_eq(&tag, "Err") && args.len() == 1 {
            return Ok(Err(E::from_pointer(heap, &args[0])?));
        }
        Err(EngineError::NativeType {
            expected: "result".into(),
            got: heap.type_name(pointer)?.into(),
        })
    }
}

impl RexType for () {
    fn rex_type() -> Type {
        Type::tuple(vec![])
    }
}

impl IntoPointer for () {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_tuple(vec![])
    }
}

impl FromPointer for () {
    fn from_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError> {
        let items = heap.pointer_as_tuple(pointer)?;
        if items.is_empty() {
            Ok(())
        } else {
            Err(EngineError::NativeType {
                expected: "tuple".into(),
                got: heap.type_name(pointer)?.into(),
            })
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

        impl<$($name: IntoPointer),+> IntoPointer for ($($name,)+) {
            #[allow(non_snake_case)]
            fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
                let ($($name,)+) = self;
                let ptrs = vec![$($name.into_pointer(heap)?),+];
                heap.alloc_tuple(ptrs)
            }
        }

        impl<$($name: FromPointer),+> FromPointer for ($($name,)+) {
            #[allow(non_snake_case)]
            fn from_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError> {
                let items = heap.pointer_as_tuple(pointer)?;
                match items.as_slice() {
                    [$($name),+] => {
                        Ok(($(<$name as FromPointer>::from_pointer(heap, $name)?),+,))
                    }
                    _ => Err(EngineError::NativeType {
                        expected: "tuple".into(),
                        got: heap.type_name(pointer)?.into(),
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

impl RexType for serde_json::Value {
    fn rex_type() -> Type {
        Type::con("serde_json::Value", 0)
    }
}

impl IntoPointer for serde_json::Value {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        let json_string = serde_json::to_string(&self)
            .map_err(|e| EngineError::Internal(format!("failed to serialize JSON: {}", e)))?;
        let string_ptr = heap.alloc_string(json_string)?;
        heap.alloc_adt(sym("serde_json::Value"), vec![string_ptr])
    }
}

impl FromPointer for serde_json::Value {
    fn from_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError> {
        let (tag, args) = heap.pointer_as_adt(pointer)?;
        if !sym_eq(&tag, "serde_json::Value") {
            return Err(EngineError::NativeType {
                expected: "serde_json::Value".into(),
                got: heap.type_name(pointer)?.into(),
            });
        }
        if args.len() != 1 {
            return Err(EngineError::Internal(format!(
                "serde_json::Value ADT should have 1 field, got {}",
                args.len()
            )));
        }
        let json_string = heap.pointer_as_string(&args[0])?;
        serde_json::from_str(&json_string)
            .map_err(|e| EngineError::Internal(format!("failed to deserialize JSON: {}", e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heap_rejects_pointer_from_different_heap() {
        let heap_a = Heap::new();
        let heap_b = Heap::new();
        let pointer = heap_a.alloc_i32(42).expect("alloc_i32 should succeed");

        let err = match heap_b.get(&pointer) {
            Ok(_) => panic!("cross-heap pointer use should fail"),
            Err(err) => err,
        };
        let EngineError::Internal(msg) = err else {
            panic!("expected internal error for cross-heap pointer");
        };
        assert!(msg.contains("different heap"), "unexpected error: {msg}");
    }

    #[test]
    fn scoped_heap_allocates_and_reads() {
        Heap::scoped(|heap| {
            let pointer = heap.alloc_i32(7).expect("alloc_i32 should succeed");
            let value = heap.get(&pointer).expect("pointer should resolve");
            assert!(matches!(value.as_ref(), Value::I32(7)));
        });
    }

    #[test]
    fn value_as_reports_mismatch_with_value_type_error() {
        let value = Value::Bool(true);
        let err = value
            .value_as_i32()
            .expect_err("bool should not coerce to i32");
        match err {
            EngineError::NativeType { expected, got } => {
                assert_eq!(expected, "i32");
                assert_eq!(got, "bool");
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn pointer_as_reports_mismatch_with_value_type_error() {
        let heap = Heap::new();
        let pointer = heap.alloc_bool(true).expect("alloc_bool should succeed");
        let err = heap
            .pointer_as_i32(&pointer)
            .expect_err("bool pointer should not coerce to i32");
        match err {
            EngineError::NativeType { expected, got } => {
                assert_eq!(expected, "i32");
                assert_eq!(got, "bool");
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn pointer_as_returns_payload_on_match() {
        let heap = Heap::new();
        let pointer = heap.alloc_i32(42).expect("alloc_i32 should succeed");
        let value = heap
            .pointer_as_i32(&pointer)
            .expect("i32 pointer should decode");
        assert_eq!(value, 42);
    }

    #[test]
    fn value_display_default_keeps_suffixes_and_names() {
        let heap = Heap::new();
        let num = heap.alloc_i32(2).expect("alloc i32");
        let got_num = pointer_display(&heap, &num).expect("display i32");
        assert_eq!(got_num, "2");

        let ctor = heap
            .alloc_adt(sym("@snippetabc.A"), vec![])
            .expect("alloc adt");
        let got_ctor = pointer_display(&heap, &ctor).expect("display adt");
        assert_eq!(got_ctor, "A");
    }

    #[test]
    fn value_display_unsanitized_keeps_suffixes_and_names() {
        let heap = Heap::new();
        let opts = ValueDisplayOptions::unsanitized();
        let num = heap.alloc_i32(2).expect("alloc i32");
        let got_num = pointer_display_with(&heap, &num, opts).expect("display i32");
        assert_eq!(got_num, "2i32");

        let ctor = heap
            .alloc_adt(sym("@snippetabc.A"), vec![])
            .expect("alloc adt");
        let got_ctor = pointer_display_with(&heap, &ctor, opts).expect("display adt");
        assert_eq!(got_ctor, "@snippetabc.A");
    }

    #[test]
    fn value_display_docs_mode_strips_internal_noise() {
        let heap = Heap::new();
        let opts = ValueDisplayOptions::docs();
        let num = heap.alloc_i32(2).expect("alloc i32");
        let got_num = pointer_display_with(&heap, &num, opts).expect("display i32 docs");
        assert_eq!(got_num, "2");

        let ctor = heap
            .alloc_adt(sym("@snippetabc.A"), vec![])
            .expect("alloc adt");
        let got_ctor = pointer_display_with(&heap, &ctor, opts).expect("display adt docs");
        assert_eq!(got_ctor, "A");

        let non_snippet = heap.alloc_adt(sym("pkg.A"), vec![]).expect("alloc adt");
        let got_non_snippet =
            pointer_display_with(&heap, &non_snippet, opts).expect("display non-snippet adt docs");
        assert_eq!(got_non_snippet, "pkg.A");
    }
}
