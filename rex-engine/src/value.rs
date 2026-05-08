//! Core value representation for Rex.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rex_ast::Symbol;
use rex_typesystem::types::{Type, TypedExpr};
use uuid::Uuid;

use crate::EngineError;
use crate::Environment;
use crate::engine::{NativeFn, OverloadedFn};
use crate::stack::Frame;

// GC invariants:
//
// - Any alloc_* call may run collection before it creates the requested object.
//   Callers must not keep raw Pointer values across allocation unless those
//   pointers are reachable from a Handle, TempRoots, a stack frame, or another
//   traced runtime structure.
// - Handle and TempRoots are heap-managed roots. They are the safe way for
//   public API and ordinary native/prelude code to keep values alive while
//   allocating.
// - Scheduler-native code may still store Pointer values directly, but only in
//   frame/task state that implements trace_pointers and rewrite_pointers.
// - The collector is copying: every traced Pointer must be rewritten after a
//   collection. A raw Pointer from before collection is stale by design.
struct HeapState {
    slots: Vec<HeapSlot>,
    root_slots: Vec<RootSlot>,
    free_root_list: Vec<u64>,
    next_gc_slot_count: usize,
    collect_on_every_alloc: bool,
    collections: u64,
}

const DEFAULT_GC_SLOT_THRESHOLD: usize = 4_096;
const GC_SLOT_GROWTH_NUMERATOR: usize = 3;
const GC_SLOT_GROWTH_DENOMINATOR: usize = 2;

impl Default for HeapState {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            root_slots: Vec::new(),
            free_root_list: Vec::new(),
            next_gc_slot_count: DEFAULT_GC_SLOT_THRESHOLD,
            collect_on_every_alloc: false,
            collections: 0,
        }
    }
}

#[derive(Clone)]
struct HeapSlot {
    generation: u64,
    cell: Option<Cell>,
}

#[derive(Clone)]
struct RootSlot {
    generation: u64,
    pointer: Option<Pointer>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct RootId {
    heap_id: u64,
    index: u64,
    generation: u64,
}

/// Rex heap and allocation API.
///
/// Public allocation methods return [`Handle`] values. A handle is a GC root, so
/// host code may keep it across later allocations and `await` points. Every
/// `alloc_*` call may run a copying collection before returning, which means
/// raw internal pointers must not be kept across allocation unless they are
/// protected by a handle, temporary root, or traced runtime frame.
#[derive(Clone)]
pub struct Heap {
    id: u64,
    state: Arc<Mutex<HeapState>>,
}

pub(crate) struct HeapAccess<'a> {
    heap: Heap,
    state: &'a mut HeapState,
}

/// Internal temporary roots used while runtime code is constructing heap cells
/// from raw pointers.
///
/// This type is deliberately crate-private. Public API and host callbacks should
/// use [`Handle`] instead.
pub(crate) struct TempRoots {
    heap: Heap,
    root_ids: Vec<RootId>,
    collection_count: u64,
}

/// A rooted reference to a Rex heap value.
///
/// Cloning a handle clones the root, so the underlying value remains visible to
/// the collector until the last clone is dropped. This is the public way for
/// embedders and host functions to keep Rex values alive while allocating new
/// values or suspending in async code.
#[derive(Clone)]
pub struct Handle {
    root: Arc<HandleRoot>,
}

#[derive(Clone, Debug)]
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
    Tuple(Vec<Handle>),
    Array(Vec<Handle>),
    Dict(BTreeMap<Symbol, Handle>),
    Adt(Symbol, Vec<Handle>),
    Uninitialized(Symbol),
    Frame,
    Closure,
    Native,
    Overloaded,
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
            Value::Adt(name, ..) if name.as_ref() == "Empty" || name.as_ref() == "Cons" => "list",
            Value::Adt(..) => "adt",
            Value::Uninitialized(..) => "uninitialized",
            Value::Frame => "frame",
            Value::Closure => "closure",
            Value::Native => "native",
            Value::Overloaded => "overloaded",
        }
    }
}

struct HandleRoot {
    heap: Heap,
    root_id: RootId,
}

impl Drop for TempRoots {
    fn drop(&mut self) {
        for root_id in self.root_ids.drain(..) {
            let _ = self.heap.unregister_external_root(root_id);
        }
    }
}

impl TempRoots {
    pub(crate) fn len(&self) -> usize {
        self.root_ids.len()
    }

    pub(crate) fn has_collected_since_creation(&self) -> Result<bool, EngineError> {
        let state = self
            .heap
            .state
            .lock()
            .map_err(|_| EngineError::Internal("heap state poisoned".into()))?;
        Ok(state.collections != self.collection_count)
    }

    pub(crate) fn get(&self, index: usize) -> Result<Pointer, EngineError> {
        let root_id = *self
            .root_ids
            .get(index)
            .ok_or_else(|| EngineError::Internal("temporary root index out of bounds".into()))?;
        self.heap.resolve_external_root(root_id)
    }
}

impl Drop for HandleRoot {
    fn drop(&mut self) {
        let _ = self.heap.unregister_external_root(self.root_id);
    }
}

impl fmt::Debug for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.display() {
            Ok(value) => f.debug_tuple("Handle").field(&value).finish(),
            Err(_) => f.write_str("Handle(<invalid>)"),
        }
    }
}

impl Handle {
    pub fn type_name(&self) -> Result<&'static str, EngineError> {
        let pointer = self.pointer()?;
        self.heap().type_name(&pointer)
    }

    pub fn value(&self) -> Result<Value, EngineError> {
        let pointer = self.pointer()?;
        self.heap().view(&pointer)
    }

    pub fn as_bool(&self) -> Result<bool, EngineError> {
        match self.value()? {
            Value::Bool(value) => Ok(value),
            _ => Err(self.type_error("bool")),
        }
    }

    pub fn as_u8(&self) -> Result<u8, EngineError> {
        match self.value()? {
            Value::U8(value) => Ok(value),
            _ => Err(self.type_error("u8")),
        }
    }

    pub fn as_u16(&self) -> Result<u16, EngineError> {
        match self.value()? {
            Value::U16(value) => Ok(value),
            _ => Err(self.type_error("u16")),
        }
    }

    pub fn as_u32(&self) -> Result<u32, EngineError> {
        match self.value()? {
            Value::U32(value) => Ok(value),
            _ => Err(self.type_error("u32")),
        }
    }

    pub fn as_u64(&self) -> Result<u64, EngineError> {
        match self.value()? {
            Value::U64(value) => Ok(value),
            _ => Err(self.type_error("u64")),
        }
    }

    pub fn as_i8(&self) -> Result<i8, EngineError> {
        match self.value()? {
            Value::I8(value) => Ok(value),
            _ => Err(self.type_error("i8")),
        }
    }

    pub fn as_i16(&self) -> Result<i16, EngineError> {
        match self.value()? {
            Value::I16(value) => Ok(value),
            _ => Err(self.type_error("i16")),
        }
    }

    pub fn as_i32(&self) -> Result<i32, EngineError> {
        match self.value()? {
            Value::I32(value) => Ok(value),
            _ => Err(self.type_error("i32")),
        }
    }

    pub fn as_i64(&self) -> Result<i64, EngineError> {
        match self.value()? {
            Value::I64(value) => Ok(value),
            _ => Err(self.type_error("i64")),
        }
    }

    pub fn as_f32(&self) -> Result<f32, EngineError> {
        match self.value()? {
            Value::F32(value) => Ok(value),
            _ => Err(self.type_error("f32")),
        }
    }

    pub fn as_f64(&self) -> Result<f64, EngineError> {
        match self.value()? {
            Value::F64(value) => Ok(value),
            _ => Err(self.type_error("f64")),
        }
    }

    pub fn as_string(&self) -> Result<String, EngineError> {
        match self.value()? {
            Value::String(value) => Ok(value),
            _ => Err(self.type_error("string")),
        }
    }

    pub fn as_uuid(&self) -> Result<Uuid, EngineError> {
        match self.value()? {
            Value::Uuid(value) => Ok(value),
            _ => Err(self.type_error("uuid")),
        }
    }

    pub fn as_datetime(&self) -> Result<DateTime<Utc>, EngineError> {
        match self.value()? {
            Value::DateTime(value) => Ok(value),
            _ => Err(self.type_error("datetime")),
        }
    }

    pub fn as_tuple(&self) -> Result<Vec<Handle>, EngineError> {
        match self.value()? {
            Value::Tuple(values) => Ok(values),
            _ => Err(self.type_error("tuple")),
        }
    }

    pub fn as_array(&self) -> Result<Vec<Handle>, EngineError> {
        match self.value()? {
            Value::Array(values) => Ok(values),
            _ => Err(self.type_error("array")),
        }
    }

    pub fn as_list(&self) -> Result<Vec<Handle>, EngineError> {
        let pointer = self.pointer()?;
        self.heap()
            .pointer_as_list(&pointer)?
            .into_iter()
            .map(|pointer| self.heap().handle(pointer))
            .collect()
    }

    pub fn as_dict(&self) -> Result<BTreeMap<Symbol, Handle>, EngineError> {
        match self.value()? {
            Value::Dict(values) => Ok(values),
            _ => Err(self.type_error("dict")),
        }
    }

    pub fn as_adt(&self) -> Result<(Symbol, Vec<Handle>), EngineError> {
        match self.value()? {
            Value::Adt(tag, args) => Ok((tag, args)),
            _ => Err(self.type_error("adt")),
        }
    }

    pub fn to_rust<T: FromRex>(&self) -> Result<T, EngineError> {
        T::from_rex(self)
    }

    pub fn display(&self) -> Result<String, EngineError> {
        self.display_with(ValueDisplayOptions::default())
    }

    pub fn display_with(&self, opts: ValueDisplayOptions) -> Result<String, EngineError> {
        let pointer = self.pointer()?;
        pointer_display_with(self.heap(), &pointer, opts)
    }

    pub fn debug(&self) -> Result<String, EngineError> {
        let pointer = self.pointer()?;
        pointer_debug(self.heap(), &pointer)
    }

    pub fn value_eq(&self, other: &Handle) -> Result<bool, EngineError> {
        let self_pointer = self.pointer()?;
        let pointer = other.pointer_for_heap(self.heap())?;
        pointer_eq(self.heap(), &self_pointer, &pointer)
    }

    fn type_error(&self, expected: &'static str) -> EngineError {
        EngineError::NativeType {
            expected: expected.to_string(),
            got: self.type_name().unwrap_or("<invalid handle>").to_string(),
        }
    }

    pub fn heap(&self) -> &Heap {
        &self.root.heap
    }

    pub(crate) fn pointer(&self) -> Result<Pointer, EngineError> {
        self.root.heap.resolve_external_root(self.root.root_id)
    }

    pub(crate) fn pointer_for_heap(&self, heap: &Heap) -> Result<Pointer, EngineError> {
        let pointer = self.pointer()?;
        if pointer.heap_id != heap.id {
            return Err(Heap::wrong_heap_pointer(
                pointer.heap_id,
                heap.id,
                pointer.index,
                pointer.generation,
            ));
        }
        Ok(pointer)
    }
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

