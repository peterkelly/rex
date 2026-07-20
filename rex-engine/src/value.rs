//! Core value representation for Rex.

use std::any::{Any, TypeId};
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rex_ast::Symbol;
use rex_typesystem::types::{Type, TypedExpr};
use uuid::Uuid;

use crate::{
    EngineError, Environment, native_fn::NativeFn, overloaded_fn::OverloadedFn, stack::Frame,
};

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
//   frame/task state that implements Collection.
// - The collector is copying: every traced Pointer must be rewritten after a
//   collection. A raw Pointer from before collection is stale by design.
pub(crate) struct HeapState {
    id: u64,
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

impl HeapState {
    fn new() -> Self {
        static NEXT_HEAP_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_HEAP_ID.fetch_add(1, Ordering::Relaxed);
        Self {
            id,
            slots: Vec::new(),
            root_slots: Vec::new(),
            free_root_list: Vec::new(),
            next_gc_slot_count: DEFAULT_GC_SLOT_THRESHOLD,
            collect_on_every_alloc: false,
            collections: 0,
        }
    }

    fn collect_needed(&self) -> bool {
        self.collect_on_every_alloc || self.slots.len() >= self.next_gc_slot_count
    }

    fn push_cell<'a>(&'a mut self, cell: Cell) -> Result<Reference<'a>, EngineError> {
        let index = u32::try_from(self.slots.len())
            .map_err(|_| EngineError::Internal("heap exhausted: too many slots".into()))?;
        self.slots.push(HeapSlot {
            generation: 0,
            cell: Some(cell),
        });
        Ok(Reference {
            heap: self,
            index,
            generation: 0,
        })
    }

    pub(crate) fn set_collect_on_every_alloc(&mut self, enabled: bool) {
        self.collect_on_every_alloc = enabled;
    }

    pub(crate) fn collection_count(&self) -> u64 {
        self.collections
    }

    fn finish_collection(&mut self, slots: Vec<HeapSlot>) -> Result<(), EngineError> {
        self.slots = slots;
        self.collections = self
            .collections
            .checked_add(1)
            .ok_or_else(|| EngineError::Internal("heap collection count exhausted".into()))?;
        self.update_next_gc_slot_count();
        Ok(())
    }

    #[cfg(test)]
    fn set_gc_slot_threshold(&mut self, threshold: usize) {
        self.next_gc_slot_count = threshold.max(1);
    }

    fn update_next_gc_slot_count(&mut self) {
        let live = self.slots.len();
        let grown = live.saturating_add(
            live.saturating_mul(GC_SLOT_GROWTH_NUMERATOR - GC_SLOT_GROWTH_DENOMINATOR)
                / GC_SLOT_GROWTH_DENOMINATOR,
        );
        let with_slack = live.saturating_add(DEFAULT_GC_SLOT_THRESHOLD);
        self.next_gc_slot_count = grown.max(with_slack).max(DEFAULT_GC_SLOT_THRESHOLD);
    }

    fn get_slot_checked<'a>(&'a self, pointer: &Pointer) -> Result<&'a HeapSlot, EngineError> {
        if pointer.heap_id != self.id {
            return Err(wrong_heap_pointer(
                pointer.heap_id,
                self.id,
                pointer.index,
                pointer.generation,
            ));
        }
        let slot = self
            .slots
            .get(pointer.index as usize)
            .ok_or_else(|| invalid_pointer(self.id, pointer.index, pointer.generation))?;
        if slot.generation != pointer.generation || slot.cell.is_none() {
            return Err(invalid_pointer(self.id, pointer.index, pointer.generation));
        }
        Ok(slot)
    }

    fn register_root(&mut self, pointer: Pointer) -> Result<RootId, EngineError> {
        self.get_slot_checked(&pointer)?;

        if let Some(index) = self.free_root_list.pop() {
            let slot_index = usize::try_from(index)
                .map_err(|_| EngineError::Internal("heap root index overflow".into()))?;
            let slot = self
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
                heap_id: self.id,
                index,
                generation: slot.generation,
            });
        }

        let index = u64::try_from(self.root_slots.len())
            .map_err(|_| EngineError::Internal("heap exhausted: too many root slots".into()))?;
        self.root_slots.push(RootSlot {
            generation: 0,
            pointer: Some(pointer),
        });
        Ok(RootId {
            heap_id: self.id,
            index,
            generation: 0,
        })
    }

    fn unregister_root(&mut self, root_id: RootId) -> Result<(), EngineError> {
        if root_id.heap_id != self.id {
            return Err(invalid_root(root_id));
        }
        let slot_index = usize::try_from(root_id.index)
            .map_err(|_| EngineError::Internal("heap root index overflow".into()))?;
        let slot = self
            .root_slots
            .get_mut(slot_index)
            .ok_or_else(|| invalid_root(root_id))?;
        if slot.generation != root_id.generation || slot.pointer.is_none() {
            return Err(invalid_root(root_id));
        }
        let next_generation = slot
            .generation
            .checked_add(1)
            .ok_or_else(|| EngineError::Internal("heap root generation exhausted".into()))?;
        slot.pointer = None;
        slot.generation = next_generation;
        self.free_root_list.push(root_id.index);
        Ok(())
    }

    fn unregister_roots(&mut self, root_ids: Vec<RootId>) -> Result<(), EngineError> {
        let mut first_error = None;
        for root_id in root_ids {
            if let Err(err) = self.unregister_root(root_id)
                && first_error.is_none()
            {
                first_error = Some(err);
            }
        }
        if let Some(err) = first_error {
            Err(err)
        } else {
            Ok(())
        }
    }

    fn resolve_root(&self, root_id: RootId) -> Result<Pointer, EngineError> {
        if root_id.heap_id != self.id {
            return Err(invalid_root(root_id));
        }
        let slot_index = usize::try_from(root_id.index)
            .map_err(|_| EngineError::Internal("heap root index overflow".into()))?;
        let slot = self
            .root_slots
            .get(slot_index)
            .ok_or_else(|| invalid_root(root_id))?;
        if slot.generation != root_id.generation {
            return Err(invalid_root(root_id));
        }
        slot.pointer.ok_or_else(|| invalid_root(root_id))
    }

    #[cfg(test)]
    fn root_count(&self) -> usize {
        self.root_slots
            .iter()
            .filter(|slot| slot.pointer.is_some())
            .count()
    }

    fn collect(&mut self) -> Result<(), EngineError> {
        let mut forwarding = vec![None; self.slots.len()];
        let mut new_slots = Vec::with_capacity(self.slots.len());

        let root_indices = self
            .root_slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| slot.pointer.map(|pointer| (index, pointer)))
            .collect::<Vec<_>>();

        for (index, pointer) in root_indices {
            let relocated = self.copy_for_gc(pointer, &mut new_slots, &mut forwarding)?;
            self.root_slots[index].pointer = Some(relocated);
        }

        let mut scan_index = 0;
        while scan_index < new_slots.len() {
            let mut cell = new_slots
                .get_mut(scan_index)
                .and_then(|slot| slot.cell.take())
                .ok_or_else(|| EngineError::Internal("copying GC copied slot missing".into()))?;
            cell.map_pointers(&mut |child| {
                self.copy_for_gc(child, &mut new_slots, &mut forwarding)
            })?;
            new_slots[scan_index].cell = Some(cell);
            scan_index += 1;
        }

        self.finish_collection(new_slots)?;
        #[cfg(debug_assertions)]
        self.verify_after_collection()?;
        Ok(())
    }

    fn copy_for_gc(
        &self,
        pointer: Pointer,
        new_slots: &mut Vec<HeapSlot>,
        forwarding: &mut [Option<Pointer>],
    ) -> Result<Pointer, EngineError> {
        let slot = self.get_slot_checked(&pointer)?;
        let forwarding_index = pointer.index as usize;
        if let Some(relocated) = forwarding.get(forwarding_index).ok_or_else(|| {
            EngineError::Internal("copying GC forwarding table missing slot".into())
        })? {
            return Ok(*relocated);
        }

        let cell = slot
            .cell
            .as_ref()
            .ok_or_else(|| invalid_pointer(self.id, pointer.index, pointer.generation))?
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
        forwarding[forwarding_index] = Some(relocated);
        new_slots.push(HeapSlot {
            generation,
            cell: Some(cell),
        });
        Ok(relocated)
    }

    #[cfg(debug_assertions)]
    fn verify_after_collection(&self) -> Result<(), EngineError> {
        let mut work = VecDeque::new();
        let mut seen = HashSet::new();

        for (root_index, slot) in self.root_slots.iter().enumerate() {
            let Some(pointer) = slot.pointer else {
                continue;
            };
            self.get_slot_checked(&pointer).map_err(|err| {
                EngineError::Internal(format!(
                    "GC verification failed for root {root_index}: {err}"
                ))
            })?;
            if seen.insert(pointer_key(&pointer)) {
                work.push_back(pointer);
            }
        }

        while let Some(pointer) = work.pop_front() {
            let slot = self.get_slot_checked(&pointer).map_err(|err| {
                EngineError::Internal(format!(
                    "GC verification failed for object {pointer:?}: {err}"
                ))
            })?;
            let cell = slot.cell.as_ref().ok_or_else(|| {
                EngineError::Internal(format!(
                    "GC verification found empty live slot for {pointer:?}"
                ))
            })?;
            let mut cell = cell.clone();
            let mut children = Vec::new();
            cell.trace_pointers(&mut children);
            for child in children {
                self.get_slot_checked(&child).map_err(|err| {
                    EngineError::Internal(format!(
                        "GC verification failed for child {child:?} of {pointer:?}: {err}"
                    ))
                })?;
                if seen.insert(pointer_key(&child)) {
                    work.push_back(child);
                }
            }
        }

        for (index, slot) in self.slots.iter().enumerate() {
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

    // The single ordinary heap-object allocation path. When allocation will
    // collect, it roots pointers already stored in the new cell, then rewrites
    // them to their post-collection locations.
    fn alloc_reference<'a>(&'a mut self, mut cell: Cell) -> Result<Reference<'a>, EngineError> {
        if !self.collect_needed() {
            return self.push_cell(cell);
        }

        let mut protected = Vec::new();
        cell.trace_pointers(&mut protected);

        let mut root_ids = Vec::with_capacity(protected.len());
        for pointer in protected {
            match self.register_root(pointer) {
                Ok(root_id) => root_ids.push(root_id),
                Err(err) => {
                    for root_id in root_ids {
                        let _ = self.unregister_root(root_id);
                    }
                    return Err(err);
                }
            }
        }

        self.collect()?;

        let mut relocated = Vec::with_capacity(root_ids.len());
        for root_id in &root_ids {
            relocated.push(self.resolve_root(*root_id)?);
        }
        for root_id in root_ids {
            self.unregister_root(root_id)?;
        }
        let mut relocated = relocated.into_iter();
        cell.map_pointers(&mut |_| {
            relocated.next().ok_or_else(|| {
                EngineError::Internal("temporary allocation root count mismatch".into())
            })
        })?;
        if relocated.next().is_some() {
            return Err(EngineError::Internal(
                "temporary allocation roots were not fully consumed".into(),
            ));
        }

        self.push_cell(cell)
    }

    pub(crate) fn get_cell_from_pointer(&self, pointer: &Pointer) -> Result<&Cell, EngineError> {
        if pointer.heap_id != self.id {
            return Err(wrong_heap_pointer(
                pointer.heap_id,
                self.id,
                pointer.index,
                pointer.generation,
            ));
        }
        let heap_id = self.id;
        let slot = self
            .slots
            .get(pointer.index as usize)
            .ok_or_else(|| invalid_pointer(heap_id, pointer.index, pointer.generation))?;
        if slot.generation != pointer.generation {
            return Err(invalid_pointer(self.id, pointer.index, pointer.generation));
        }
        let heap_id = self.id;
        slot.cell
            .as_ref()
            .ok_or_else(|| invalid_pointer(heap_id, pointer.index, pointer.generation))
    }

    pub(crate) fn type_name(&self, pointer: &Pointer) -> Result<&'static str, EngineError> {
        self.get_cell_from_pointer(pointer)
            .map(Cell::cell_type_name)
    }

    pub(crate) fn overwrite(&mut self, pointer: &Pointer, cell: Cell) -> Result<(), EngineError> {
        if pointer.heap_id != self.id {
            return Err(wrong_heap_pointer(
                pointer.heap_id,
                self.id,
                pointer.index,
                pointer.generation,
            ));
        }

        let heap_id = self.id;
        let slot = self
            .slots
            .get_mut(pointer.index as usize)
            .ok_or_else(|| invalid_pointer(heap_id, pointer.index, pointer.generation))?;
        if slot.generation != pointer.generation {
            return Err(invalid_pointer(self.id, pointer.index, pointer.generation));
        }
        slot.cell = Some(cell);
        Ok(())
    }

    pub(crate) fn alloc_ptr_bool(&mut self, value: bool) -> Result<Reference<'_>, EngineError> {
        self.alloc_reference(Cell::Bool(value))
    }

    pub(crate) fn alloc_ptr_u8(&mut self, value: u8) -> Result<Reference<'_>, EngineError> {
        self.alloc_reference(Cell::U8(value))
    }

    pub(crate) fn alloc_ptr_u16(&mut self, value: u16) -> Result<Reference<'_>, EngineError> {
        self.alloc_reference(Cell::U16(value))
    }

    pub(crate) fn alloc_ptr_u32(&mut self, value: u32) -> Result<Reference<'_>, EngineError> {
        self.alloc_reference(Cell::U32(value))
    }

    pub(crate) fn alloc_ptr_u64(&mut self, value: u64) -> Result<Reference<'_>, EngineError> {
        self.alloc_reference(Cell::U64(value))
    }

    pub(crate) fn alloc_ptr_i8(&mut self, value: i8) -> Result<Reference<'_>, EngineError> {
        self.alloc_reference(Cell::I8(value))
    }

    pub(crate) fn alloc_ptr_i16(&mut self, value: i16) -> Result<Reference<'_>, EngineError> {
        self.alloc_reference(Cell::I16(value))
    }

    pub(crate) fn alloc_ptr_i32(&mut self, value: i32) -> Result<Reference<'_>, EngineError> {
        self.alloc_reference(Cell::I32(value))
    }

    pub(crate) fn alloc_ptr_i64(&mut self, value: i64) -> Result<Reference<'_>, EngineError> {
        self.alloc_reference(Cell::I64(value))
    }

    pub(crate) fn alloc_ptr_f32(&mut self, value: f32) -> Result<Reference<'_>, EngineError> {
        self.alloc_reference(Cell::F32(value))
    }

    pub(crate) fn alloc_ptr_f64(&mut self, value: f64) -> Result<Reference<'_>, EngineError> {
        self.alloc_reference(Cell::F64(value))
    }

    pub(crate) fn alloc_ptr_string(&mut self, value: String) -> Result<Reference<'_>, EngineError> {
        self.alloc_reference(Cell::String(value))
    }

    pub(crate) fn alloc_ptr_uuid(&mut self, value: Uuid) -> Result<Reference<'_>, EngineError> {
        self.alloc_reference(Cell::Uuid(value))
    }

    pub(crate) fn alloc_ptr_datetime(
        &mut self,
        value: DateTime<Utc>,
    ) -> Result<Reference<'_>, EngineError> {
        self.alloc_reference(Cell::DateTime(value))
    }

    pub(crate) fn alloc_ptr_cell(&mut self, cell: Cell) -> Result<Reference<'_>, EngineError> {
        self.alloc_reference(cell)
    }

    pub(crate) fn alloc_ptr_uninitialized(
        &mut self,
        name: Symbol,
    ) -> Result<Reference<'_>, EngineError> {
        self.alloc_reference(Cell::Uninitialized(name))
    }

    pub(crate) fn alloc_ptr_frame(&mut self, frame: Frame) -> Result<Reference<'_>, EngineError> {
        self.alloc_reference(Cell::Frame(frame))
    }

    pub(crate) fn replace_frame(
        &mut self,
        pointer: &Pointer,
        frame: Frame,
    ) -> Result<(), EngineError> {
        self.pointer_as_frame(pointer)?;
        self.overwrite(pointer, Cell::Frame(frame))
    }

    pub(crate) fn alloc_ptr_root_frame_parent<'a>(
        &'a mut self,
    ) -> Result<Reference<'a>, EngineError> {
        self.alloc_ptr_u64(0)
    }

    pub(crate) fn alloc_ptr_tuple(
        &mut self,
        values: Vec<Pointer>,
    ) -> Result<Reference<'_>, EngineError> {
        self.alloc_reference(Cell::Tuple(values))
    }

    pub(crate) fn alloc_ptr_dict(
        &mut self,
        values: BTreeMap<Symbol, Pointer>,
    ) -> Result<Reference<'_>, EngineError> {
        self.alloc_reference(Cell::Dict(values))
    }

    pub(crate) fn alloc_ptr_adt(
        &mut self,
        name: Symbol,
        args: Vec<Pointer>,
    ) -> Result<Reference<'_>, EngineError> {
        self.alloc_reference(Cell::Adt(name, args))
    }

    pub(crate) fn alloc_ptr_empty(&mut self) -> Result<Reference<'_>, EngineError> {
        self.alloc_reference(Cell::Empty)
    }

    pub(crate) fn alloc_ptr_cons(
        &mut self,
        head: Pointer,
        tail: Pointer,
    ) -> Result<Reference<'_>, EngineError> {
        self.alloc_reference(Cell::Cons(head, tail))
    }

    pub(crate) fn alloc_ptr_data(
        &mut self,
        values: Vec<Pointer>,
    ) -> Result<Reference<'_>, EngineError> {
        self.alloc_reference(Cell::Data(values))
    }

    pub(crate) fn alloc_ptr_binary_data(
        &mut self,
        values: Vec<u8>,
    ) -> Result<Reference<'_>, EngineError> {
        self.alloc_reference(Cell::BinaryData(values))
    }

    pub(crate) fn alloc_ptr_closure(
        &mut self,
        env: Environment,
        param: Symbol,
        param_ty: Type,
        typ: Type,
        body: Arc<TypedExpr>,
    ) -> Result<Reference<'_>, EngineError> {
        self.alloc_reference(Cell::Closure(Closure {
            env,
            param,
            param_ty,
            typ,
            body,
        }))
    }

    pub(crate) fn alloc_ptr_native(
        &mut self,
        native_id: u64,
        name: Symbol,
        arity: usize,
        typ: Type,
        applied: Vec<Pointer>,
        applied_types: Vec<Type>,
    ) -> Result<Reference<'_>, EngineError> {
        self.alloc_reference(Cell::Native(NativeFn::from_parts(
            native_id,
            name,
            arity,
            typ,
            applied,
            applied_types,
        )))
    }

    pub(crate) fn alloc_ptr_overloaded(
        &mut self,
        name: Symbol,
        typ: Type,
        applied: Vec<Pointer>,
        applied_types: Vec<Type>,
    ) -> Result<Reference<'_>, EngineError> {
        self.alloc_reference(Cell::Overloaded(OverloadedFn::from_parts(
            name,
            typ,
            applied,
            applied_types,
        )))
    }

    pub(crate) fn alloc_ptr_list_slice(
        &mut self,
        start: usize,
        end: usize,
        elements: Pointer,
    ) -> Result<Reference<'_>, EngineError> {
        let len = list_slice_backing_len(self.get_cell_from_pointer(&elements)?)?;
        validate_list_slice_bounds(len, start, end)?;
        if start == end {
            return self.alloc_ptr_empty();
        }
        self.alloc_reference(Cell::ListSlice {
            start,
            end,
            elements,
        })
    }

    pub(crate) fn pointer_as_bool(&self, pointer: &Pointer) -> Result<bool, EngineError> {
        self.get_cell_from_pointer(pointer)?.cell_as_bool()
    }

    pub(crate) fn pointer_as_u8(&self, pointer: &Pointer) -> Result<u8, EngineError> {
        self.get_cell_from_pointer(pointer)?.cell_as_u8()
    }

    pub(crate) fn pointer_as_u16(&self, pointer: &Pointer) -> Result<u16, EngineError> {
        self.get_cell_from_pointer(pointer)?.cell_as_u16()
    }

    pub(crate) fn pointer_as_u32(&self, pointer: &Pointer) -> Result<u32, EngineError> {
        self.get_cell_from_pointer(pointer)?.cell_as_u32()
    }

    pub(crate) fn pointer_as_u64(&self, pointer: &Pointer) -> Result<u64, EngineError> {
        self.get_cell_from_pointer(pointer)?.cell_as_u64()
    }

    pub(crate) fn pointer_as_i8(&self, pointer: &Pointer) -> Result<i8, EngineError> {
        self.get_cell_from_pointer(pointer)?.cell_as_i8()
    }

    pub(crate) fn pointer_as_i16(&self, pointer: &Pointer) -> Result<i16, EngineError> {
        self.get_cell_from_pointer(pointer)?.cell_as_i16()
    }

    pub(crate) fn pointer_as_i32(&self, pointer: &Pointer) -> Result<i32, EngineError> {
        self.get_cell_from_pointer(pointer)?.cell_as_i32()
    }

    pub(crate) fn pointer_as_i64(&self, pointer: &Pointer) -> Result<i64, EngineError> {
        self.get_cell_from_pointer(pointer)?.cell_as_i64()
    }

    pub(crate) fn pointer_as_f32(&self, pointer: &Pointer) -> Result<f32, EngineError> {
        self.get_cell_from_pointer(pointer)?.cell_as_f32()
    }

    pub(crate) fn pointer_as_f64(&self, pointer: &Pointer) -> Result<f64, EngineError> {
        self.get_cell_from_pointer(pointer)?.cell_as_f64()
    }

    pub(crate) fn pointer_as_string(&self, pointer: &Pointer) -> Result<String, EngineError> {
        self.get_cell_from_pointer(pointer)?.cell_as_string()
    }

    pub(crate) fn pointer_as_uuid(&self, pointer: &Pointer) -> Result<Uuid, EngineError> {
        self.get_cell_from_pointer(pointer)?.cell_as_uuid()
    }

    pub(crate) fn pointer_as_datetime(
        &self,
        pointer: &Pointer,
    ) -> Result<DateTime<Utc>, EngineError> {
        self.get_cell_from_pointer(pointer)?.cell_as_datetime()
    }

    pub(crate) fn pointer_as_tuple(&self, pointer: &Pointer) -> Result<Vec<Pointer>, EngineError> {
        self.get_cell_from_pointer(pointer)?.cell_as_tuple()
    }

    pub(crate) fn pointer_as_dict(
        &self,
        pointer: &Pointer,
    ) -> Result<BTreeMap<Symbol, Pointer>, EngineError> {
        self.get_cell_from_pointer(pointer)?.cell_as_dict()
    }

    pub(crate) fn pointer_as_adt(
        &self,
        pointer: &Pointer,
    ) -> Result<(Symbol, Vec<Pointer>), EngineError> {
        self.get_cell_from_pointer(pointer)?.cell_as_adt()
    }

    pub(crate) fn pointer_as_frame(&self, pointer: &Pointer) -> Result<Frame, EngineError> {
        self.get_cell_from_pointer(pointer)?.cell_as_frame()
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
    pub(crate) state: Arc<Mutex<HeapState>>,
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
    Empty,
    Cons(Handle, Handle),
    ListSlice {
        start: usize,
        end: usize,
        elements: Handle,
    },
    Data(Vec<Handle>),
    BinaryData(Vec<u8>),
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
            Value::Empty | Value::Cons(..) | Value::ListSlice { .. } => "list",
            Value::Data(..) => "data",
            Value::BinaryData(..) => "binary_data",
            Value::Dict(..) => "dict",
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
        let _ = self
            .heap
            .with_locked(|heap| heap.unregister_roots(std::mem::take(&mut self.root_ids)));
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
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        Ok(state.collection_count() != self.collection_count)
    }

    pub(crate) fn get(&self, index: usize) -> Result<Pointer, EngineError> {
        let root_id = *self
            .root_ids
            .get(index)
            .ok_or_else(|| EngineError::Internal("temporary root index out of bounds".into()))?;
        self.heap.with_locked(|heap| heap.resolve_root(root_id))
    }
}