enum ValueSeed {
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
    Frame,
    Closure,
    Native,
    Overloaded,
}

impl ValueSeed {
    fn from_cell(cell: &Cell) -> Self {
        match cell {
            Cell::Bool(value) => Self::Bool(*value),
            Cell::U8(value) => Self::U8(*value),
            Cell::U16(value) => Self::U16(*value),
            Cell::U32(value) => Self::U32(*value),
            Cell::U64(value) => Self::U64(*value),
            Cell::I8(value) => Self::I8(*value),
            Cell::I16(value) => Self::I16(*value),
            Cell::I32(value) => Self::I32(*value),
            Cell::I64(value) => Self::I64(*value),
            Cell::F32(value) => Self::F32(*value),
            Cell::F64(value) => Self::F64(*value),
            Cell::String(value) => Self::String(value.clone()),
            Cell::Uuid(value) => Self::Uuid(*value),
            Cell::DateTime(value) => Self::DateTime(*value),
            Cell::Tuple(values) => Self::Tuple(values.clone()),
            Cell::Array(values) => Self::Array(values.clone()),
            Cell::Dict(values) => Self::Dict(values.clone()),
            Cell::Adt(name, args) => Self::Adt(name.clone(), args.clone()),
            Cell::Uninitialized(name) => Self::Uninitialized(name.clone()),
            Cell::Frame(_) => Self::Frame,
            Cell::Closure(_) => Self::Closure,
            Cell::Native(_) => Self::Native,
            Cell::Overloaded(_) => Self::Overloaded,
        }
    }
}

impl HeapAccess<'_> {
    pub(crate) fn get(&self, pointer: &Pointer) -> Result<&Cell, EngineError> {
        if pointer.heap_id != self.heap.id {
            return Err(Heap::wrong_heap_pointer(
                pointer.heap_id,
                self.heap.id,
                pointer.index,
                pointer.generation,
            ));
        }
        let slot = self
            .state
            .slots
            .get(pointer.index as usize)
            .ok_or_else(|| {
                Heap::invalid_pointer(self.heap.id, pointer.index, pointer.generation)
            })?;
        if slot.generation != pointer.generation {
            return Err(Heap::invalid_pointer(
                self.heap.id,
                pointer.index,
                pointer.generation,
            ));
        }
        slot.cell
            .as_ref()
            .ok_or_else(|| Heap::invalid_pointer(self.heap.id, pointer.index, pointer.generation))
    }

    pub(crate) fn type_name(&self, pointer: &Pointer) -> Result<&'static str, EngineError> {
        self.get(pointer).map(Cell::cell_type_name)
    }

    pub(crate) fn overwrite(&mut self, pointer: &Pointer, cell: Cell) -> Result<(), EngineError> {
        if pointer.heap_id != self.heap.id {
            return Err(Heap::wrong_heap_pointer(
                pointer.heap_id,
                self.heap.id,
                pointer.index,
                pointer.generation,
            ));
        }

        let slot = self
            .state
            .slots
            .get_mut(pointer.index as usize)
            .ok_or_else(|| {
                Heap::invalid_pointer(self.heap.id, pointer.index, pointer.generation)
            })?;
        if slot.generation != pointer.generation {
            return Err(Heap::invalid_pointer(
                self.heap.id,
                pointer.index,
                pointer.generation,
            ));
        }
        slot.cell = Some(cell);
        Ok(())
    }

    fn register_external_root(&mut self, pointer: Pointer) -> Result<RootId, EngineError> {
        Heap::register_root_locked(self.heap.id, self.state, pointer)
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
            state: Arc::new(Mutex::new(HeapState::default())),
        }
    }

    pub(crate) fn with_access<R>(
        &self,
        f: impl FnOnce(&mut HeapAccess<'_>) -> Result<R, EngineError>,
    ) -> Result<R, EngineError> {
        // Keep all heap reads inside this access object while the lock is held.
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::Internal("heap state poisoned".into()))?;
        let mut access = HeapAccess {
            heap: self.clone(),
            state: &mut state,
        };
        f(&mut access)
    }

    fn invalid_pointer(heap_id: u64, index: u32, generation: u64) -> EngineError {
        EngineError::Internal(format!(
            "invalid heap pointer (heap_id={}, index={}, generation={})",
            heap_id, index, generation
        ))
    }

    fn wrong_heap_pointer(
        pointer_heap_id: u64,
        heap_id: u64,
        index: u32,
        generation: u64,
    ) -> EngineError {
        EngineError::Internal(format!(
            "heap pointer belongs to different heap (pointer_heap_id={}, heap_id={}, index={}, generation={})",
            pointer_heap_id, heap_id, index, generation
        ))
    }

    fn invalid_root(root_id: RootId) -> EngineError {
        EngineError::Internal(format!(
            "invalid heap root (heap_id={}, index={}, generation={})",
            root_id.heap_id, root_id.index, root_id.generation
        ))
    }

    fn get_slot_checked<'a>(
        heap_id: u64,
        state: &'a HeapState,
        pointer: &Pointer,
    ) -> Result<&'a HeapSlot, EngineError> {
        if pointer.heap_id != heap_id {
            return Err(Self::wrong_heap_pointer(
                pointer.heap_id,
                heap_id,
                pointer.index,
                pointer.generation,
            ));
        }
        let slot = state
            .slots
            .get(pointer.index as usize)
            .ok_or_else(|| Self::invalid_pointer(heap_id, pointer.index, pointer.generation))?;
        if slot.generation != pointer.generation || slot.cell.is_none() {
            return Err(Self::invalid_pointer(
                heap_id,
                pointer.index,
                pointer.generation,
            ));
        }
        Ok(slot)
    }

    fn register_root_locked(
        heap_id: u64,
        state: &mut HeapState,
        pointer: Pointer,
    ) -> Result<RootId, EngineError> {
        Self::get_slot_checked(heap_id, state, &pointer)?;

        if let Some(index) = state.free_root_list.pop() {
            let slot_index = usize::try_from(index)
                .map_err(|_| EngineError::Internal("heap root index overflow".into()))?;
            let slot = state
                .root_slots
                .get_mut(slot_index)
                .ok_or_else(|| EngineError::Internal("heap root free-list corruption".into()))?;
            if slot.pointer.is_some() {
                return Err(EngineError::Internal(
                    "heap root free-list referenced a live root".into(),
                ));
            }
            slot.pointer = Some(pointer);
            return Ok(RootId {
                heap_id,
                index,
                generation: slot.generation,
            });
        }

        let index = u64::try_from(state.root_slots.len())
            .map_err(|_| EngineError::Internal("heap exhausted: too many root slots".into()))?;
        state.root_slots.push(RootSlot {
            generation: 0,
            pointer: Some(pointer),
        });
        Ok(RootId {
            heap_id,
            index,
            generation: 0,
        })
    }

    fn unregister_root_locked(
        heap_id: u64,
        state: &mut HeapState,
        root_id: RootId,
    ) -> Result<(), EngineError> {
        if root_id.heap_id != heap_id {
            return Err(Self::invalid_root(root_id));
        }
        let slot_index = usize::try_from(root_id.index)
            .map_err(|_| EngineError::Internal("heap root index overflow".into()))?;
        let slot = state
            .root_slots
            .get_mut(slot_index)
            .ok_or_else(|| Self::invalid_root(root_id))?;
        if slot.generation != root_id.generation || slot.pointer.is_none() {
            return Err(Self::invalid_root(root_id));
        }
        let next_generation = slot
            .generation
            .checked_add(1)
            .ok_or_else(|| EngineError::Internal("heap root generation exhausted".into()))?;
        slot.pointer = None;
        slot.generation = next_generation;
        state.free_root_list.push(root_id.index);
        Ok(())
    }

    fn resolve_root_locked(
        heap_id: u64,
        state: &HeapState,
        root_id: RootId,
    ) -> Result<Pointer, EngineError> {
        if root_id.heap_id != heap_id {
            return Err(Self::invalid_root(root_id));
        }
        let slot_index = usize::try_from(root_id.index)
            .map_err(|_| EngineError::Internal("heap root index overflow".into()))?;
        let slot = state
            .root_slots
            .get(slot_index)
            .ok_or_else(|| Self::invalid_root(root_id))?;
        if slot.generation != root_id.generation {
            return Err(Self::invalid_root(root_id));
        }
        slot.pointer.ok_or_else(|| Self::invalid_root(root_id))
    }

    pub(crate) fn temp_roots(&self, pointers: Vec<Pointer>) -> Result<TempRoots, EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::Internal("heap state poisoned".into()))?;
        let mut root_ids = Vec::with_capacity(pointers.len());
        for pointer in pointers {
            root_ids.push(Self::register_root_locked(self.id, &mut state, pointer)?);
        }
        let collection_count = state.collections;
        Ok(TempRoots {
            heap: self.clone(),
            root_ids,
            collection_count,
        })
    }

    pub(crate) fn handle(&self, pointer: Pointer) -> Result<Handle, EngineError> {
        let root_id = self.with_access(|heap| {
            heap.get(&pointer)?;
            heap.register_external_root(pointer)
        })?;
        Ok(Handle {
            root: Arc::new(HandleRoot {
                heap: self.clone(),
                root_id,
            }),
        })
    }

    pub fn set_collect_on_every_alloc(&self, enabled: bool) -> Result<(), EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::Internal("heap state poisoned".into()))?;
        state.collect_on_every_alloc = enabled;
        Ok(())
    }

    pub fn alloc_bool(&self, value: bool) -> Result<Handle, EngineError> {
        handle_from_pointer(self, self.alloc_ptr_bool(value)?)
    }

    pub fn alloc_u8(&self, value: u8) -> Result<Handle, EngineError> {
        handle_from_pointer(self, self.alloc_ptr_u8(value)?)
    }

    pub fn alloc_u16(&self, value: u16) -> Result<Handle, EngineError> {
        handle_from_pointer(self, self.alloc_ptr_u16(value)?)
    }

    pub fn alloc_u32(&self, value: u32) -> Result<Handle, EngineError> {
        handle_from_pointer(self, self.alloc_ptr_u32(value)?)
    }

    pub fn alloc_u64(&self, value: u64) -> Result<Handle, EngineError> {
        handle_from_pointer(self, self.alloc_ptr_u64(value)?)
    }

    pub fn alloc_i8(&self, value: i8) -> Result<Handle, EngineError> {
        handle_from_pointer(self, self.alloc_ptr_i8(value)?)
    }

    pub fn alloc_i16(&self, value: i16) -> Result<Handle, EngineError> {
        handle_from_pointer(self, self.alloc_ptr_i16(value)?)
    }

    pub fn alloc_i32(&self, value: i32) -> Result<Handle, EngineError> {
        handle_from_pointer(self, self.alloc_ptr_i32(value)?)
    }

    pub fn alloc_i64(&self, value: i64) -> Result<Handle, EngineError> {
        handle_from_pointer(self, self.alloc_ptr_i64(value)?)
    }

    pub fn alloc_f32(&self, value: f32) -> Result<Handle, EngineError> {
        handle_from_pointer(self, self.alloc_ptr_f32(value)?)
    }

    pub fn alloc_f64(&self, value: f64) -> Result<Handle, EngineError> {
        handle_from_pointer(self, self.alloc_ptr_f64(value)?)
    }

    pub fn alloc_string(&self, value: String) -> Result<Handle, EngineError> {
        handle_from_pointer(self, self.alloc_ptr_string(value)?)
    }

    pub fn alloc_uuid(&self, value: Uuid) -> Result<Handle, EngineError> {
        handle_from_pointer(self, self.alloc_ptr_uuid(value)?)
    }

    pub fn alloc_datetime(&self, value: DateTime<Utc>) -> Result<Handle, EngineError> {
        handle_from_pointer(self, self.alloc_ptr_datetime(value)?)
    }

    pub fn alloc_tuple(&self, values: Vec<Handle>) -> Result<Handle, EngineError> {
        let pointers = self.pointers_from_handles(values)?;
        handle_from_pointer(self, self.alloc_ptr_tuple(pointers)?)
    }

    pub fn alloc_array(&self, values: Vec<Handle>) -> Result<Handle, EngineError> {
        let pointers = self.pointers_from_handles(values)?;
        handle_from_pointer(self, self.alloc_ptr_array(pointers)?)
    }

    pub fn alloc_list(&self, values: Vec<Handle>) -> Result<Handle, EngineError> {
        let pointers = self.pointers_from_handles(values)?;
        handle_from_pointer(self, self.alloc_ptr_list(pointers)?)
    }

    pub fn alloc_dict(&self, values: BTreeMap<Symbol, Handle>) -> Result<Handle, EngineError> {
        let mut pointers = BTreeMap::new();
        for (name, handle) in values {
            pointers.insert(name, handle.pointer_for_heap(self)?);
        }
        handle_from_pointer(self, self.alloc_ptr_dict(pointers)?)
    }

    pub fn alloc_adt(&self, name: Symbol, args: Vec<Handle>) -> Result<Handle, EngineError> {
        let pointers = self.pointers_from_handles(args)?;
        handle_from_pointer(self, self.alloc_ptr_adt(name, pointers)?)
    }

    pub fn alloc_value(&self, value: Value) -> Result<Handle, EngineError> {
        match value {
            Value::Bool(value) => self.alloc_bool(value),
            Value::U8(value) => self.alloc_u8(value),
            Value::U16(value) => self.alloc_u16(value),
            Value::U32(value) => self.alloc_u32(value),
            Value::U64(value) => self.alloc_u64(value),
            Value::I8(value) => self.alloc_i8(value),
            Value::I16(value) => self.alloc_i16(value),
            Value::I32(value) => self.alloc_i32(value),
            Value::I64(value) => self.alloc_i64(value),
            Value::F32(value) => self.alloc_f32(value),
            Value::F64(value) => self.alloc_f64(value),
            Value::String(value) => self.alloc_string(value),
            Value::Uuid(value) => self.alloc_uuid(value),
            Value::DateTime(value) => self.alloc_datetime(value),
            Value::Tuple(values) => self.alloc_tuple(values),
            Value::Array(values) => self.alloc_array(values),
            Value::Dict(values) => self.alloc_dict(values),
            Value::Adt(name, args) => self.alloc_adt(name, args),
            Value::Uninitialized(_)
            | Value::Frame
            | Value::Closure
            | Value::Native
            | Value::Overloaded => Err(EngineError::Internal(
                "cannot allocate internal runtime value through public API".into(),
            )),
        }
    }

    fn pointers_from_handles(&self, values: Vec<Handle>) -> Result<Vec<Pointer>, EngineError> {
        values
            .iter()
            .map(|handle| handle.pointer_for_heap(self))
            .collect()
    }

    pub(crate) fn clone_cell(&self, pointer: &Pointer) -> Result<Cell, EngineError> {
        self.with_access(|heap| Ok(heap.get(pointer)?.clone()))
    }

    pub(crate) fn type_name(&self, pointer: &Pointer) -> Result<&'static str, EngineError> {
        self.with_access(|heap| heap.type_name(pointer))
    }

    pub(crate) fn view(&self, pointer: &Pointer) -> Result<Value, EngineError> {
        let seed = self.with_access(|heap| Ok(ValueSeed::from_cell(heap.get(pointer)?)))?;
        self.view_seed(seed)
    }

    fn view_seed(&self, seed: ValueSeed) -> Result<Value, EngineError> {
        Ok(match seed {
            ValueSeed::Bool(value) => Value::Bool(value),
            ValueSeed::U8(value) => Value::U8(value),
            ValueSeed::U16(value) => Value::U16(value),
            ValueSeed::U32(value) => Value::U32(value),
            ValueSeed::U64(value) => Value::U64(value),
            ValueSeed::I8(value) => Value::I8(value),
            ValueSeed::I16(value) => Value::I16(value),
            ValueSeed::I32(value) => Value::I32(value),
            ValueSeed::I64(value) => Value::I64(value),
            ValueSeed::F32(value) => Value::F32(value),
            ValueSeed::F64(value) => Value::F64(value),
            ValueSeed::String(value) => Value::String(value),
            ValueSeed::Uuid(value) => Value::Uuid(value),
            ValueSeed::DateTime(value) => Value::DateTime(value),
            ValueSeed::Tuple(values) => Value::Tuple(self.handles_from_pointers(&values)?),
            ValueSeed::Array(values) => Value::Array(self.handles_from_pointers(&values)?),
            ValueSeed::Dict(values) => {
                let mut out = BTreeMap::new();
                for (name, pointer) in values {
                    out.insert(name, self.handle(pointer)?);
                }
                Value::Dict(out)
            }
            ValueSeed::Adt(name, args) => Value::Adt(name, self.handles_from_pointers(&args)?),
            ValueSeed::Uninitialized(name) => Value::Uninitialized(name),
            ValueSeed::Frame => Value::Frame,
            ValueSeed::Closure => Value::Closure,
            ValueSeed::Native => Value::Native,
            ValueSeed::Overloaded => Value::Overloaded,
        })
    }

    fn handles_from_pointers(&self, values: &[Pointer]) -> Result<Vec<Handle>, EngineError> {
        values
            .iter()
            .map(|pointer| self.handle(*pointer))
            .collect::<Result<Vec<_>, _>>()
    }

    pub(crate) fn pointer_as_bool(&self, pointer: &Pointer) -> Result<bool, EngineError> {
        self.with_access(|heap| heap.get(pointer)?.cell_as_bool())
    }

    pub(crate) fn pointer_as_u8(&self, pointer: &Pointer) -> Result<u8, EngineError> {
        self.with_access(|heap| heap.get(pointer)?.cell_as_u8())
    }

    pub(crate) fn pointer_as_u16(&self, pointer: &Pointer) -> Result<u16, EngineError> {
        self.with_access(|heap| heap.get(pointer)?.cell_as_u16())
    }

    pub(crate) fn pointer_as_u32(&self, pointer: &Pointer) -> Result<u32, EngineError> {
        self.with_access(|heap| heap.get(pointer)?.cell_as_u32())
    }

    pub(crate) fn pointer_as_u64(&self, pointer: &Pointer) -> Result<u64, EngineError> {
        self.with_access(|heap| heap.get(pointer)?.cell_as_u64())
    }

    pub(crate) fn pointer_as_i8(&self, pointer: &Pointer) -> Result<i8, EngineError> {
        self.with_access(|heap| heap.get(pointer)?.cell_as_i8())
    }

    pub(crate) fn pointer_as_i16(&self, pointer: &Pointer) -> Result<i16, EngineError> {
        self.with_access(|heap| heap.get(pointer)?.cell_as_i16())
    }

    pub(crate) fn pointer_as_i32(&self, pointer: &Pointer) -> Result<i32, EngineError> {
        self.with_access(|heap| heap.get(pointer)?.cell_as_i32())
    }

    pub(crate) fn pointer_as_i64(&self, pointer: &Pointer) -> Result<i64, EngineError> {
        self.with_access(|heap| heap.get(pointer)?.cell_as_i64())
    }

    pub(crate) fn pointer_as_f32(&self, pointer: &Pointer) -> Result<f32, EngineError> {
        self.with_access(|heap| heap.get(pointer)?.cell_as_f32())
    }

    pub(crate) fn pointer_as_f64(&self, pointer: &Pointer) -> Result<f64, EngineError> {
        self.with_access(|heap| heap.get(pointer)?.cell_as_f64())
    }

    pub(crate) fn pointer_as_string(&self, pointer: &Pointer) -> Result<String, EngineError> {
        self.with_access(|heap| heap.get(pointer)?.cell_as_string())
    }

    pub(crate) fn pointer_as_uuid(&self, pointer: &Pointer) -> Result<Uuid, EngineError> {
        self.with_access(|heap| heap.get(pointer)?.cell_as_uuid())
    }

    pub(crate) fn pointer_as_datetime(
        &self,
        pointer: &Pointer,
    ) -> Result<DateTime<Utc>, EngineError> {
        self.with_access(|heap| heap.get(pointer)?.cell_as_datetime())
    }

    pub(crate) fn pointer_as_tuple(&self, pointer: &Pointer) -> Result<Vec<Pointer>, EngineError> {
        self.with_access(|heap| heap.get(pointer)?.cell_as_tuple())
    }

    pub(crate) fn pointer_as_array(&self, pointer: &Pointer) -> Result<Vec<Pointer>, EngineError> {
        self.with_access(|heap| heap.get(pointer)?.cell_as_array())
    }

    pub(crate) fn pointer_as_dict(
        &self,
        pointer: &Pointer,
    ) -> Result<BTreeMap<Symbol, Pointer>, EngineError> {
        self.with_access(|heap| heap.get(pointer)?.cell_as_dict())
    }

    pub(crate) fn pointer_as_adt(
        &self,
        pointer: &Pointer,
    ) -> Result<(Symbol, Vec<Pointer>), EngineError> {
        self.with_access(|heap| heap.get(pointer)?.cell_as_adt())
    }

    pub(crate) fn pointer_as_frame(&self, pointer: &Pointer) -> Result<Frame, EngineError> {
        self.with_access(|heap| heap.get(pointer)?.cell_as_frame())
    }

    pub(crate) fn pointer_as_list(&self, pointer: &Pointer) -> Result<Vec<Pointer>, EngineError> {
        self.with_access(|heap| {
            let cell = heap.get(pointer)?;
            list_to_vec(heap, cell)
        })
    }

    pub(crate) fn alloc_ptr_bool(&self, value: bool) -> Result<Pointer, EngineError> {
        self.alloc_cell(Cell::Bool(value))
    }

    pub(crate) fn alloc_ptr_u8(&self, value: u8) -> Result<Pointer, EngineError> {
        self.alloc_cell(Cell::U8(value))
    }

    pub(crate) fn alloc_ptr_u16(&self, value: u16) -> Result<Pointer, EngineError> {
        self.alloc_cell(Cell::U16(value))
    }

    pub(crate) fn alloc_ptr_u32(&self, value: u32) -> Result<Pointer, EngineError> {
        self.alloc_cell(Cell::U32(value))
    }

    pub(crate) fn alloc_ptr_u64(&self, value: u64) -> Result<Pointer, EngineError> {
        self.alloc_cell(Cell::U64(value))
    }

    pub(crate) fn alloc_ptr_i8(&self, value: i8) -> Result<Pointer, EngineError> {
        self.alloc_cell(Cell::I8(value))
    }

    pub(crate) fn alloc_ptr_i16(&self, value: i16) -> Result<Pointer, EngineError> {
        self.alloc_cell(Cell::I16(value))
    }

    pub(crate) fn alloc_ptr_i32(&self, value: i32) -> Result<Pointer, EngineError> {
        self.alloc_cell(Cell::I32(value))
    }

    pub(crate) fn alloc_ptr_i64(&self, value: i64) -> Result<Pointer, EngineError> {
        self.alloc_cell(Cell::I64(value))
    }

    pub(crate) fn alloc_ptr_f32(&self, value: f32) -> Result<Pointer, EngineError> {
        self.alloc_cell(Cell::F32(value))
    }

    pub(crate) fn alloc_ptr_f64(&self, value: f64) -> Result<Pointer, EngineError> {
        self.alloc_cell(Cell::F64(value))
    }

    pub(crate) fn alloc_ptr_string(&self, value: String) -> Result<Pointer, EngineError> {
        self.alloc_cell(Cell::String(value))
    }

    pub(crate) fn alloc_ptr_uuid(&self, value: Uuid) -> Result<Pointer, EngineError> {
        self.alloc_cell(Cell::Uuid(value))
    }

    pub(crate) fn alloc_ptr_datetime(&self, value: DateTime<Utc>) -> Result<Pointer, EngineError> {
        self.alloc_cell(Cell::DateTime(value))
    }

    pub(crate) fn alloc_ptr_cell(&self, cell: Cell) -> Result<Pointer, EngineError> {
        self.alloc_cell(cell)
    }

    pub(crate) fn alloc_ptr_uninitialized(&self, name: Symbol) -> Result<Pointer, EngineError> {
        self.alloc_cell(Cell::Uninitialized(name))
    }

    pub(crate) fn alloc_ptr_frame(&self, frame: Frame) -> Result<Pointer, EngineError> {
        self.alloc_cell(Cell::Frame(frame))
    }

    pub(crate) fn alloc_ptr_root_frame_parent(&self) -> Result<Pointer, EngineError> {
        self.alloc_ptr_u64(0)
    }

    pub(crate) fn replace_frame(&self, pointer: &Pointer, frame: Frame) -> Result<(), EngineError> {
        self.pointer_as_frame(pointer)?;
        self.overwrite(pointer, Cell::Frame(frame))
    }

    pub(crate) fn alloc_ptr_tuple(&self, values: Vec<Pointer>) -> Result<Pointer, EngineError> {
        self.alloc_cell(Cell::Tuple(values))
    }

    pub(crate) fn alloc_ptr_array(&self, values: Vec<Pointer>) -> Result<Pointer, EngineError> {
        self.alloc_cell(Cell::Array(values))
    }

    pub(crate) fn alloc_ptr_dict(
        &self,
        values: BTreeMap<Symbol, Pointer>,
    ) -> Result<Pointer, EngineError> {
        self.alloc_cell(Cell::Dict(values))
    }

    pub(crate) fn alloc_ptr_adt(
        &self,
        name: Symbol,
        args: Vec<Pointer>,
    ) -> Result<Pointer, EngineError> {
        self.alloc_cell(Cell::Adt(name, args))
    }

    pub(crate) fn alloc_ptr_list(&self, values: Vec<Pointer>) -> Result<Pointer, EngineError> {
        let roots = self.temp_roots(values)?;
        let mut list = self.alloc_ptr_adt(Symbol::intern("Empty"), vec![])?;
        for index in (0..roots.len()).rev() {
            let value = roots.get(index)?;
            list = self.alloc_ptr_adt(Symbol::intern("Cons"), vec![value, list])?;
        }
        Ok(list)
    }

    pub(crate) fn alloc_ptr_closure(
        &self,
        env: Environment,
        param: Symbol,
        param_ty: Type,
        typ: Type,
        body: Arc<TypedExpr>,
    ) -> Result<Pointer, EngineError> {
        self.alloc_cell(Cell::Closure(Closure {
            env,
            param,
            param_ty,
            typ,
            body,
        }))
    }

    pub(crate) fn alloc_ptr_native(
        &self,
        native_id: u64,
        name: Symbol,
        arity: usize,
        typ: Type,
        applied: Vec<Pointer>,
        applied_types: Vec<Type>,
    ) -> Result<Pointer, EngineError> {
        self.alloc_cell(Cell::Native(NativeFn::from_parts(
            native_id,
            name,
            arity,
            typ,
            applied,
            applied_types,
        )))
    }

    pub(crate) fn alloc_ptr_overloaded(
        &self,
        name: Symbol,
        typ: Type,
        applied: Vec<Pointer>,
        applied_types: Vec<Type>,
    ) -> Result<Pointer, EngineError> {
        self.alloc_cell(Cell::Overloaded(OverloadedFn::from_parts(
            name,
            typ,
            applied,
            applied_types,
        )))
    }

    pub(crate) fn overwrite(&self, pointer: &Pointer, cell: Cell) -> Result<(), EngineError> {
        self.with_access(|heap| heap.overwrite(pointer, cell))
    }

    fn unregister_external_root(&self, root_id: RootId) -> Result<(), EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::Internal("heap state poisoned".into()))?;
        Self::unregister_root_locked(self.id, &mut state, root_id)
    }

    fn resolve_external_root(&self, root_id: RootId) -> Result<Pointer, EngineError> {
        let state = self
            .state
            .lock()
            .map_err(|_| EngineError::Internal("heap state poisoned".into()))?;
        Self::resolve_root_locked(self.id, &state, root_id)
    }

    #[cfg(test)]
    fn external_root_count(&self) -> Result<usize, EngineError> {
        let state = self
            .state
            .lock()
            .map_err(|_| EngineError::Internal("heap state poisoned".into()))?;
        Ok(state
            .root_slots
            .iter()
            .filter(|slot| slot.pointer.is_some())
            .count())
    }

    #[cfg(test)]
    fn set_gc_slot_threshold(&self, threshold: usize) -> Result<(), EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::Internal("heap state poisoned".into()))?;
        state.next_gc_slot_count = threshold.max(1);
        Ok(())
    }

    #[cfg(test)]
    fn collection_count(&self) -> Result<u64, EngineError> {
        let state = self
            .state
            .lock()
            .map_err(|_| EngineError::Internal("heap state poisoned".into()))?;
        Ok(state.collections)
    }

    #[cfg(test)]
    fn collect_now(&self) -> Result<(), EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::Internal("heap state poisoned".into()))?;
        self.collect_locked(&mut state)
    }

    fn collect_if_needed_locked(&self, state: &mut HeapState) -> Result<(), EngineError> {
        if state.collect_on_every_alloc || state.slots.len() >= state.next_gc_slot_count {
            self.collect_locked(state)?;
        }
        Ok(())
    }

    fn collect_locked(&self, state: &mut HeapState) -> Result<(), EngineError> {
        let mut forwarding = HashMap::new();
        let mut new_slots = Vec::new();
        let mut work = VecDeque::new();

        let root_indices = state
            .root_slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| slot.pointer.map(|pointer| (index, pointer)))
            .collect::<Vec<_>>();

        for (index, pointer) in root_indices {
            let relocated =
                self.copy_for_gc(state, pointer, &mut new_slots, &mut forwarding, &mut work)?;
            state.root_slots[index].pointer = Some(relocated);
        }

        while let Some(old_pointer) = work.pop_front() {
            let new_pointer = *forwarding.get(&pointer_key(&old_pointer)).ok_or_else(|| {
                EngineError::Internal("copying GC forwarding table missing object".into())
            })?;
            let mut cell = new_slots
                .get_mut(new_pointer.index as usize)
                .and_then(|slot| slot.cell.take())
                .ok_or_else(|| EngineError::Internal("copying GC copied slot missing".into()))?;
            rewrite_cell_pointers(&mut cell, &mut |child| {
                self.copy_for_gc(state, child, &mut new_slots, &mut forwarding, &mut work)
            })?;
            new_slots[new_pointer.index as usize].cell = Some(cell);
        }

        state.slots = new_slots;
        state.collections = state
            .collections
            .checked_add(1)
            .ok_or_else(|| EngineError::Internal("heap collection count exhausted".into()))?;
        Self::update_next_gc_slot_count(state);
        #[cfg(debug_assertions)]
        self.verify_after_collection_locked(state)?;
        Ok(())
    }

    fn copy_for_gc(
        &self,
        state: &HeapState,
        pointer: Pointer,
        new_slots: &mut Vec<HeapSlot>,
        forwarding: &mut HashMap<PointerKey, Pointer>,
        work: &mut VecDeque<Pointer>,
    ) -> Result<Pointer, EngineError> {
        let key = pointer_key(&pointer);
        if let Some(relocated) = forwarding.get(&key) {
            return Ok(*relocated);
        }

        let slot = Self::get_slot_checked(self.id, state, &pointer)?;
        let cell = slot
            .cell
            .as_ref()
            .ok_or_else(|| Self::invalid_pointer(self.id, pointer.index, pointer.generation))?
            .clone();
        let index = u32::try_from(new_slots.len())
            .map_err(|_| EngineError::Internal("heap exhausted: too many slots".into()))?;
        let generation = slot
            .generation
            .checked_add(1)
            .ok_or_else(|| EngineError::Internal("heap object generation exhausted".into()))?;
        let relocated = Pointer {
            heap_id: self.id,
            index,
            generation,
        };
        forwarding.insert(key, relocated);
        new_slots.push(HeapSlot {
            generation,
            cell: Some(cell),
        });
        work.push_back(pointer);
        Ok(relocated)
    }

    fn update_next_gc_slot_count(state: &mut HeapState) {
        let live = state.slots.len();
        let grown = live.saturating_add(
            live.saturating_mul(GC_SLOT_GROWTH_NUMERATOR - GC_SLOT_GROWTH_DENOMINATOR)
                / GC_SLOT_GROWTH_DENOMINATOR,
        );
        let with_slack = live.saturating_add(DEFAULT_GC_SLOT_THRESHOLD);
        state.next_gc_slot_count = grown.max(with_slack).max(DEFAULT_GC_SLOT_THRESHOLD);
    }

    #[cfg(debug_assertions)]
    fn verify_after_collection_locked(&self, state: &HeapState) -> Result<(), EngineError> {
        let mut work = VecDeque::new();
        let mut seen = HashSet::new();

        for (root_index, slot) in state.root_slots.iter().enumerate() {
            let Some(pointer) = slot.pointer else {
                continue;
            };
            Self::get_slot_checked(self.id, state, &pointer).map_err(|err| {
                EngineError::Internal(format!(
                    "GC verification failed for root {root_index}: {err}"
                ))
            })?;
            if seen.insert(pointer_key(&pointer)) {
                work.push_back(pointer);
            }
        }

        while let Some(pointer) = work.pop_front() {
            let slot = Self::get_slot_checked(self.id, state, &pointer).map_err(|err| {
                EngineError::Internal(format!(
                    "GC verification failed for object {pointer:?}: {err}"
                ))
            })?;
            let cell = slot.cell.as_ref().ok_or_else(|| {
                EngineError::Internal(format!(
                    "GC verification found empty live slot for {pointer:?}"
                ))
            })?;
            let mut children = Vec::new();
            trace_cell_pointers(cell, &mut children);
            for child in children {
                Self::get_slot_checked(self.id, state, &child).map_err(|err| {
                    EngineError::Internal(format!(
                        "GC verification failed for child {child:?} of {pointer:?}: {err}"
                    ))
                })?;
                if seen.insert(pointer_key(&child)) {
                    work.push_back(child);
                }
            }
        }

        for (index, slot) in state.slots.iter().enumerate() {
            if slot.cell.is_none() {
                return Err(EngineError::Internal(format!(
                    "GC verification found empty slot {index} after collection"
                )));
            }
            let index = u32::try_from(index)
                .map_err(|_| EngineError::Internal("heap exhausted: too many slots".into()))?;
            let pointer = Pointer {
                heap_id: self.id,
                index,
                generation: slot.generation,
            };
            if !seen.contains(&pointer_key(&pointer)) {
                return Err(EngineError::Internal(format!(
                    "GC verification found unrooted copied object {pointer:?}"
                )));
            }
        }

        Ok(())
    }

    // The single ordinary heap-object allocation path. It roots pointers already
    // stored in the new cell before checking whether this allocation should
    // collect, then rewrites them to their post-collection locations.
    fn alloc_cell(&self, mut cell: Cell) -> Result<Pointer, EngineError> {
        let mut protected = Vec::new();
        trace_cell_pointers(&cell, &mut protected);

        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::Internal("heap state poisoned".into()))?;

        let mut root_ids = Vec::with_capacity(protected.len());
        for pointer in protected {
            match Self::register_root_locked(self.id, &mut state, pointer) {
                Ok(root_id) => root_ids.push(root_id),
                Err(err) => {
                    for root_id in root_ids {
                        let _ = Self::unregister_root_locked(self.id, &mut state, root_id);
                    }
                    return Err(err);
                }
            }
        }

        self.collect_if_needed_locked(&mut state)?;

        let mut relocated = Vec::with_capacity(root_ids.len());
        for root_id in &root_ids {
            relocated.push(Self::resolve_root_locked(self.id, &state, *root_id)?);
        }
        for root_id in root_ids {
            Self::unregister_root_locked(self.id, &mut state, root_id)?;
        }
        let mut relocated = relocated.into_iter();
        rewrite_cell_pointers(&mut cell, &mut |_| {
            relocated.next().ok_or_else(|| {
                EngineError::Internal("temporary allocation root count mismatch".into())
            })
        })?;
        if relocated.next().is_some() {
            return Err(EngineError::Internal(
                "temporary allocation roots were not fully consumed".into(),
            ));
        }

        let index = u32::try_from(state.slots.len())
            .map_err(|_| EngineError::Internal("heap exhausted: too many slots".into()))?;
        state.slots.push(HeapSlot {
            generation: 0,
            cell: Some(cell),
        });
        Ok(Pointer {
            heap_id: self.id,
            index,
            generation: 0,
        })
    }
}