impl Drop for HandleRoot {
    fn drop(&mut self) {
        let _ = self
            .heap
            .with_locked(|heap| heap.unregister_root(self.root_id));
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
        self.heap().with_locked(|heap| heap.type_name(&pointer))
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
        let heap = self
            .root
            .heap
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        let pointer = heap.resolve_root(self.root.root_id)?;
        pointer_display_with(&heap, &pointer, opts)
    }

    pub fn debug(&self) -> Result<String, EngineError> {
        let heap = self
            .root
            .heap
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        let pointer = heap.resolve_root(self.root.root_id)?;
        pointer_debug(&heap, &pointer)
    }

    pub fn value_eq(&self, other: &Handle) -> Result<bool, EngineError> {
        let self_pointer = self.pointer()?;
        let pointer = other.pointer_for_heap(self.heap())?;
        self.heap()
            .with_locked(|heap| pointer_eq(heap, &self_pointer, &pointer))
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
        self.root
            .heap
            .with_locked(|heap| heap.resolve_root(self.root.root_id))
    }

    pub(crate) fn pointer_for_heap(&self, heap: &Heap) -> Result<Pointer, EngineError> {
        let pointer = self.pointer()?;
        if pointer.heap_id != heap.id {
            return Err(wrong_heap_pointer(
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
    Empty,
    Cons(Pointer, Pointer),
    ListSlice {
        start: usize,
        end: usize,
        elements: Pointer,
    },
    Data(Vec<Pointer>),
    BinaryData(Vec<u8>),
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
            Cell::Empty => Self::Empty,
            Cell::Cons(head, tail) => Self::Cons(*head, *tail),
            Cell::ListSlice {
                start,
                end,
                elements,
            } => Self::ListSlice {
                start: *start,
                end: *end,
                elements: *elements,
            },
            Cell::Data(values) => Self::Data(values.clone()),
            Cell::BinaryData(values) => Self::BinaryData(values.clone()),
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

impl Heap {
    /// Create a heap for a lexical scope.
    pub fn scoped<R>(f: impl FnOnce(&Heap) -> R) -> R {
        let heap = Heap::new();
        f(&heap)
    }

    pub fn new() -> Self {
        let state = HeapState::new();
        let id = state.id;
        Self {
            id,
            state: Arc::new(Mutex::new(state)),
        }
    }

    pub(crate) fn with_locked<R>(
        &self,
        f: impl FnOnce(&mut HeapState) -> Result<R, EngineError>,
    ) -> Result<R, EngineError> {
        // Keep all heap reads inside this access object while the lock is held.
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        f(&mut state)
    }

    #[cfg(test)]
    pub(crate) fn with_locked_ok<R>(
        &self,
        f: impl FnOnce(&mut HeapState) -> R,
    ) -> Result<R, EngineError> {
        // Keep all heap reads inside this access object while the lock is held.
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        Ok(f(&mut state))
    }

    pub(crate) fn temp_roots(&self, pointers: Vec<Pointer>) -> Result<TempRoots, EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        let mut root_ids = Vec::with_capacity(pointers.len());
        for pointer in pointers {
            root_ids.push(state.register_root(pointer)?);
        }
        let collection_count = state.collection_count();
        Ok(TempRoots {
            heap: self.clone(),
            root_ids,
            collection_count,
        })
    }

    pub(crate) fn handle(&self, pointer: Pointer) -> Result<Handle, EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        state.get_cell_from_pointer(&pointer)?;
        let root_id = state.register_root(pointer)?;
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
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        state.set_collect_on_every_alloc(enabled);
        Ok(())
    }

    pub fn alloc_bool(&self, value: bool) -> Result<Handle, EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        state.alloc_ptr_bool(value)?.into_handle(self)
    }

    pub fn alloc_u8(&self, value: u8) -> Result<Handle, EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        state.alloc_ptr_u8(value)?.into_handle(self)
    }

    pub fn alloc_u16(&self, value: u16) -> Result<Handle, EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        state.alloc_ptr_u16(value)?.into_handle(self)
    }