#[derive(Clone)]
pub(crate) struct Closure {
    pub env: Environment,
    pub param: Symbol,
    pub param_ty: Type,
    pub typ: Type,
    pub body: Arc<TypedExpr>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Pointer {
    heap_id: u64,
    index: u32,
    generation: u64,
}

#[derive(Clone)]
pub(crate) enum Cell {
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
    Frame(Frame),
    Closure(Closure),
    Native(NativeFn),
    Overloaded(OverloadedFn),
}

impl Cell {
    pub(crate) fn cell_type_name(&self) -> &'static str {
        match self {
            Cell::Bool(..) => "bool",
            Cell::U8(..) => "u8",
            Cell::U16(..) => "u16",
            Cell::U32(..) => "u32",
            Cell::U64(..) => "u64",
            Cell::I8(..) => "i8",
            Cell::I16(..) => "i16",
            Cell::I32(..) => "i32",
            Cell::I64(..) => "i64",
            Cell::F32(..) => "f32",
            Cell::F64(..) => "f64",
            Cell::String(..) => "string",
            Cell::Uuid(..) => "uuid",
            Cell::DateTime(..) => "datetime",
            Cell::Tuple(..) => "tuple",
            Cell::Array(..) => "array",
            Cell::Dict(..) => "dict",
            Cell::Adt(name, ..) if name.as_ref() == "Empty" || name.as_ref() == "Cons" => "list",
            Cell::Adt(..) => "adt",
            Cell::Uninitialized(..) => "uninitialized",
            Cell::Frame(..) => "frame",
            Cell::Closure(..) => "closure",
            Cell::Native(..) => "native",
            Cell::Overloaded(..) => "overloaded",
        }
    }

    fn cell_type_error(&self, expected: &'static str) -> EngineError {
        EngineError::NativeType {
            expected: expected.to_string(),
            got: self.cell_type_name().to_string(),
        }
    }

    pub(crate) fn cell_as_bool(&self) -> Result<bool, EngineError> {
        match self {
            Cell::Bool(v) => Ok(*v),
            _ => Err(self.cell_type_error("bool")),
        }
    }

    pub(crate) fn cell_as_u8(&self) -> Result<u8, EngineError> {
        match self {
            Cell::U8(v) => Ok(*v),
            _ => Err(self.cell_type_error("u8")),
        }
    }

    pub(crate) fn cell_as_u16(&self) -> Result<u16, EngineError> {
        match self {
            Cell::U16(v) => Ok(*v),
            _ => Err(self.cell_type_error("u16")),
        }
    }

    pub(crate) fn cell_as_u32(&self) -> Result<u32, EngineError> {
        match self {
            Cell::U32(v) => Ok(*v),
            _ => Err(self.cell_type_error("u32")),
        }
    }

    pub(crate) fn cell_as_u64(&self) -> Result<u64, EngineError> {
        match self {
            Cell::U64(v) => Ok(*v),
            _ => Err(self.cell_type_error("u64")),
        }
    }

    pub(crate) fn cell_as_i8(&self) -> Result<i8, EngineError> {
        match self {
            Cell::I8(v) => Ok(*v),
            _ => Err(self.cell_type_error("i8")),
        }
    }

    pub(crate) fn cell_as_i16(&self) -> Result<i16, EngineError> {
        match self {
            Cell::I16(v) => Ok(*v),
            _ => Err(self.cell_type_error("i16")),
        }
    }

    pub(crate) fn cell_as_i32(&self) -> Result<i32, EngineError> {
        match self {
            Cell::I32(v) => Ok(*v),
            _ => Err(self.cell_type_error("i32")),
        }
    }

    pub(crate) fn cell_as_i64(&self) -> Result<i64, EngineError> {
        match self {
            Cell::I64(v) => Ok(*v),
            _ => Err(self.cell_type_error("i64")),
        }
    }

    pub(crate) fn cell_as_f32(&self) -> Result<f32, EngineError> {
        match self {
            Cell::F32(v) => Ok(*v),
            _ => Err(self.cell_type_error("f32")),
        }
    }

    pub(crate) fn cell_as_f64(&self) -> Result<f64, EngineError> {
        match self {
            Cell::F64(v) => Ok(*v),
            _ => Err(self.cell_type_error("f64")),
        }
    }

    pub(crate) fn cell_as_string(&self) -> Result<String, EngineError> {
        match self {
            Cell::String(v) => Ok(v.clone()),
            _ => Err(self.cell_type_error("string")),
        }
    }

    pub(crate) fn cell_as_uuid(&self) -> Result<Uuid, EngineError> {
        match self {
            Cell::Uuid(v) => Ok(*v),
            _ => Err(self.cell_type_error("uuid")),
        }
    }

    pub(crate) fn cell_as_datetime(&self) -> Result<DateTime<Utc>, EngineError> {
        match self {
            Cell::DateTime(v) => Ok(*v),
            _ => Err(self.cell_type_error("datetime")),
        }
    }

    pub(crate) fn cell_as_tuple(&self) -> Result<Vec<Pointer>, EngineError> {
        match self {
            Cell::Tuple(v) => Ok(v.clone()),
            _ => Err(self.cell_type_error("tuple")),
        }
    }

    pub(crate) fn cell_as_array(&self) -> Result<Vec<Pointer>, EngineError> {
        match self {
            Cell::Array(v) => Ok(v.clone()),
            _ => Err(self.cell_type_error("array")),
        }
    }

    pub(crate) fn cell_as_dict(&self) -> Result<BTreeMap<Symbol, Pointer>, EngineError> {
        match self {
            Cell::Dict(v) => Ok(v.clone()),
            _ => Err(self.cell_type_error("dict")),
        }
    }

    pub(crate) fn cell_as_adt(&self) -> Result<(Symbol, Vec<Pointer>), EngineError> {
        match self {
            Cell::Adt(name, args) => Ok((name.clone(), args.clone())),
            _ => Err(self.cell_type_error("adt")),
        }
    }