    pub fn alloc_u32(&self, value: u32) -> Result<Handle, EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        state.alloc_ptr_u32(value)?.into_handle(self)
    }

    pub fn alloc_u64(&self, value: u64) -> Result<Handle, EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        state.alloc_ptr_u64(value)?.into_handle(self)
    }

    pub fn alloc_i8(&self, value: i8) -> Result<Handle, EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        state.alloc_ptr_i8(value)?.into_handle(self)
    }

    pub fn alloc_i16(&self, value: i16) -> Result<Handle, EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        state.alloc_ptr_i16(value)?.into_handle(self)
    }

    pub fn alloc_i32(&self, value: i32) -> Result<Handle, EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        state.alloc_ptr_i32(value)?.into_handle(self)
    }

    pub fn alloc_i64(&self, value: i64) -> Result<Handle, EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        state.alloc_ptr_i64(value)?.into_handle(self)
    }

    pub fn alloc_f32(&self, value: f32) -> Result<Handle, EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        state.alloc_ptr_f32(value)?.into_handle(self)
    }

    pub fn alloc_f64(&self, value: f64) -> Result<Handle, EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        state.alloc_ptr_f64(value)?.into_handle(self)
    }

    pub fn alloc_string(&self, value: String) -> Result<Handle, EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        state.alloc_ptr_string(value)?.into_handle(self)
    }

    pub fn alloc_uuid(&self, value: Uuid) -> Result<Handle, EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        state.alloc_ptr_uuid(value)?.into_handle(self)
    }

    pub fn alloc_datetime(&self, value: DateTime<Utc>) -> Result<Handle, EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        state.alloc_ptr_datetime(value)?.into_handle(self)
    }

    pub fn alloc_tuple(&self, values: Vec<Handle>) -> Result<Handle, EngineError> {
        let pointers = self.pointers_from_handles(values)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        state.alloc_ptr_tuple(pointers)?.into_handle(self)
    }

    pub fn alloc_list(&self, values: Vec<Handle>) -> Result<Handle, EngineError> {
        let pointers = self.pointers_from_handles(values)?;
        handle_from_pointer(self, self.alloc_ptr_list(pointers)?)
    }

    pub fn alloc_empty(&self) -> Result<Handle, EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        state.alloc_ptr_empty()?.into_handle(self)
    }

    pub fn alloc_cons(&self, head: Handle, tail: Handle) -> Result<Handle, EngineError> {
        let head = head.pointer_for_heap(self)?;
        let tail = tail.pointer_for_heap(self)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        state.alloc_ptr_cons(head, tail)?.into_handle(self)
    }

    pub fn alloc_data(&self, values: Vec<Handle>) -> Result<Handle, EngineError> {
        let pointers = self.pointers_from_handles(values)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        state.alloc_ptr_data(pointers)?.into_handle(self)
    }

    pub fn alloc_binary_data(&self, values: Vec<u8>) -> Result<Handle, EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        state.alloc_ptr_binary_data(values)?.into_handle(self)
    }

    pub fn alloc_list_slice(
        &self,
        start: usize,
        end: usize,
        elements: Handle,
    ) -> Result<Handle, EngineError> {
        let elements = elements.pointer_for_heap(self)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        state
            .alloc_ptr_list_slice(start, end, elements)?
            .into_handle(self)
    }

    pub fn alloc_dict(&self, values: BTreeMap<Symbol, Handle>) -> Result<Handle, EngineError> {
        let mut pointers = BTreeMap::new();
        for (name, handle) in values {
            pointers.insert(name, handle.pointer_for_heap(self)?);
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        state.alloc_ptr_dict(pointers)?.into_handle(self)
    }

    pub fn alloc_adt(&self, name: Symbol, args: Vec<Handle>) -> Result<Handle, EngineError> {
        let pointers = self.pointers_from_handles(args)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        state.alloc_ptr_adt(name, pointers)?.into_handle(self)
    }

    fn pointers_from_handles(&self, values: Vec<Handle>) -> Result<Vec<Pointer>, EngineError> {
        values
            .iter()
            .map(|handle| handle.pointer_for_heap(self))
            .collect()
    }

    pub(crate) fn clone_cell(&self, pointer: &Pointer) -> Result<Cell, EngineError> {
        self.with_locked(|heap| Ok(heap.get_cell_from_pointer(pointer)?.clone()))
    }

    pub(crate) fn view(&self, pointer: &Pointer) -> Result<Value, EngineError> {
        let seed = self
            .with_locked(|heap| Ok(ValueSeed::from_cell(heap.get_cell_from_pointer(pointer)?)))?;
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
            ValueSeed::Empty => Value::Empty,
            ValueSeed::Cons(head, tail) => Value::Cons(self.handle(head)?, self.handle(tail)?),
            ValueSeed::ListSlice {
                start,
                end,
                elements,
            } => Value::ListSlice {
                start,
                end,
                elements: self.handle(elements)?,
            },
            ValueSeed::Data(values) => Value::Data(self.handles_from_pointers(&values)?),
            ValueSeed::BinaryData(values) => Value::BinaryData(values),
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

    pub(crate) fn pointer_as_list(&self, pointer: &Pointer) -> Result<Vec<Pointer>, EngineError> {
        let elements = self.with_locked(|heap| list_elements_from_pointer(heap, *pointer))?;
        materialize_list_elements(self, elements)
    }

    pub(crate) fn list_len(&self, pointer: &Pointer) -> Result<usize, EngineError> {
        self.with_locked(|heap| list_len_from_pointer(heap, *pointer))
    }

    pub(crate) fn list_items(&self, pointer: Pointer) -> Result<ListItems, EngineError> {
        match self.with_locked(|heap| list_items_from_pointer(heap, pointer))? {
            ListItemsSeed::Ready(items) => Ok(items),
            ListItemsSeed::Elements(elements) => {
                if elements
                    .iter()
                    .all(|element| matches!(element, ListElement::Pointer(_)))
                {
                    return list_elements_to_pointer_vec(elements).map(ListItems::Pointers);
                }
                materialize_list_elements(self, elements).map(ListItems::Pointers)
            }
        }
    }

    pub(crate) fn list_head_tail(
        &self,
        pointer: &Pointer,
    ) -> Result<Option<(Pointer, Pointer)>, EngineError> {
        let Some((head, tail_index, elements)) = self.with_locked(|heap| {
            let cell = heap.get_cell_from_pointer(pointer)?;
            match cell {
                Cell::Empty => Ok(None),
                Cell::Cons(head, tail) => Ok(Some((ListElement::Pointer(*head), None, *tail))),
                Cell::ListSlice {
                    start,
                    end,
                    elements,
                } => {
                    let Some(head) = list_slice_head_element(heap, elements, *start, *end)? else {
                        return Ok(None);
                    };
                    Ok(Some((head, Some((start + 1, *end)), *elements)))
                }
                _ => Err(EngineError::NativeType {
                    expected: "list".into(),
                    got: cell.cell_type_name().into(),
                }),
            }
        })?
        else {
            return Ok(None);
        };
        let head = match head {
            ListElement::Pointer(pointer) => pointer,
            ListElement::U8(value) => {
                let elements_root = self.temp_roots(vec![elements])?;
                let head = self.with_locked(|heap| Ok(heap.alloc_ptr_u8(value)?.into_pointer()))?;
                let elements = elements_root.get(0)?;
                let tail = match tail_index {
                    Some((start, end)) => {
                        let roots = self.temp_roots(vec![head, elements])?;
                        let elements_ptr = roots.get(1)?; // must call get before with_locked
                        let tail = self.with_locked(|heap| {
                            Ok(heap
                                .alloc_ptr_list_slice(start, end, elements_ptr)?
                                .into_pointer())
                        })?;
                        let head = roots.get(0)?;
                        return Ok(Some((head, tail)));
                    }
                    None => elements,
                };
                return Ok(Some((head, tail)));
            }
        };
        let tail = match tail_index {
            Some((start, end)) => {
                // Creating the tail slice can trigger copying GC. The head was
                // read from the backing data cell before that allocation, so
                // it must be temporarily rooted or the returned pointer may
                // refer to the pre-collection location.
                let head_root = self.temp_roots(vec![head])?;
                let tail = self.with_locked(|heap| {
                    Ok(heap
                        .alloc_ptr_list_slice(start, end, elements)?
                        .into_pointer())
                })?;
                let head = head_root.get(0)?;
                return Ok(Some((head, tail)));
            }
            None => elements,
        };
        Ok(Some((head, tail)))
    }

    pub(crate) fn alloc_ptr_list(&self, values: Vec<Pointer>) -> Result<Pointer, EngineError> {
        if values.is_empty() {
            return self.with_locked(|heap| Ok(heap.alloc_ptr_empty()?.into_pointer()));
        }
        let roots = self.temp_roots(values)?;
        let len = roots.len();
        let values = (0..roots.len())
            .map(|index| roots.get(index))
            .collect::<Result<Vec<_>, _>>()?;
        let data = self.with_locked(|heap| Ok(heap.alloc_ptr_data(values)?.into_pointer()))?;
        let data = self.temp_roots(vec![data])?;
        let elements = data.get(0)?; // must call get before with_locked
        self.with_locked(|heap| Ok(heap.alloc_ptr_list_slice(0, len, elements)?.into_pointer()))
    }

    pub(crate) fn alloc_ptr_binary_list(&self, values: Vec<u8>) -> Result<Pointer, EngineError> {
        if values.is_empty() {
            return self.with_locked(|heap| Ok(heap.alloc_ptr_empty()?.into_pointer()));
        }
        let len = values.len();
        let data =
            self.with_locked(|heap| Ok(heap.alloc_ptr_binary_data(values)?.into_pointer()))?;
        let data = self.temp_roots(vec![data])?;
        let elements = data.get(0)?; // must call get before with_locked
        self.with_locked(|heap| Ok(heap.alloc_ptr_list_slice(0, len, elements)?.into_pointer()))
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

pub(crate) struct Reference<'a> {
    heap: &'a mut HeapState,
    index: u32,
    generation: u64,
}

impl<'a> Reference<'a> {
    pub(crate) fn into_pointer(self) -> Pointer {
        Pointer {
            heap_id: self.heap.id,
            index: self.index,
            generation: self.generation,
        }
    }

    pub(crate) fn into_handle(self, heap: &Heap) -> Result<Handle, EngineError> {
        let pointer = Pointer {
            heap_id: self.heap.id,
            index: self.index,
            generation: self.generation,
        };
        self.heap.get_cell_from_pointer(&pointer)?;
        let root_id = self.heap.register_root(pointer)?;
        Ok(Handle {
            root: Arc::new(HandleRoot {
                heap: heap.clone(),
                root_id,
            }),
        })
    }
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
    Empty,
    Cons(Pointer, Pointer),
    ListSlice {
        start: usize,
        end: usize,
        elements: Pointer,
    },
    Data(Vec<Pointer>),
    BinaryData(Vec<u8>),
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
            Cell::Empty | Cell::Cons(..) | Cell::ListSlice { .. } => "list",
            Cell::Data(..) => "data",
            Cell::BinaryData(..) => "binary_data",
            Cell::Dict(..) => "dict",
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

    pub(crate) fn cell_as_data(&self) -> Result<Vec<Pointer>, EngineError> {
        match self {
            Cell::Data(v) => Ok(v.clone()),
            _ => Err(self.cell_type_error("data")),
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

impl Collection for Cell {
    fn map_pointers<E>(
        &mut self,
        map: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E> {
        match self {
            Cell::Tuple(values) | Cell::Data(values) | Cell::Adt(_, values) => {
                for pointer in values {
                    *pointer = map(*pointer)?;
                }
                Ok(())
            }
            Cell::Cons(head, tail) => {
                *head = map(*head)?;
                *tail = map(*tail)?;
                Ok(())
            }
            Cell::ListSlice { elements, .. } => {
                *elements = map(*elements)?;
                Ok(())
            }
            Cell::Dict(values) => {
                for pointer in values.values_mut() {
                    *pointer = map(*pointer)?;
                }
                Ok(())
            }
            Cell::Frame(frame) => frame.map_pointers(map),
            Cell::Closure(closure) => closure.env.map_pointers(map),
            Cell::Native(native) => native.map_pointers(map),
            Cell::Overloaded(overloaded) => overloaded.map_pointers(map),
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
            | Cell::BinaryData(_)
            | Cell::Empty
            | Cell::Uninitialized(_) => Ok(()),
        }
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
    heap: &HeapState,
    pointer: &Pointer,
    active: &mut HashSet<PointerKey>,
) -> Result<String, EngineError> {
    let key = pointer_key(pointer);
    if !active.insert(key) {
        return Ok(format!("<cycle:{}:{}>", pointer.index, pointer.generation));
    }
    let cell = heap.get_cell_from_pointer(pointer)?;
    let out = cell_debug_inner(heap, cell, active);
    active.remove(&key);
    out
}

fn pointer_display_inner(
    heap: &HeapState,
    pointer: &Pointer,
    active: &mut HashSet<PointerKey>,
    opts: ValueDisplayOptions,
) -> Result<String, EngineError> {
    let key = pointer_key(pointer);
    if !active.insert(key) {
        return Ok(format!("<cycle:{}:{}>", pointer.index, pointer.generation));
    }
    let cell = heap.get_cell_from_pointer(pointer)?;
    let out = cell_display_inner(heap, cell, active, opts);
    active.remove(&key);
    out
}

fn env_debug_inner(
    heap: &HeapState,
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
    heap: &HeapState,
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
    heap: &HeapState,
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
        Cell::Empty | Cell::Cons(..) | Cell::ListSlice { .. } => {
            format_list_debug(heap, cell, active)?
        }
        Cell::Data(values) => {
            let items = values
                .iter()
                .map(|pointer| pointer_debug_inner(heap, pointer, active))
                .collect::<Result<Vec<_>, _>>()?;
            format!("<data {}>", items.join(", "))
        }
        Cell::BinaryData(values) => format!("<binary data {} bytes>", values.len()),
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
            let mut rendered = vec![name.to_string()];
            for pointer in args {
                rendered.push(pointer_debug_inner(heap, pointer, active)?);
            }
            rendered.join(" ")
        }
        Cell::Uninitialized(name) => format!("<uninitialized:{name}>"),
        Cell::Frame(frame) => format!("<frame:{frame:?}>"),
        Cell::Closure(closure) => closure_debug_inner(heap, closure, active)?,
        Cell::Native(native) => format!("<native:{}>", native.name()),
        Cell::Overloaded(over) => format!("<overloaded:{}>", over.name()),
    })
}

fn cell_display_inner(
    heap: &HeapState,
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
        Cell::Empty | Cell::Cons(..) | Cell::ListSlice { .. } => {
            format_list_display(heap, cell, active, opts)?
        }
        Cell::Data(values) => {
            let items = values
                .iter()
                .map(|pointer| pointer_display_inner(heap, pointer, active, opts))
                .collect::<Result<Vec<_>, _>>()?;
            format!("<data {}>", items.join(", "))
        }
        Cell::BinaryData(values) => format!("<binary data {} bytes>", values.len()),
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
            let mut rendered = vec![maybe_strip_snippet_qualifier(name.as_ref(), opts)];
            for pointer in args {
                rendered.push(pointer_display_inner(heap, pointer, active, opts)?);
            }
            rendered.join(" ")
        }
        Cell::Uninitialized(name) => format!("<uninitialized:{name}>"),
        Cell::Frame(frame) => format!("<frame:{frame:?}>"),
        Cell::Closure(..) => "<closure>".to_string(),
        Cell::Native(native) => format!("<native:{}>", native.name()),
        Cell::Overloaded(over) => format!("<overloaded:{}>", over.name()),
    })
}

pub(crate) fn pointer_debug(heap: &HeapState, pointer: &Pointer) -> Result<String, EngineError> {
    let mut active = HashSet::new();
    pointer_debug_inner(heap, pointer, &mut active)
}

pub(crate) fn pointer_display_with(
    heap: &HeapState,
    pointer: &Pointer,
    opts: ValueDisplayOptions,
) -> Result<String, EngineError> {
    let mut active = HashSet::new();
    pointer_display_inner(heap, pointer, &mut active, opts)
}

fn pointer_eq_inner(
    heap: &HeapState,
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
    let lhs_cell = heap.get_cell_from_pointer(lhs)?;
    let rhs_cell = heap.get_cell_from_pointer(rhs)?;
    cell_eq_inner(heap, lhs_cell, rhs_cell, seen)
}

fn env_eq_inner(
    heap: &HeapState,
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
    heap: &HeapState,
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
    heap: &HeapState,
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
        (Cell::Tuple(lhs), Cell::Tuple(rhs)) | (Cell::Data(lhs), Cell::Data(rhs)) => {
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
        (Cell::BinaryData(lhs), Cell::BinaryData(rhs)) => Ok(lhs == rhs),
        (
            Cell::Empty | Cell::Cons(..) | Cell::ListSlice { .. },
            Cell::Empty | Cell::Cons(..) | Cell::ListSlice { .. },
        ) => list_cells_eq_inner(heap, lhs, rhs, seen),
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

pub(crate) fn pointer_eq(
    heap: &HeapState,
    lhs: &Pointer,
    rhs: &Pointer,
) -> Result<bool, EngineError> {
    let mut seen = HashSet::new();
    pointer_eq_inner(heap, lhs, rhs, &mut seen)
}

fn usize_to_i32_saturating(index: usize) -> i32 {
    i32::try_from(index).unwrap_or(i32::MAX)
}

fn validate_list_slice_bounds(
    data_len: usize,
    start: usize,
    end: usize,
) -> Result<(), EngineError> {
    if start > end {
        return Err(EngineError::Custom(format!(
            "invalid list slice range: start {start} is greater than end {end}"
        )));
    }
    if end > data_len {
        return Err(EngineError::IndexOutOfBounds {
            name: Symbol::intern("ListSlice"),
            index: usize_to_i32_saturating(end),
            len: data_len,
        });
    }
    Ok(())
}

fn list_slice_backing_len(cell: &Cell) -> Result<usize, EngineError> {
    match cell {
        Cell::Data(values) => Ok(values.len()),
        Cell::BinaryData(values) => Ok(values.len()),
        _ => Err(EngineError::NativeType {
            expected: "list slice backing data".into(),
            got: cell.cell_type_name().into(),
        }),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ListElement {
    Pointer(Pointer),
    U8(u8),
}

fn append_list_slice_elements(
    heap: &HeapState,
    elements: &Pointer,
    start: usize,
    end: usize,
    out: &mut Vec<ListElement>,
) -> Result<(), EngineError> {
    match heap.get_cell_from_pointer(elements)? {
        Cell::Data(values) => {
            validate_list_slice_bounds(values.len(), start, end)?;
            out.extend(values[start..end].iter().copied().map(ListElement::Pointer));
            Ok(())
        }
        Cell::BinaryData(values) => {
            validate_list_slice_bounds(values.len(), start, end)?;
            out.extend(values[start..end].iter().copied().map(ListElement::U8));
            Ok(())
        }
        cell => Err(EngineError::NativeType {
            expected: "list slice backing data".into(),
            got: cell.cell_type_name().into(),
        }),
    }
}

fn list_slice_head_element(
    heap: &HeapState,
    elements: &Pointer,
    start: usize,
    end: usize,
) -> Result<Option<ListElement>, EngineError> {
    if start >= end {
        return Ok(None);
    }
    match heap.get_cell_from_pointer(elements)? {
        Cell::Data(values) => {
            validate_list_slice_bounds(values.len(), start, end)?;
            Ok(values.get(start).copied().map(ListElement::Pointer))
        }
        Cell::BinaryData(values) => {
            validate_list_slice_bounds(values.len(), start, end)?;
            Ok(values.get(start).copied().map(ListElement::U8))
        }
        cell => Err(EngineError::NativeType {
            expected: "list slice backing data".into(),
            got: cell.cell_type_name().into(),
        }),
    }
}

fn list_elements_from_cell(heap: &HeapState, cell: &Cell) -> Result<Vec<ListElement>, EngineError> {
    let mut out = Vec::new();
    let mut cursor = cell;
    loop {
        match cursor {
            Cell::Empty => return Ok(out),
            Cell::Cons(head, tail) => {
                out.push(ListElement::Pointer(*head));
                cursor = heap.get_cell_from_pointer(tail)?;
            }
            Cell::ListSlice {
                start,
                end,
                elements,
            } => {
                append_list_slice_elements(heap, elements, *start, *end, &mut out)?;
                return Ok(out);
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

fn list_elements_from_pointer(
    heap: &HeapState,
    pointer: Pointer,
) -> Result<Vec<ListElement>, EngineError> {
    let cell = heap.get_cell_from_pointer(&pointer)?;
    list_elements_from_cell(heap, cell)
}

fn list_len_from_pointer(heap: &HeapState, pointer: Pointer) -> Result<usize, EngineError> {
    let mut len = 0usize;
    let mut cursor = heap.get_cell_from_pointer(&pointer)?;
    loop {
        match cursor {
            Cell::Empty => return Ok(len),
            Cell::Cons(_, tail) => {
                len = len
                    .checked_add(1)
                    .ok_or_else(|| EngineError::Internal("list length overflow".into()))?;
                cursor = heap.get_cell_from_pointer(tail)?;
            }
            Cell::ListSlice {
                start,
                end,
                elements,
            } => {
                let backing_len = list_slice_backing_len(heap.get_cell_from_pointer(elements)?)?;
                validate_list_slice_bounds(backing_len, *start, *end)?;
                return len
                    .checked_add(end - start)
                    .ok_or_else(|| EngineError::Internal("list length overflow".into()));
            }
            cell => {
                return Err(EngineError::NativeType {
                    expected: "list".into(),
                    got: cell.cell_type_name().into(),
                });
            }
        }
    }
}

enum MaterializedListElement {
    RootedPointer(usize),
    RootedByte(usize),
}

fn materialize_list_elements(
    heap: &Heap,
    elements: Vec<ListElement>,
) -> Result<Vec<Pointer>, EngineError> {
    let pointer_values = elements
        .iter()
        .filter_map(|element| match element {
            ListElement::Pointer(pointer) => Some(*pointer),
            ListElement::U8(_) => None,
        })
        .collect::<Vec<_>>();
    let pointer_roots = heap.temp_roots(pointer_values)?;
    let mut next_pointer_root = 0;
    let mut byte_roots = Vec::new();
    let mut materialized = Vec::with_capacity(elements.len());

    for element in elements {
        match element {
            ListElement::Pointer(_) => {
                materialized.push(MaterializedListElement::RootedPointer(next_pointer_root));
                next_pointer_root += 1;
            }
            ListElement::U8(value) => {
                let pointer =
                    heap.with_locked(|heap| Ok(heap.alloc_ptr_u8(value)?.into_pointer()))?;
                byte_roots.push(heap.temp_roots(vec![pointer])?);
                materialized.push(MaterializedListElement::RootedByte(byte_roots.len() - 1));
            }
        }
    }

    materialized
        .into_iter()
        .map(|element| match element {
            MaterializedListElement::RootedPointer(index) => pointer_roots.get(index),
            MaterializedListElement::RootedByte(index) => byte_roots[index].get(0),
        })
        .collect()
}

fn list_elements_to_pointer_vec(elements: Vec<ListElement>) -> Result<Vec<Pointer>, EngineError> {
    elements
        .into_iter()
        .map(|element| match element {
            ListElement::Pointer(pointer) => Ok(pointer),
            ListElement::U8(_) => Err(EngineError::NativeType {
                expected: "pointer-backed list".into(),
                got: "binary-backed list".into(),
            }),
        })
        .collect()
}

pub(crate) fn list_to_vec(heap: &HeapState, cell: &Cell) -> Result<Vec<Pointer>, EngineError> {
    list_elements_to_pointer_vec(list_elements_from_cell(heap, cell)?)
}

fn collect_list_u8(heap: &HeapState, pointer: &Pointer) -> Result<Vec<u8>, EngineError> {
    let elements = list_elements_from_pointer(heap, *pointer)?;
    let mut out = Vec::with_capacity(elements.len());
    for element in elements {
        match element {
            ListElement::Pointer(pointer) => out.push(heap.pointer_as_u8(&pointer)?),
            ListElement::U8(value) => out.push(value),
        }
    }
    Ok(out)
}

fn format_list_debug(
    heap: &HeapState,
    cell: &Cell,
    active: &mut HashSet<PointerKey>,
) -> Result<String, EngineError> {
    let items = list_elements_from_cell(heap, cell)?
        .into_iter()
        .map(|element| match element {
            ListElement::Pointer(pointer) => pointer_debug_inner(heap, &pointer, active),
            ListElement::U8(value) => Ok(format!("{value}u8")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(format!("[{}]", items.join(", ")))
}

fn format_list_display(
    heap: &HeapState,
    cell: &Cell,
    active: &mut HashSet<PointerKey>,
    opts: ValueDisplayOptions,
) -> Result<String, EngineError> {
    let items = list_elements_from_cell(heap, cell)?
        .into_iter()
        .map(|element| match element {
            ListElement::Pointer(pointer) => pointer_display_inner(heap, &pointer, active, opts),
            ListElement::U8(value) => {
                if opts.include_numeric_suffixes {
                    Ok(format!("{value}u8"))
                } else {
                    Ok(value.to_string())
                }
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(format!("[{}]", items.join(", ")))
}

fn list_element_eq_inner(
    heap: &HeapState,
    lhs: ListElement,
    rhs: ListElement,
    seen: &mut HashSet<PointerPairKey>,
) -> Result<bool, EngineError> {
    match (lhs, rhs) {
        (ListElement::Pointer(lhs), ListElement::Pointer(rhs)) => {
            pointer_eq_inner(heap, &lhs, &rhs, seen)
        }
        (ListElement::U8(lhs), ListElement::U8(rhs)) => Ok(lhs == rhs),
        (ListElement::U8(lhs), ListElement::Pointer(rhs))
        | (ListElement::Pointer(rhs), ListElement::U8(lhs)) => {
            match heap.get_cell_from_pointer(&rhs)? {
                Cell::U8(rhs) => Ok(lhs == *rhs),
                _ => Ok(false),
            }
        }
    }
}

fn list_cells_eq_inner(
    heap: &HeapState,
    lhs: &Cell,
    rhs: &Cell,
    seen: &mut HashSet<PointerPairKey>,
) -> Result<bool, EngineError> {
    let lhs = list_elements_from_cell(heap, lhs)?;
    let rhs = list_elements_from_cell(heap, rhs)?;
    if lhs.len() != rhs.len() {
        return Ok(false);
    }
    for (lhs, rhs) in lhs.into_iter().zip(rhs) {
        if !list_element_eq_inner(heap, lhs, rhs, seen)? {
            return Ok(false);
        }
    }
    Ok(true)
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ListItems {
    Slice {
        elements: Pointer,
        start: usize,
        end: usize,
    },
    BinarySlice {
        elements: Pointer,
        start: usize,
        end: usize,
        bytes: Arc<[u8]>,
    },
    Pointers(Vec<Pointer>),
}

impl ListItems {
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Slice { start, end, .. } => end - start,
            Self::BinarySlice { start, end, .. } => end - start,
            Self::Pointers(values) => values.len(),
        }
    }

    pub(crate) fn get(&self, heap: &mut HeapState, index: usize) -> Result<Pointer, EngineError> {
        match self {
            Self::Slice {
                elements,
                start,
                end,
            } => {
                let len = end - start;
                if index >= len {
                    return Err(EngineError::Internal(
                        "list item index out of bounds".into(),
                    ));
                }
                let backing_index = start.checked_add(index).ok_or_else(|| {
                    EngineError::Internal("list slice backing index overflow".into())
                })?;
                if backing_index >= *end {
                    return Err(EngineError::Internal(
                        "list slice backing index out of bounds".into(),
                    ));
                }
                let values = heap.get_cell_from_pointer(elements)?.cell_as_data()?;
                values.get(backing_index).copied().ok_or_else(|| {
                    EngineError::Internal("list slice backing index out of bounds".into())
                })
            }
            Self::BinarySlice {
                start, end, bytes, ..
            } => {
                let len = end - start;
                if index >= len {
                    return Err(EngineError::Internal(
                        "list item index out of bounds".into(),
                    ));
                }
                let value = bytes.get(index).copied().ok_or_else(|| {
                    EngineError::Internal("binary list slice index out of bounds".into())
                })?;
                Ok(heap.alloc_ptr_u8(value)?.into_pointer())
            }
            Self::Pointers(values) => values
                .get(index)
                .copied()
                .ok_or_else(|| EngineError::Internal("list item index out of bounds".into())),
        }
    }
}

enum ListItemsSeed {
    Ready(ListItems),
    Elements(Vec<ListElement>),
}

fn list_items_from_pointer(
    heap: &HeapState,
    pointer: Pointer,
) -> Result<ListItemsSeed, EngineError> {
    let cell = heap.get_cell_from_pointer(&pointer)?;
    match cell {
        Cell::Empty => Ok(ListItemsSeed::Ready(ListItems::Pointers(Vec::new()))),
        Cell::ListSlice {
            start,
            end,
            elements,
        } => match heap.get_cell_from_pointer(elements)? {
            Cell::Data(values) => {
                validate_list_slice_bounds(values.len(), *start, *end)?;
                Ok(ListItemsSeed::Ready(ListItems::Slice {
                    elements: *elements,
                    start: *start,
                    end: *end,
                }))
            }
            Cell::BinaryData(values) => {
                validate_list_slice_bounds(values.len(), *start, *end)?;
                Ok(ListItemsSeed::Ready(ListItems::BinarySlice {
                    elements: *elements,
                    start: *start,
                    end: *end,
                    bytes: Arc::from(&values[*start..*end]),
                }))
            }
            cell => Err(EngineError::NativeType {
                expected: "list slice backing data".into(),
                got: cell.cell_type_name().into(),
            }),
        },
        Cell::Cons(..) => Ok(ListItemsSeed::Elements(list_elements_from_cell(
            heap, cell,
        )?)),
        _ => Err(EngineError::NativeType {
            expected: "list".into(),
            got: cell.cell_type_name().into(),
        }),
    }
}

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

impl Collection for ListItems {
    fn map_pointers<E>(
        &mut self,
        map: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E> {
        match self {
            Self::Slice { elements, .. } | Self::BinarySlice { elements, .. } => {
                *elements = map(*elements)?;
                Ok(())
            }
            Self::Pointers(values) => {
                for pointer in values {
                    *pointer = map(*pointer)?;
                }
                Ok(())
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

fn handle_from_pointer(heap: &Heap, pointer: Pointer) -> Result<Handle, EngineError> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_roots_value_until_last_clone_drops() {
        let heap = Heap::new();
        let pointer = heap
            .with_locked(|heap| Ok(heap.alloc_ptr_i32(42)?.into_pointer()))
            .expect("alloc_i32 should succeed");
        assert_eq!(
            heap.with_locked(|heap| Ok(heap.root_count()))
                .expect("root count"),
            0
        );

        let handle = heap.handle(pointer).expect("handle should root pointer");
        assert_eq!(handle.type_name().expect("handle type name"), "i32");
        assert_eq!(
            heap.with_locked(|heap| Ok(heap.root_count()))
                .expect("root count"),
            1
        );

        let clone = handle.clone();
        assert_eq!(
            heap.with_locked(|heap| Ok(heap.root_count()))
                .expect("root count"),
            1
        );

        drop(handle);
        assert_eq!(
            heap.with_locked(|heap| Ok(heap.root_count()))
                .expect("root count"),
            1
        );

        drop(clone);
        assert_eq!(
            heap.with_locked(|heap| Ok(heap.root_count()))
                .expect("root count"),
            0
        );
    }

    #[test]
    fn handle_root_ids_are_reused_with_generation_bump() {
        let heap = Heap::new();
        let first_pointer = heap
            .with_locked(|heap| Ok(heap.alloc_ptr_i32(1)?.into_pointer()))
            .expect("alloc_i32 should succeed");
        let first = heap
            .handle(first_pointer)
            .expect("handle should root pointer");
        let first_root_id = first.root.root_id;
        drop(first);

        let second_pointer = heap
            .with_locked(|heap| Ok(heap.alloc_ptr_i32(2)?.into_pointer()))
            .expect("alloc_i32 should succeed");
        let second = heap
            .handle(second_pointer)
            .expect("handle should reuse root slot");
        let second_root_id = second.root.root_id;

        assert_eq!(second_root_id.index, first_root_id.index);
        assert_eq!(second_root_id.generation, first_root_id.generation + 1);
        assert_eq!(
            heap.with_locked(|heap| Ok(heap.root_count()))
                .expect("root count"),
            1
        );
    }

    #[test]
    fn handle_resolves_pointer_from_root_slot() {
        let heap = Heap::new();
        let first_pointer = heap
            .with_locked(|heap| Ok(heap.alloc_ptr_i32(1)?.into_pointer()))
            .expect("alloc_i32 should succeed");
        let second_pointer = heap
            .with_locked(|heap| Ok(heap.alloc_ptr_i32(2)?.into_pointer()))
            .expect("alloc_i32 should succeed");
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
        let pointer = heap_a
            .with_locked(|heap| Ok(heap.alloc_ptr_i32(42)?.into_pointer()))
            .expect("alloc_i32 should succeed");

        let err = match heap_b.handle(pointer) {
            Ok(_) => panic!("cross-heap pointer should not be rootable"),
            Err(err) => err,
        };
        let EngineError::Internal(msg) = err else {
            panic!("expected internal error for cross-heap pointer");
        };
        assert!(msg.contains("different heap"), "unexpected error: {msg}");
        assert_eq!(
            heap_b
                .with_locked(|heap| Ok(heap.root_count()))
                .expect("root count"),
            0
        );
    }

    #[test]
    fn copying_gc_updates_handles_and_rejects_stale_pointers() {
        let heap = Heap::new();
        let stale = heap
            .with_locked(|heap| Ok(heap.alloc_ptr_i32(42)?.into_pointer()))
            .expect("alloc_i32 should succeed");
        let handle = heap.handle(stale).expect("handle should root pointer");

        heap.with_locked(|heap| heap.collect())
            .expect("collection should succeed");

        assert_eq!(
            handle.as_i32().expect("handle should follow moved value"),
            42
        );
        assert!(
            heap.with_locked(|heap| heap.pointer_as_i32(&stale))
                .is_err(),
            "raw pointer from before collection should be stale"
        );
    }

    #[test]
    fn temp_roots_detect_and_follow_copying_collection() {
        let heap = Heap::new();
        let stale = heap
            .with_locked(|heap| Ok(heap.alloc_ptr_i32(42)?.into_pointer()))
            .expect("alloc_i32 should succeed");
        let roots = heap
            .temp_roots(vec![stale])
            .expect("temporary root should register");

        assert!(
            !roots
                .has_collected_since_creation()
                .expect("collection state should be available")
        );
        heap.with_locked(|heap| heap.collect())
            .expect("collection should succeed");
        assert!(
            roots
                .has_collected_since_creation()
                .expect("collection state should be available")
        );

        let refreshed = roots.get(0).expect("temporary root should be rewritten");
        assert_eq!(
            heap.with_locked(|heap| heap.pointer_as_i32(&refreshed))
                .expect("rewritten pointer should resolve"),
            42
        );
        assert!(
            heap.with_locked(|heap| heap.pointer_as_i32(&stale))
                .is_err(),
            "raw pointer from before collection should be stale"
        );
    }

    #[test]
    fn alloc_triggers_collection_after_heap_growth() {
        let heap = Heap::new();
        let rooted = heap.alloc_i32(7).expect("alloc_i32 handle");
        heap.with_locked_ok(|heap| heap.set_gc_slot_threshold(1))
            .expect("set threshold");

        let _garbage = heap
            .with_locked(|heap| Ok(heap.alloc_ptr_i32(99)?.into_pointer()))
            .expect("alloc should trigger GC");

        assert!(
            heap.with_locked_ok(|heap| heap.collection_count())
                .expect("collection count")
                > 0,
            "allocation should have triggered collection"
        );
        assert_eq!(rooted.as_i32().expect("rooted value"), 7);
    }

    #[test]
    fn alloc_list_protects_inputs_across_collection() {
        let heap = Heap::new();
        heap.with_locked_ok(|heap| heap.set_gc_slot_threshold(usize::MAX))
            .expect("set threshold");
        let values = (0..2048)
            .map(|value| {
                heap.with_locked(|heap| Ok(heap.alloc_ptr_i32(value)?.into_pointer()))
                    .expect("alloc_i32 should succeed")
            })
            .collect::<Vec<_>>();
        heap.with_locked_ok(|heap| heap.set_gc_slot_threshold(1))
            .expect("set threshold");

        let list = heap
            .alloc_ptr_list(values)
            .expect("list allocation should protect inputs");
        let list = heap.handle(list).expect("list should be rootable");
        let values = heap
            .pointer_as_list(&list.pointer().expect("list pointer"))
            .expect("list should decode");

        assert_eq!(values.len(), 2048);
        assert_eq!(
            heap.with_locked(|heap| heap.pointer_as_i32(values.first().expect("first value")))
                .expect("first i32"),
            0
        );
        assert_eq!(
            heap.with_locked(|heap| heap.pointer_as_i32(values.last().expect("last value")))
                .expect("last i32"),
            2047
        );
    }

    #[test]
    fn alloc_ptr_list_uses_vector_backed_slice_representation() {
        let heap = Heap::new();
        let values = [1, 2, 3]
            .into_iter()
            .map(|value| {
                heap.with_locked(|heap| Ok(heap.alloc_ptr_i32(value)?.into_pointer()))
                    .expect("alloc_i32 should succeed")
            })
            .collect::<Vec<_>>();

        let list = heap
            .alloc_ptr_list(values.clone())
            .expect("list allocation should succeed");
        let Cell::ListSlice {
            start,
            end,
            elements,
        } = heap.clone_cell(&list).expect("list cell should exist")
        else {
            panic!("expected vector-backed list slice");
        };
        assert_eq!(start, 0);
        assert_eq!(end, values.len());
        let Cell::Data(backing) = heap.clone_cell(&elements).expect("data cell should exist")
        else {
            panic!("expected list data backing");
        };
        assert_eq!(backing, values);
    }

    #[test]
    fn list_slice_tail_shares_data_backing() {
        let heap = Heap::new();
        let values = [1, 2, 3]
            .into_iter()
            .map(|value| {
                heap.with_locked(|heap| Ok(heap.alloc_ptr_i32(value)?.into_pointer()))
                    .expect("alloc_i32 should succeed")
            })
            .collect::<Vec<_>>();
        let list = heap
            .alloc_ptr_list(values)
            .expect("list allocation should succeed");
        let Cell::ListSlice {
            elements: original_data,
            ..
        } = heap.clone_cell(&list).expect("list cell should exist")
        else {
            panic!("expected vector-backed list slice");
        };

        let (_head, tail) = heap
            .list_head_tail(&list)
            .expect("head/tail should decode")
            .expect("list should be non-empty");
        let Cell::ListSlice {
            start,
            end,
            elements,
        } = heap.clone_cell(&tail).expect("tail cell should exist")
        else {
            panic!("expected tail list slice");
        };
        assert_eq!(start, 1);
        assert_eq!(end, 3);
        assert_eq!(elements, original_data);
    }

    fn binary_slice(heap: &Heap, values: &[u8], start: usize, end: usize) -> Handle {
        let data = heap
            .alloc_binary_data(values.to_vec())
            .expect("binary data should allocate");
        heap.alloc_list_slice(start, end, data)
            .expect("binary slice should allocate")
    }

    fn data_u8_slice(heap: &Heap, values: &[u8], start: usize, end: usize) -> Handle {
        let values = values
            .iter()
            .map(|value| heap.alloc_u8(*value).expect("u8 should allocate"))
            .collect::<Vec<_>>();
        let data = heap.alloc_data(values).expect("data should allocate");
        heap.alloc_list_slice(start, end, data)
            .expect("data slice should allocate")
    }

    fn cons_u8_prefix(heap: &Heap, prefix: &[u8], tail: Handle) -> Handle {
        let mut list = tail;
        for value in prefix.iter().rev() {
            let head = heap.alloc_u8(*value).expect("u8 should allocate");
            list = heap.alloc_cons(head, list).expect("cons should allocate");
        }
        list
    }

    fn assert_vec_u8(handle: &Handle, expected: &[u8]) {
        assert_eq!(
            Vec::<u8>::from_rex(handle).expect("Vec<u8> should decode"),
            expected
        );
    }

    #[test]
    fn vec_u8_into_rex_uses_binary_data_backing() {
        let heap = Heap::new();
        let bytes = vec![1u8, 2, 3, 4]
            .into_rex(&heap)
            .expect("Vec<u8> should convert");

        let Cell::ListSlice {
            start,
            end,
            elements,
        } = heap
            .clone_cell(&bytes.pointer().expect("bytes pointer"))
            .expect("list cell should exist")
        else {
            panic!("expected binary-backed list slice");
        };
        assert_eq!(start, 0);
        assert_eq!(end, 4);
        let Cell::BinaryData(backing) = heap
            .clone_cell(&elements)
            .expect("binary data cell should exist")
        else {
            panic!("expected binary data backing");
        };
        assert_eq!(backing, vec![1, 2, 3, 4]);
        assert_eq!(
            bytes
                .display_with(ValueDisplayOptions {
                    include_numeric_suffixes: true,
                    ..ValueDisplayOptions::default()
                })
                .expect("binary list should display"),
            "[1u8, 2u8, 3u8, 4u8]"
        );
    }

    #[test]
    fn binary_list_head_tail_shares_binary_backing_across_gc() {
        let heap = Heap::new();
        let bytes = vec![7u8, 8, 9]
            .into_rex(&heap)
            .expect("Vec<u8> should convert");

        heap.set_collect_on_every_alloc(true)
            .expect("enable gc every alloc");
        let (head, tail) = heap
            .list_head_tail(&bytes.pointer().expect("bytes pointer"))
            .expect("head/tail should decode")
            .expect("list should be non-empty");

        assert_eq!(
            heap.with_locked(|heap| heap.pointer_as_u8(&head))
                .expect("head should be u8"),
            7
        );
        let Cell::ListSlice {
            start,
            end,
            elements,
        } = heap.clone_cell(&tail).expect("tail cell should exist")
        else {
            panic!("expected binary tail list slice");
        };
        assert_eq!(start, 1);
        assert_eq!(end, 3);
        let Cell::BinaryData(backing) = heap
            .clone_cell(&elements)
            .expect("binary data cell should exist")
        else {
            panic!("expected binary data backing");
        };
        assert_eq!(backing, vec![7, 8, 9]);
        let tail = heap.handle(tail).expect("tail should be rootable");
        assert_vec_u8(&tail, &[8, 9]);
    }

    #[test]
    fn vec_u8_from_rex_decodes_all_list_backing_permutations() {
        let heap = Heap::new();

        let empty = Vec::<u8>::new()
            .into_rex(&heap)
            .expect("empty Vec<u8> should convert");
        assert_vec_u8(&empty, &[]);

        let binary_full = binary_slice(&heap, &[10, 11, 12, 13], 0, 4);
        assert_vec_u8(&binary_full, &[10, 11, 12, 13]);

        let binary_sub = binary_slice(&heap, &[10, 11, 12, 13, 14], 1, 4);
        assert_vec_u8(&binary_sub, &[11, 12, 13]);

        let data_full = data_u8_slice(&heap, &[20, 21, 22], 0, 3);
        assert_vec_u8(&data_full, &[20, 21, 22]);

        let data_sub = data_u8_slice(&heap, &[20, 21, 22, 23], 1, 3);
        assert_vec_u8(&data_sub, &[21, 22]);

        let cons_only = cons_u8_prefix(&heap, &[1, 2], heap.alloc_empty().expect("empty list"));
        assert_vec_u8(&cons_only, &[1, 2]);

        let cons_then_data =
            cons_u8_prefix(&heap, &[1, 2], data_u8_slice(&heap, &[30, 31, 32], 1, 3));
        assert_vec_u8(&cons_then_data, &[1, 2, 31, 32]);

        let cons_then_binary =
            cons_u8_prefix(&heap, &[1, 2], binary_slice(&heap, &[40, 41, 42, 43], 1, 4));
        assert_vec_u8(&cons_then_binary, &[1, 2, 41, 42, 43]);
    }

    #[test]
    fn binary_and_data_backed_u8_lists_compare_and_view_as_lists() {
        let heap = Heap::new();
        let binary = binary_slice(&heap, &[1, 2, 3, 4], 1, 3);
        let data = data_u8_slice(&heap, &[2, 3], 0, 2);

        assert!(
            binary.value_eq(&data).expect("lists should compare"),
            "binary-backed and data-backed lists should compare by elements"
        );
        assert_eq!(
            binary
                .display_with(ValueDisplayOptions {
                    include_numeric_suffixes: true,
                    ..ValueDisplayOptions::default()
                })
                .expect("binary list should display"),
            "[2u8, 3u8]"
        );
        let values = binary.as_list().expect("binary list should materialize");
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].as_u8().expect("first byte"), 2);
        assert_eq!(values[1].as_u8().expect("second byte"), 3);
    }

    #[test]
    fn copying_gc_traces_deep_lists_iteratively() {
        let heap = Heap::new();
        heap.with_locked_ok(|heap| heap.set_gc_slot_threshold(usize::MAX))
            .expect("set threshold");
        let values = (0..10_000)
            .map(|value| {
                heap.with_locked(|heap| Ok(heap.alloc_ptr_i32(value)?.into_pointer()))
                    .expect("alloc_i32 should succeed")
            })
            .collect::<Vec<_>>();
        let list = heap
            .handle(
                heap.alloc_ptr_list(values)
                    .expect("list allocation should succeed"),
            )
            .expect("list should be rootable");

        heap.with_locked(|heap| heap.collect())
            .expect("deep collection should succeed");

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
        let first = heap
            .with_locked(|heap| Ok(heap.alloc_ptr_i32(1)?.into_pointer()))
            .expect("alloc_i32 should succeed");
        let second = heap
            .with_locked(|heap| Ok(heap.alloc_ptr_string("two".into())?.into_pointer()))
            .expect("alloc_string should succeed");
        let tuple = heap
            .handle(
                heap.with_locked(|heap| {
                    Ok(heap.alloc_ptr_tuple(vec![first, second])?.into_pointer())
                })
                .expect("alloc_tuple should succeed"),
            )
            .expect("tuple should be rootable");

        assert_eq!(
            heap.with_locked(|heap| Ok(heap.root_count()))
                .expect("root count"),
            1
        );

        let view = tuple.value().expect("tuple value");
        let Value::Tuple(items) = &view else {
            panic!("expected tuple value");
        };
        assert_eq!(
            heap.with_locked(|heap| Ok(heap.root_count()))
                .expect("root count"),
            3
        );
        assert_eq!(items.len(), 2);
        assert_eq!(i32::from_rex(&items[0]).expect("i32 should decode"), 1);
        assert_eq!(
            String::from_rex(&items[1]).expect("string should decode"),
            "two"
        );

        drop(view);
        assert_eq!(
            heap.with_locked(|heap| Ok(heap.root_count()))
                .expect("root count"),
            1
        );

        let items = tuple.as_tuple().expect("tuple handle");
        assert_eq!(
            heap.with_locked(|heap| Ok(heap.root_count()))
                .expect("root count"),
            3
        );
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn handle_value_reports_named_composites() {
        let heap = Heap::new();
        let payload = heap
            .with_locked(|heap| Ok(heap.alloc_ptr_bool(true)?.into_pointer()))
            .expect("alloc_bool should succeed");

        let mut fields = BTreeMap::new();
        fields.insert(Symbol::intern("ready"), payload);
        let dict = heap
            .handle(
                heap.with_locked(|heap| Ok(heap.alloc_ptr_dict(fields)?.into_pointer()))
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
                heap.with_locked(|heap| {
                    Ok(heap
                        .alloc_ptr_adt(Symbol::intern("Some"), vec![payload])?
                        .into_pointer())
                })
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

        assert_eq!(
            heap.with_locked(|heap| Ok(heap.root_count()))
                .expect("root count"),
            2
        );
    }

    #[test]
    fn rex_traits_roundtrip_containers() {
        let heap = Heap::new();

        let array = vec![1i32, 2, 3]
            .into_rex(&heap)
            .expect("vec should convert");
        assert_eq!(
            heap.with_locked(|heap| Ok(heap.root_count()))
                .expect("root count"),
            1
        );
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
        assert_eq!(
            heap.with_locked(|heap| Ok(heap.root_count()))
                .expect("root count"),
            1
        );

        let cloned = Handle::from_rex(&handle).expect("handle should clone");
        assert_eq!(
            heap.with_locked(|heap| Ok(heap.root_count()))
                .expect("root count"),
            1
        );

        let returned = cloned.into_rex(&heap).expect("handle should convert");
        assert_eq!(i32::from_rex(&returned).expect("i32 should decode"), 7);
        assert_eq!(
            heap.with_locked(|heap| Ok(heap.root_count()))
                .expect("root count"),
            1
        );

        drop(handle);
        assert_eq!(
            heap.with_locked(|heap| Ok(heap.root_count()))
                .expect("root count"),
            1
        );
        drop(returned);
        assert_eq!(
            heap.with_locked(|heap| Ok(heap.root_count()))
                .expect("root count"),
            0
        );
    }
}