    pub(crate) fn cell_as_frame(&self) -> Result<Frame, EngineError> {
        match self {
            Cell::Frame(frame) => Ok(frame.clone()),
            _ => Err(self.cell_type_error("frame")),
        }
    }
}

fn trace_cell_pointers(cell: &Cell, out: &mut Vec<Pointer>) {
    match cell {
        Cell::Tuple(values) | Cell::Array(values) | Cell::Adt(_, values) => {
            out.extend(values.iter().copied());
        }
        Cell::Dict(values) => out.extend(values.values().copied()),
        Cell::Frame(frame) => frame.trace_pointers(out),
        Cell::Closure(closure) => closure.env.trace_pointers(out),
        Cell::Native(native) => native.trace_pointers(out),
        Cell::Overloaded(overloaded) => overloaded.trace_pointers(out),
        Cell::Bool(_)
        | Cell::U8(_)
        | Cell::U16(_)
        | Cell::U32(_)
        | Cell::U64(_)
        | Cell::I8(_)
        | Cell::I16(_)
        | Cell::I32(_)
        | Cell::I64(_)
        | Cell::F32(_)
        | Cell::F64(_)
        | Cell::String(_)
        | Cell::Uuid(_)
        | Cell::DateTime(_)
        | Cell::Uninitialized(_) => {}
    }
}

fn rewrite_cell_pointers(
    cell: &mut Cell,
    rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
) -> Result<(), EngineError> {
    match cell {
        Cell::Tuple(values) | Cell::Array(values) | Cell::Adt(_, values) => {
            for pointer in values {
                *pointer = rewrite(*pointer)?;
            }
            Ok(())
        }
        Cell::Dict(values) => {
            for pointer in values.values_mut() {
                *pointer = rewrite(*pointer)?;
            }
            Ok(())
        }
        Cell::Frame(frame) => frame.rewrite_pointers(rewrite),
        Cell::Closure(closure) => closure.env.rewrite_pointers(rewrite),
        Cell::Native(native) => native.rewrite_pointers(rewrite),
        Cell::Overloaded(overloaded) => overloaded.rewrite_pointers(rewrite),
        Cell::Bool(_)
        | Cell::U8(_)
        | Cell::U16(_)
        | Cell::U32(_)
        | Cell::U64(_)
        | Cell::I8(_)
        | Cell::I16(_)
        | Cell::I32(_)
        | Cell::I64(_)
        | Cell::F32(_)
        | Cell::F64(_)
        | Cell::String(_)
        | Cell::Uuid(_)
        | Cell::DateTime(_)
        | Cell::Uninitialized(_) => Ok(()),
    }
}

type PointerKey = (u64, u32, u64);
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
    heap: &HeapAccess<'_>,
    pointer: &Pointer,
    active: &mut HashSet<PointerKey>,
) -> Result<String, EngineError> {
    let key = pointer_key(pointer);
    if !active.insert(key) {
        return Ok(format!("<cycle:{}:{}>", pointer.index, pointer.generation));
    }
    let cell = heap.get(pointer)?;
    let out = cell_debug_inner(heap, cell, active);
    active.remove(&key);
    out
}

fn pointer_display_inner(
    heap: &HeapAccess<'_>,
    pointer: &Pointer,
    active: &mut HashSet<PointerKey>,
    opts: ValueDisplayOptions,
) -> Result<String, EngineError> {
    let key = pointer_key(pointer);
    if !active.insert(key) {
        return Ok(format!("<cycle:{}:{}>", pointer.index, pointer.generation));
    }
    let cell = heap.get(pointer)?;
    let out = cell_display_inner(heap, cell, active, opts);
    active.remove(&key);
    out
}

fn env_debug_inner(
    heap: &HeapAccess<'_>,
    env: &Environment,
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
    heap: &HeapAccess<'_>,
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

fn cell_debug_inner(
    heap: &HeapAccess<'_>,
    cell: &Cell,
    active: &mut HashSet<PointerKey>,
) -> Result<String, EngineError> {
    Ok(match cell {
        Cell::Bool(v) => v.to_string(),
        Cell::U8(v) => format!("{v}u8"),
        Cell::U16(v) => format!("{v}u16"),
        Cell::U32(v) => format!("{v}u32"),
        Cell::U64(v) => format!("{v}u64"),
        Cell::I8(v) => format!("{v}i8"),
        Cell::I16(v) => format!("{v}i16"),
        Cell::I32(v) => format!("{v}i32"),
        Cell::I64(v) => format!("{v}i64"),
        Cell::F32(v) => format!("{v}f32"),
        Cell::F64(v) => format!("{v}f64"),
        Cell::String(v) => format!("{v:?}"),
        Cell::Uuid(v) => v.to_string(),
        Cell::DateTime(v) => v.to_string(),
        Cell::Tuple(values) => {
            let items = values
                .iter()
                .map(|pointer| pointer_debug_inner(heap, pointer, active))
                .collect::<Result<Vec<_>, _>>()?;
            format!("({})", items.join(", "))
        }
        Cell::Array(values) => {
            let items = values
                .iter()
                .map(|pointer| pointer_debug_inner(heap, pointer, active))
                .collect::<Result<Vec<_>, _>>()?;
            format!("<array {}>", items.join(", "))
        }
        Cell::Dict(values) => {
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
        Cell::Adt(name, args) => {
            if let Some(values) = list_to_vec_opt(heap, cell)? {
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
        Cell::Uninitialized(name) => format!("<uninitialized:{name}>"),
        Cell::Frame(frame) => format!("<frame:{frame:?}>"),
        Cell::Closure(closure) => closure_debug_inner(heap, closure, active)?,
        Cell::Native(native) => format!("<native:{}>", native.name()),
        Cell::Overloaded(over) => format!("<overloaded:{}>", over.name()),
    })
}

fn cell_display_inner(
    heap: &HeapAccess<'_>,
    cell: &Cell,
    active: &mut HashSet<PointerKey>,
    opts: ValueDisplayOptions,
) -> Result<String, EngineError> {
    Ok(match cell {
        Cell::Bool(v) => v.to_string(),
        Cell::U8(v) => {
            if opts.include_numeric_suffixes {
                format!("{v}u8")
            } else {
                v.to_string()
            }
        }
        Cell::U16(v) => {
            if opts.include_numeric_suffixes {
                format!("{v}u16")
            } else {
                v.to_string()
            }
        }
        Cell::U32(v) => {
            if opts.include_numeric_suffixes {
                format!("{v}u32")
            } else {
                v.to_string()
            }
        }
        Cell::U64(v) => {
            if opts.include_numeric_suffixes {
                format!("{v}u64")
            } else {
                v.to_string()
            }
        }
        Cell::I8(v) => {
            if opts.include_numeric_suffixes {
                format!("{v}i8")
            } else {
                v.to_string()
            }
        }
        Cell::I16(v) => {
            if opts.include_numeric_suffixes {
                format!("{v}i16")
            } else {
                v.to_string()
            }
        }
        Cell::I32(v) => {
            if opts.include_numeric_suffixes {
                format!("{v}i32")
            } else {
                v.to_string()
            }
        }
        Cell::I64(v) => {
            if opts.include_numeric_suffixes {
                format!("{v}i64")
            } else {
                v.to_string()
            }
        }
        Cell::F32(v) => {
            if opts.include_numeric_suffixes {
                format!("{v}f32")
            } else {
                v.to_string()
            }
        }
        Cell::F64(v) => {
            if opts.include_numeric_suffixes {
                format!("{v}f64")
            } else {
                v.to_string()
            }
        }
        Cell::String(v) => format!("{v:?}"),
        Cell::Uuid(v) => v.to_string(),
        Cell::DateTime(v) => v.to_string(),
        Cell::Tuple(values) => {
            let items = values
                .iter()
                .map(|pointer| pointer_display_inner(heap, pointer, active, opts))
                .collect::<Result<Vec<_>, _>>()?;
            format!("({})", items.join(", "))
        }
        Cell::Array(values) => {
            let items = values
                .iter()
                .map(|pointer| pointer_display_inner(heap, pointer, active, opts))
                .collect::<Result<Vec<_>, _>>()?;
            format!("<array {}>", items.join(", "))
        }
        Cell::Dict(values) => {
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
        Cell::Adt(name, args) => {
            if let Some(values) = list_to_vec_opt(heap, cell)? {
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
        Cell::Uninitialized(name) => format!("<uninitialized:{name}>"),
        Cell::Frame(frame) => format!("<frame:{frame:?}>"),
        Cell::Closure(..) => "<closure>".to_string(),
        Cell::Native(native) => format!("<native:{}>", native.name()),
        Cell::Overloaded(over) => format!("<overloaded:{}>", over.name()),
    })
}

pub(crate) fn pointer_debug(heap: &Heap, pointer: &Pointer) -> Result<String, EngineError> {
    heap.with_access(|heap| {
        let mut active = HashSet::new();
        pointer_debug_inner(heap, pointer, &mut active)
    })
}

pub(crate) fn pointer_display_with(
    heap: &Heap,
    pointer: &Pointer,
    opts: ValueDisplayOptions,
) -> Result<String, EngineError> {
    heap.with_access(|heap| {
        let mut active = HashSet::new();
        pointer_display_inner(heap, pointer, &mut active, opts)
    })
}

fn pointer_eq_inner(
    heap: &HeapAccess<'_>,
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
    let lhs_cell = heap.get(lhs)?;
    let rhs_cell = heap.get(rhs)?;
    cell_eq_inner(heap, lhs_cell, rhs_cell, seen)
}

fn env_eq_inner(
    heap: &HeapAccess<'_>,
    lhs: &Environment,
    rhs: &Environment,
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
    heap: &HeapAccess<'_>,
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

fn cell_eq_inner(
    heap: &HeapAccess<'_>,
    lhs: &Cell,
    rhs: &Cell,
    seen: &mut HashSet<PointerPairKey>,
) -> Result<bool, EngineError> {
    match (lhs, rhs) {
        (Cell::Bool(lhs), Cell::Bool(rhs)) => Ok(lhs == rhs),
        (Cell::U8(lhs), Cell::U8(rhs)) => Ok(lhs == rhs),
        (Cell::U16(lhs), Cell::U16(rhs)) => Ok(lhs == rhs),
        (Cell::U32(lhs), Cell::U32(rhs)) => Ok(lhs == rhs),
        (Cell::U64(lhs), Cell::U64(rhs)) => Ok(lhs == rhs),
        (Cell::I8(lhs), Cell::I8(rhs)) => Ok(lhs == rhs),
        (Cell::I16(lhs), Cell::I16(rhs)) => Ok(lhs == rhs),
        (Cell::I32(lhs), Cell::I32(rhs)) => Ok(lhs == rhs),
        (Cell::I64(lhs), Cell::I64(rhs)) => Ok(lhs == rhs),
        (Cell::F32(lhs), Cell::F32(rhs)) => Ok(lhs == rhs),
        (Cell::F64(lhs), Cell::F64(rhs)) => Ok(lhs == rhs),
        (Cell::String(lhs), Cell::String(rhs)) => Ok(lhs == rhs),
        (Cell::Uuid(lhs), Cell::Uuid(rhs)) => Ok(lhs == rhs),
        (Cell::DateTime(lhs), Cell::DateTime(rhs)) => Ok(lhs == rhs),
        (Cell::Tuple(lhs), Cell::Tuple(rhs)) | (Cell::Array(lhs), Cell::Array(rhs)) => {
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
        (Cell::Dict(lhs), Cell::Dict(rhs)) => {
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
        (Cell::Adt(lhs_name, lhs_args), Cell::Adt(rhs_name, rhs_args)) => {
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
        (Cell::Uninitialized(lhs), Cell::Uninitialized(rhs)) => Ok(lhs == rhs),
        (Cell::Frame(lhs), Cell::Frame(rhs)) => Ok(lhs == rhs),
        (Cell::Closure(lhs), Cell::Closure(rhs)) => closure_eq_inner(heap, lhs, rhs, seen),
        (Cell::Native(lhs), Cell::Native(rhs)) => Ok(lhs == rhs),
        (Cell::Overloaded(lhs), Cell::Overloaded(rhs)) => Ok(lhs == rhs),
        _ => Ok(false),
    }
}

pub(crate) fn pointer_eq(heap: &Heap, lhs: &Pointer, rhs: &Pointer) -> Result<bool, EngineError> {
    heap.with_access(|heap| {
        let mut seen = HashSet::new();
        pointer_eq_inner(heap, lhs, rhs, &mut seen)
    })
}

fn list_to_vec_opt(
    heap: &HeapAccess<'_>,
    cell: &Cell,
) -> Result<Option<Vec<Pointer>>, EngineError> {
    let mut out = Vec::new();
    let mut cursor = cell;
    loop {
        match cursor {
            Cell::Adt(tag, args) if tag.as_ref() == "Empty" && args.is_empty() => {
                return Ok(Some(out));
            }
            Cell::Adt(tag, args) if tag.as_ref() == "Cons" && args.len() == 2 => {
                out.push(args[0]);
                cursor = heap.get(&args[1])?;
            }
            _ => return Ok(None),
        }
    }
}

pub(crate) fn list_to_vec(heap: &HeapAccess<'_>, cell: &Cell) -> Result<Vec<Pointer>, EngineError> {
    let mut out = Vec::new();
    let mut cursor = cell;
    loop {
        match cursor {
            Cell::Adt(tag, args) if tag.as_ref() == "Empty" && args.is_empty() => return Ok(out),
            Cell::Adt(tag, args) if tag.as_ref() == "Cons" && args.len() == 2 => {
                out.push(args[0]);
                cursor = heap.get(&args[1])?;
            }
            _ => {
                return Err(EngineError::NativeType {
                    expected: "list".into(),
                    got: cursor.cell_type_name().into(),
                });
            }
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
        heap.alloc_ptr_cell(self)
    }
}

impl IntoPointer for &Cell {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_ptr_cell(self.clone())
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
        heap.alloc_ptr_bool(self)
    }
}

impl IntoPointer for u8 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_ptr_u8(self)
    }
}

impl IntoPointer for u16 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_ptr_u16(self)
    }
}

impl IntoPointer for u32 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_ptr_u32(self)
    }
}

impl IntoPointer for u64 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_ptr_u64(self)
    }
}

impl IntoPointer for i8 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_ptr_i8(self)
    }
}

impl IntoPointer for i16 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_ptr_i16(self)
    }
}

impl IntoPointer for i32 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_ptr_i32(self)
    }
}

impl IntoPointer for i64 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_ptr_i64(self)
    }
}

impl IntoPointer for f32 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_ptr_f32(self)
    }
}

impl IntoPointer for f64 {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_ptr_f64(self)
    }
}

impl IntoPointer for String {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_ptr_string(self)
    }
}

impl IntoPointer for &str {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_ptr_string(self.to_string())
    }
}

impl<T: IntoPointer> IntoPointer for Vec<T> {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        let mut roots = Vec::new();
        for value in self {
            let pointer = value.into_pointer(heap)?;
            roots.push(heap.temp_roots(vec![pointer])?);
        }
        let ptrs = roots
            .iter()
            .map(|root| root.get(0))
            .collect::<Result<Vec<_>, _>>()?;
        heap.alloc_ptr_array(ptrs)
    }
}

impl<T: IntoPointer> IntoPointer for Option<T> {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        match self {
            Some(v) => {
                let ptr = v.into_pointer(heap)?;
                heap.alloc_ptr_adt(Symbol::intern("Some"), vec![ptr])
            }
            None => heap.alloc_ptr_adt(Symbol::intern("None"), vec![]),
        }
    }
}

fn handle_from_pointer(heap: &Heap, pointer: Pointer) -> Result<Handle, EngineError> {
    heap.handle(pointer)
}

impl IntoRex for Handle {
    fn into_rex(self, heap: &Heap) -> Result<Handle, EngineError> {
        let pointer = self.pointer()?;
        if pointer.heap_id != heap.id {
            return Err(Heap::wrong_heap_pointer(
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

impl<T: IntoRex> IntoRex for Vec<T> {
    fn into_rex(self, heap: &Heap) -> Result<Handle, EngineError> {
        let values = self
            .into_iter()
            .map(|value| value.into_rex(heap))
            .collect::<Result<Vec<_>, _>>()?;
        let pointers = values
            .iter()
            .map(Handle::pointer)
            .collect::<Result<Vec<_>, _>>()?;
        handle_from_pointer(heap, heap.alloc_ptr_array(pointers)?)
    }
}

impl<T: FromRex> FromRex for Vec<T> {
    fn from_rex(handle: &Handle) -> Result<Self, EngineError> {
        let heap = handle.heap();
        let pointer = handle.pointer()?;
        let pointers = heap.pointer_as_array(&pointer)?;
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
                handle_from_pointer(
                    heap,
                    heap.alloc_ptr_adt(Symbol::intern("Some"), vec![value.pointer()?])?,
                )
            }
            None => handle_from_pointer(heap, heap.alloc_ptr_adt(Symbol::intern("None"), vec![])?),
        }
    }
}

impl<T: FromRex> FromRex for Option<T> {
    fn from_rex(handle: &Handle) -> Result<Self, EngineError> {
        let heap = handle.heap();
        let pointer = handle.pointer()?;
        let (tag, args) = heap.pointer_as_adt(&pointer)?;
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
        heap.alloc_ptr_uuid(self)
    }
}

impl IntoPointer for DateTime<Utc> {
    fn into_pointer(self, heap: &Heap) -> Result<Pointer, EngineError> {
        heap.alloc_ptr_datetime(self)
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

impl FromPointer for Cell {
    fn from_pointer(heap: &Heap, pointer: &Pointer) -> Result<Self, EngineError> {
        heap.clone_cell(pointer)
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
        if tag.as_ref() == "Some" && args.len() == 1 {
            return Ok(Some(T::from_pointer(heap, &args[0])?));
        }
        if tag.as_ref() == "None" && args.is_empty() {
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
                heap.alloc_ptr_adt(Symbol::intern("Ok"), vec![ptr])
            }
            Err(e) => {
                let ptr = e.into_pointer(heap)?;
                heap.alloc_ptr_adt(Symbol::intern("Err"), vec![ptr])
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
        let (tag, args) = heap.pointer_as_adt(pointer)?;
        if tag.as_ref() == "Ok" && args.len() == 1 {
            return Ok(Ok(T::from_pointer(heap, &args[0])?));
        }
        if tag.as_ref() == "Err" && args.len() == 1 {
            return Ok(Err(E::from_pointer(heap, &args[0])?));
        }
        Err(EngineError::NativeType {
            expected: "result".into(),
            got: heap.type_name(pointer)?.into(),
        })
    }
}

impl<T: IntoRex, E: IntoRex> IntoRex for Result<T, E> {
    fn into_rex(self, heap: &Heap) -> Result<Handle, EngineError> {
        match self {
            Ok(value) => {
                let value = value.into_rex(heap)?;
                handle_from_pointer(
                    heap,
                    heap.alloc_ptr_adt(Symbol::intern("Ok"), vec![value.pointer()?])?,
                )
            }
            Err(error) => {
                let error = error.into_rex(heap)?;
                handle_from_pointer(
                    heap,
                    heap.alloc_ptr_adt(Symbol::intern("Err"), vec![error.pointer()?])?,
                )
            }
        }
    }
}

impl<T: FromRex, E: FromRex> FromRex for Result<T, E> {
    fn from_rex(handle: &Handle) -> Result<Self, EngineError> {
        let heap = handle.heap();
        let pointer = handle.pointer()?;
        let (tag, args) = heap.pointer_as_adt(&pointer)?;
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
        heap.alloc_ptr_tuple(vec![])
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
                heap.alloc_ptr_tuple(ptrs)
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

        impl<$($name: IntoRex),+> IntoRex for ($($name,)+) {
            #[allow(non_snake_case)]
            fn into_rex(self, heap: &Heap) -> Result<Handle, EngineError> {
                let ($($name,)+) = self;
                $(let $name = $name.into_rex(heap)?;)+
                let ptrs = vec![$($name.pointer()?),+];
                handle_from_pointer(heap, heap.alloc_ptr_tuple(ptrs)?)
            }
        }

        impl<$($name: FromRex),+> FromRex for ($($name,)+) {
            #[allow(non_snake_case)]
            fn from_rex(handle: &Handle) -> Result<Self, EngineError> {
                let heap = handle.heap();
                let pointer = handle.pointer()?;
                let items = heap.pointer_as_tuple(&pointer)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_roots_value_until_last_clone_drops() {
        let heap = Heap::new();
        let pointer = heap.alloc_ptr_i32(42).expect("alloc_i32 should succeed");
        assert_eq!(heap.external_root_count().expect("root count"), 0);

        let handle = heap.handle(pointer).expect("handle should root pointer");
        assert_eq!(handle.type_name().expect("handle type name"), "i32");
        assert_eq!(heap.external_root_count().expect("root count"), 1);

        let clone = handle.clone();
        assert_eq!(heap.external_root_count().expect("root count"), 1);

        drop(handle);
        assert_eq!(heap.external_root_count().expect("root count"), 1);

        drop(clone);
        assert_eq!(heap.external_root_count().expect("root count"), 0);
    }

    #[test]
    fn handle_root_ids_are_reused_with_generation_bump() {
        let heap = Heap::new();
        let first_pointer = heap.alloc_ptr_i32(1).expect("alloc_i32 should succeed");
        let first = heap
            .handle(first_pointer)
            .expect("handle should root pointer");
        let first_root_id = first.root.root_id;
        drop(first);

        let second_pointer = heap.alloc_ptr_i32(2).expect("alloc_i32 should succeed");
        let second = heap
            .handle(second_pointer)
            .expect("handle should reuse root slot");
        let second_root_id = second.root.root_id;

        assert_eq!(second_root_id.index, first_root_id.index);
        assert_eq!(second_root_id.generation, first_root_id.generation + 1);
        assert_eq!(heap.external_root_count().expect("root count"), 1);
    }

    #[test]
    fn handle_resolves_pointer_from_root_slot() {
        let heap = Heap::new();
        let first_pointer = heap.alloc_ptr_i32(1).expect("alloc_i32 should succeed");
        let second_pointer = heap.alloc_ptr_i32(2).expect("alloc_i32 should succeed");
        let handle = heap
            .handle(first_pointer)
            .expect("handle should root pointer");

        {
            let mut state = heap.state.lock().expect("heap state lock");
            let slot = state
                .root_slots
                .get_mut(handle.root.root_id.index as usize)
                .expect("root slot should exist");
            assert_eq!(slot.pointer, Some(first_pointer));
            slot.pointer = Some(second_pointer);
        }

        assert_eq!(handle.as_i32().expect("handle should follow root slot"), 2);
    }

    #[test]
    fn handle_rejects_pointer_from_different_heap() {
        let heap_a = Heap::new();
        let heap_b = Heap::new();
        let pointer = heap_a.alloc_ptr_i32(42).expect("alloc_i32 should succeed");

        let err = match heap_b.handle(pointer) {
            Ok(_) => panic!("cross-heap pointer should not be rootable"),
            Err(err) => err,
        };
        let EngineError::Internal(msg) = err else {
            panic!("expected internal error for cross-heap pointer");
        };
        assert!(msg.contains("different heap"), "unexpected error: {msg}");
        assert_eq!(heap_b.external_root_count().expect("root count"), 0);
    }

    #[test]
    fn copying_gc_updates_handles_and_rejects_stale_pointers() {
        let heap = Heap::new();
        let stale = heap.alloc_ptr_i32(42).expect("alloc_i32 should succeed");
        let handle = heap.handle(stale).expect("handle should root pointer");

        heap.collect_now().expect("collection should succeed");

        assert_eq!(
            handle.as_i32().expect("handle should follow moved value"),
            42
        );
        assert!(
            heap.pointer_as_i32(&stale).is_err(),
            "raw pointer from before collection should be stale"
        );
    }

    #[test]
    fn alloc_triggers_collection_after_heap_growth() {
        let heap = Heap::new();
        let rooted = heap.alloc_i32(7).expect("alloc_i32 handle");
        heap.set_gc_slot_threshold(1).expect("set threshold");

        let _garbage = heap.alloc_ptr_i32(99).expect("alloc should trigger GC");

        assert!(
            heap.collection_count().expect("collection count") > 0,
            "allocation should have triggered collection"
        );
        assert_eq!(rooted.as_i32().expect("rooted value"), 7);
    }

    #[test]
    fn alloc_list_protects_inputs_across_collection() {
        let heap = Heap::new();
        heap.set_gc_slot_threshold(usize::MAX)
            .expect("set threshold");
        let values = (0..2048)
            .map(|value| heap.alloc_ptr_i32(value).expect("alloc_i32 should succeed"))
            .collect::<Vec<_>>();
        heap.set_gc_slot_threshold(1).expect("set threshold");

        let list = heap
            .alloc_ptr_list(values)
            .expect("list allocation should protect inputs");
        let list = heap.handle(list).expect("list should be rootable");
        let values = heap
            .pointer_as_list(&list.pointer().expect("list pointer"))
            .expect("list should decode");

        assert_eq!(values.len(), 2048);
        assert_eq!(
            heap.pointer_as_i32(values.first().expect("first value"))
                .expect("first i32"),
            0
        );
        assert_eq!(
            heap.pointer_as_i32(values.last().expect("last value"))
                .expect("last i32"),
            2047
        );
    }

    #[test]
    fn copying_gc_traces_deep_lists_iteratively() {
        let heap = Heap::new();
        heap.set_gc_slot_threshold(usize::MAX)
            .expect("set threshold");
        let values = (0..10_000)
            .map(|value| heap.alloc_ptr_i32(value).expect("alloc_i32 should succeed"))
            .collect::<Vec<_>>();
        let list = heap
            .handle(
                heap.alloc_ptr_list(values)
                    .expect("list allocation should succeed"),
            )
            .expect("list should be rootable");

        heap.collect_now().expect("deep collection should succeed");

        let pointer = list.pointer().expect("list pointer");
        assert_eq!(
            heap.pointer_as_list(&pointer)
                .expect("list should decode after GC")
                .len(),
            10_000
        );
    }

    #[test]
    fn handle_value_reports_scalar_variants() {
        let heap = Heap::new();

        let number = 42u64.into_rex(&heap).expect("u64 should convert");
        let Value::U64(value) = number.value().expect("u64 value") else {
            panic!("expected u64 value");
        };
        assert_eq!(value, 42);
        assert_eq!(number.as_u64().expect("u64 handle"), 42);

        let text = "hello".into_rex(&heap).expect("str should convert");
        let Value::String(value) = text.value().expect("string value") else {
            panic!("expected string value");
        };
        assert_eq!(value, "hello");
        assert_eq!(text.as_string().expect("string handle"), "hello");
    }

    #[test]
    fn handle_value_roots_composite_children() {
        let heap = Heap::new();
        let first = heap.alloc_ptr_i32(1).expect("alloc_i32 should succeed");
        let second = heap
            .alloc_ptr_string("two".into())
            .expect("alloc_string should succeed");
        let tuple = heap
            .handle(
                heap.alloc_ptr_tuple(vec![first, second])
                    .expect("alloc_tuple should succeed"),
            )
            .expect("tuple should be rootable");

        assert_eq!(heap.external_root_count().expect("root count"), 1);

        let view = tuple.value().expect("tuple value");
        let Value::Tuple(items) = &view else {
            panic!("expected tuple value");
        };
        assert_eq!(heap.external_root_count().expect("root count"), 3);
        assert_eq!(items.len(), 2);
        assert_eq!(i32::from_rex(&items[0]).expect("i32 should decode"), 1);
        assert_eq!(
            String::from_rex(&items[1]).expect("string should decode"),
            "two"
        );

        drop(view);
        assert_eq!(heap.external_root_count().expect("root count"), 1);

        let items = tuple.as_tuple().expect("tuple handle");
        assert_eq!(heap.external_root_count().expect("root count"), 3);
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn handle_value_reports_named_composites() {
        let heap = Heap::new();
        let payload = heap
            .alloc_ptr_bool(true)
            .expect("alloc_bool should succeed");

        let mut fields = BTreeMap::new();
        fields.insert(Symbol::intern("ready"), payload);
        let dict = heap
            .handle(
                heap.alloc_ptr_dict(fields)
                    .expect("alloc_dict should succeed"),
            )
            .expect("dict should be rootable");
        let Value::Dict(fields) = dict.value().expect("dict value") else {
            panic!("expected dict value");
        };
        assert!(
            bool::from_rex(fields.get(&Symbol::intern("ready")).expect("ready field"))
                .expect("bool should decode")
        );
        assert!(
            dict.as_dict()
                .expect("dict handle")
                .get(&Symbol::intern("ready"))
                .expect("ready field")
                .as_bool()
                .expect("bool handle")
        );

        let option = heap
            .handle(
                heap.alloc_ptr_adt(Symbol::intern("Some"), vec![payload])
                    .expect("alloc_adt should succeed"),
            )
            .expect("adt should be rootable");
        let Value::Adt(tag, args) = option.value().expect("adt value") else {
            panic!("expected adt value");
        };
        assert!(tag.as_ref() == "Some");
        assert_eq!(args.len(), 1);
        assert!(bool::from_rex(&args[0]).expect("bool should decode"));
        let (tag, args) = option.as_adt().expect("adt handle");
        assert!(tag.as_ref() == "Some");
        assert_eq!(args.len(), 1);
        assert!(args[0].as_bool().expect("bool handle"));
    }

    #[test]
    fn rex_traits_roundtrip_owned_scalars() {
        let heap = Heap::new();

        let number = 42u64.into_rex(&heap).expect("u64 should convert");
        assert_eq!(u64::from_rex(&number).expect("u64 should decode"), 42);

        let text = "hello".into_rex(&heap).expect("str should convert");
        assert_eq!(
            String::from_rex(&text).expect("string should decode"),
            "hello"
        );

        assert_eq!(heap.external_root_count().expect("root count"), 2);
    }

    #[test]
    fn rex_traits_roundtrip_containers() {
        let heap = Heap::new();

        let array = vec![1i32, 2, 3]
            .into_rex(&heap)
            .expect("vec should convert");
        assert_eq!(heap.external_root_count().expect("root count"), 1);
        assert_eq!(
            Vec::<i32>::from_rex(&array).expect("vec should decode"),
            vec![1, 2, 3]
        );

        let option = Some("value".to_string())
            .into_rex(&heap)
            .expect("option should convert");
        assert_eq!(
            Option::<String>::from_rex(&option).expect("option should decode"),
            Some("value".to_string())
        );

        let result = Result::<u32, String>::Err("nope".to_string())
            .into_rex(&heap)
            .expect("result should convert");
        assert_eq!(
            Result::<u32, String>::from_rex(&result).expect("result should decode"),
            Err("nope".to_string())
        );

        let tuple = (true, 9i64, "nine".to_string())
            .into_rex(&heap)
            .expect("tuple should convert");
        assert_eq!(
            <(bool, i64, String)>::from_rex(&tuple).expect("tuple should decode"),
            (true, 9, "nine".to_string())
        );
    }

    #[test]
    fn rex_traits_keep_handle_on_one_root() {
        let heap = Heap::new();
        let handle = 7i32.into_rex(&heap).expect("i32 should convert");
        assert_eq!(heap.external_root_count().expect("root count"), 1);

        let cloned = Handle::from_rex(&handle).expect("handle should clone");
        assert_eq!(heap.external_root_count().expect("root count"), 1);

        let returned = cloned.into_rex(&heap).expect("handle should convert");
        assert_eq!(i32::from_rex(&returned).expect("i32 should decode"), 7);
        assert_eq!(heap.external_root_count().expect("root count"), 1);

        drop(handle);
        assert_eq!(heap.external_root_count().expect("root count"), 1);
        drop(returned);
        assert_eq!(heap.external_root_count().expect("root count"), 0);
    }
}
