//! Single-owner heap storage, cells, and copying-GC roots.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use blake3::Hash;
use chrono::{DateTime, Utc};
use rex_ast::Symbol;
use rex_typesystem::types::{BuiltinTypeId, Type, TypedExpr};
use uuid::Uuid;

use crate::{
    EngineError, env::ScopedEnvironment, native_fn::NativeFn, overloaded_fn::OverloadedFn,
};

use super::lists::{
    ListElement, ListItems, ListItemsSeed, ListRootElement, collect_list_u8,
    list_elements_from_pointer, list_elements_to_rooted_ptr_vec, list_items_from_pointer,
    list_len_from_pointer, list_slice_backing_len, list_slice_head_element,
    materialize_list_elements, validate_list_slice_bounds,
};

trait Collection {
    fn map_pointers<E>(
        &mut self,
        map: &mut impl FnMut(InternalPtr) -> Result<InternalPtr, E>,
    ) -> Result<(), E>;

    fn trace_pointers(&mut self, out: &mut Vec<InternalPtr>) {
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

#[derive(Clone)]
struct InternalEnvironment(Arc<InternalEnvEntry>);

#[derive(Clone)]
struct InternalEnvEntry {
    parent: Option<InternalEnvironment>,
    bindings: BTreeMap<Symbol, InternalPtr>,
}

impl InternalEnvironment {
    fn from_scoped(env: &ScopedEnvironment, scope: &RootScope<'_>) -> Self {
        let mut rebuilt = None;
        for bindings in env.entries().into_iter().rev() {
            let bindings = bindings
                .into_iter()
                .map(|(name, value)| (name, scope.pointer(value)))
                .collect();
            rebuilt = Some(Self(Arc::new(InternalEnvEntry {
                parent: rebuilt,
                bindings,
            })));
        }
        rebuilt.unwrap_or_else(|| {
            Self(Arc::new(InternalEnvEntry {
                parent: None,
                bindings: BTreeMap::new(),
            }))
        })
    }

    fn to_scoped(&self, scope: &mut RootScope<'_>) -> ScopedEnvironment {
        let mut entries = Vec::new();
        let mut current = Some(self);
        while let Some(env) = current {
            entries.push(
                env.0
                    .bindings
                    .iter()
                    .map(|(name, value)| (name.clone(), scope.root(*value)))
                    .collect(),
            );
            current = env.0.parent.as_ref();
        }
        ScopedEnvironment::from_entries(entries)
    }
}

impl Collection for InternalEnvironment {
    fn map_pointers<E>(
        &mut self,
        map: &mut impl FnMut(InternalPtr) -> Result<InternalPtr, E>,
    ) -> Result<(), E> {
        let mut entries = Vec::new();
        let mut current = Some(&*self);
        while let Some(env) = current {
            let mut bindings = BTreeMap::new();
            for (name, pointer) in &env.0.bindings {
                bindings.insert(name.clone(), map(*pointer)?);
            }
            entries.push(bindings);
            current = env.0.parent.as_ref();
        }

        let mut rebuilt = None;
        for bindings in entries.into_iter().rev() {
            rebuilt = Some(Self(Arc::new(InternalEnvEntry {
                parent: rebuilt,
                bindings,
            })));
        }
        *self = rebuilt.unwrap_or_else(|| {
            Self(Arc::new(InternalEnvEntry {
                parent: None,
                bindings: BTreeMap::new(),
            }))
        });
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct Closure {
    env: InternalEnvironment,
    pub param: Symbol,
    pub param_ty: Type,
    pub typ: Type,
    pub body: Arc<TypedExpr>,
}

pub(crate) struct RootedClosure {
    pub(crate) env: ScopedEnvironment,
    pub(crate) param: Symbol,
    pub(crate) param_ty: Type,
    pub(crate) typ: Type,
    pub(crate) body: Arc<TypedExpr>,
}

pub(crate) enum RootedCallable {
    Closure(RootedClosure),
    Native(NativeFn<RootedPtr>),
    Overloaded(OverloadedFn<RootedPtr>),
}

/// Mutable storage and collector state for one Rex heap.
///
/// `Heap` moves from builder to compiler to evaluator and is never shared.
/// Exclusive `&mut Heap` access is the proof that a synchronous operation
/// has sole access to the collector; evaluator code receives that proof through
/// [`RootScope`].
/// A scope must never invoke host code, block, or cross an `await` boundary.
///
/// The collector recognizes three kinds of edges:
///
/// - [`InternalPtr`] values occur only in heap cells and short-lived locals.
///   Collection traces and rewrites every reachable internal edge.
/// - Runtime root slots back stable [`RootedPtr`] tokens held by evaluator and
///   compiler state. Collection rewrites the slots, while their generational
///   identifiers remain stable.
///
/// Any allocation can collect before creating its result. An `InternalPtr`
/// local therefore cannot survive an allocation unless it has first been
/// placed in a traced cell or converted to one of the rooted representations.
pub(crate) struct Heap {
    id: u64,
    slots: Vec<HeapSlot>,
    runtime_roots: Vec<RootSlot>,
    free_runtime_root_list: Vec<usize>,
    next_gc_slot_count: usize,
    extreme_stress: bool,
    collection_epoch: u64,
    defer_collection: bool,
}

const DEFAULT_GC_SLOT_THRESHOLD: usize = 4_096;
const GC_SLOT_GROWTH_NUMERATOR: usize = 3;
const GC_SLOT_GROWTH_DENOMINATOR: usize = 2;
const GC_EXTREME_STRESS: bool = false;

impl Heap {
    pub(crate) fn new() -> Self {
        static NEXT_HEAP_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_HEAP_ID.fetch_add(1, Ordering::Relaxed);
        Self {
            id,
            slots: Vec::new(),
            runtime_roots: Vec::new(),
            free_runtime_root_list: Vec::new(),
            next_gc_slot_count: DEFAULT_GC_SLOT_THRESHOLD,
            extreme_stress: false,
            collection_epoch: 0,
            defer_collection: false,
        }
    }

    fn collect_needed(&self) -> bool {
        self.extreme_stress_enabled() || self.slots.len() >= self.next_gc_slot_count
    }

    fn push_cell<'a>(&'a mut self, cell: Cell) -> Result<Reference<'a>, EngineError> {
        let index = u32::try_from(self.slots.len())
            .map_err(|_| EngineError::Internal("heap exhausted: too many slots".into()))?;
        let generation = self.collection_epoch;
        self.slots.push(HeapSlot {
            generation,
            cell: Some(cell),
        });
        Ok(Reference {
            heap: self,
            index,
            generation,
        })
    }

    fn extreme_stress_enabled(&self) -> bool {
        GC_EXTREME_STRESS || self.extreme_stress
    }

    pub(crate) fn set_extreme_stress(&mut self, enabled: bool) {
        self.extreme_stress = enabled;
    }

    #[cfg(test)]
    pub(crate) fn collection_count(&self) -> u64 {
        self.collection_epoch
    }

    fn finish_collection(&mut self, slots: Vec<HeapSlot>, next_epoch: u64) {
        self.slots = slots;
        self.collection_epoch = next_epoch;
        self.update_next_gc_slot_count();
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

    fn get_slot_checked<'a>(&'a self, pointer: &InternalPtr) -> Result<&'a HeapSlot, EngineError> {
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

    pub(crate) fn collect(&mut self) -> Result<(), EngineError> {
        let next_epoch = self
            .collection_epoch
            .checked_add(1)
            .ok_or_else(|| EngineError::Internal("heap collection count exhausted".into()))?;
        let mut forwarding = vec![None; self.slots.len()];
        let mut work = self
            .runtime_roots
            .iter()
            .filter_map(|slot| slot.pointer)
            .collect::<VecDeque<_>>();
        let mut seen = vec![false; self.slots.len()];
        let mut live = Vec::new();

        while let Some(pointer) = work.pop_front() {
            let slot = self.get_slot_checked(&pointer)?;
            let index = pointer.index as usize;
            if seen[index] {
                continue;
            }
            seen[index] = true;

            let mut cell = slot
                .cell
                .as_ref()
                .ok_or_else(|| invalid_pointer(self.id, pointer.index, pointer.generation))?
                .clone();
            let mut children = Vec::new();
            cell.trace_pointers(&mut children);
            work.extend(children);
            live.push((pointer, cell));
        }

        let old_indices = live
            .iter()
            .map(|(pointer, _)| pointer.index)
            .collect::<Vec<_>>();
        let destinations = if self.extreme_stress_enabled() {
            randomized_gc_destinations(&old_indices, self.collection_epoch)
        } else {
            (0..live.len()).collect()
        };
        for (live_index, (pointer, _)) in live.iter().enumerate() {
            self.get_slot_checked(pointer)?;
            let index = u32::try_from(destinations[live_index])
                .map_err(|_| EngineError::Internal("heap exhausted: too many slots".into()))?;
            forwarding[pointer.index as usize] = Some(InternalPtr {
                heap_id: self.id,
                index,
                generation: next_epoch,
            });
        }

        let relocated_runtime_roots = self
            .runtime_roots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| slot.pointer.map(|pointer| (index, pointer)))
            .map(|(index, pointer)| Ok((index, self.forward_for_gc(pointer, &forwarding)?)))
            .collect::<Result<Vec<_>, EngineError>>()?;

        let mut new_slots = vec![None; live.len()];
        for (live_index, (old_pointer, mut cell)) in live.into_iter().enumerate() {
            cell.map_pointers(&mut |child| self.forward_for_gc(child, &forwarding))?;
            let pointer = self.forward_for_gc(old_pointer, &forwarding)?;
            let slot = new_slots.get_mut(destinations[live_index]).ok_or_else(|| {
                EngineError::Internal("copying GC destination out of bounds".into())
            })?;
            if slot.is_some() {
                return Err(EngineError::Internal(
                    "copying GC assigned a destination twice".into(),
                ));
            }
            *slot = Some(HeapSlot {
                generation: pointer.generation,
                cell: Some(cell),
            });
        }
        let new_slots = new_slots
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| EngineError::Internal("copying GC left a destination empty".into()))?;

        for (index, pointer) in relocated_runtime_roots {
            self.runtime_roots[index].pointer = Some(pointer);
        }
        self.finish_collection(new_slots, next_epoch);
        #[cfg(debug_assertions)]
        self.verify_after_collection()?;
        Ok(())
    }

    fn forward_for_gc(
        &self,
        pointer: InternalPtr,
        forwarding: &[Option<InternalPtr>],
    ) -> Result<InternalPtr, EngineError> {
        self.get_slot_checked(&pointer)?;
        forwarding
            .get(pointer.index as usize)
            .copied()
            .flatten()
            .ok_or_else(|| EngineError::Internal("copying GC forwarding pointer missing".into()))
    }

    #[cfg(debug_assertions)]
    fn verify_after_collection(&self) -> Result<(), EngineError> {
        let mut work = VecDeque::new();
        let mut seen = HashSet::new();

        for (root_index, slot) in self.runtime_roots.iter().enumerate() {
            let Some(pointer) = slot.pointer else {
                continue;
            };
            self.get_slot_checked(&pointer).map_err(|err| {
                EngineError::Internal(format!(
                    "GC verification failed for runtime root {root_index}: {err}"
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
            if slot.generation != self.collection_epoch {
                return Err(EngineError::Internal(format!(
                    "GC verification found generation {} at slot {index}, expected heap epoch {}",
                    slot.generation, self.collection_epoch
                )));
            }
            if slot.cell.is_none() {
                return Err(EngineError::Internal(format!(
                    "GC verification found empty slot {index} after collection"
                )));
            }
            let index = u32::try_from(index)
                .map_err(|_| EngineError::Internal("heap exhausted: too many slots".into()))?;
            let pointer = InternalPtr {
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

    // The single ordinary heap-object allocation path. Children in the
    // pending cell are temporarily registered with the lock-free runtime root
    // table so a collection can relocate them before the cell is installed.
    fn alloc_reference<'a>(&'a mut self, mut cell: Cell) -> Result<Reference<'a>, EngineError> {
        if self.defer_collection || !self.collect_needed() {
            return self.push_cell(cell);
        }

        let mut pointers = Vec::new();
        cell.trace_pointers(&mut pointers);
        for pointer in &pointers {
            self.get_slot_checked(pointer)?;
        }
        let roots = pointers
            .into_iter()
            .map(|pointer| self.register_runtime_root(pointer))
            .collect::<Vec<_>>();

        let collected = self.collect();
        if let Err(error) = collected {
            for root in roots {
                self.unregister_runtime_root(root);
            }
            return Err(error);
        }

        let relocated = roots
            .iter()
            .map(|root| self.resolve_runtime_root(*root))
            .collect::<Vec<_>>();
        for root in roots {
            self.unregister_runtime_root(root);
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

    pub(super) fn get_cell_from_pointer(
        &self,
        pointer: &InternalPtr,
    ) -> Result<&Cell, EngineError> {
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

    fn overwrite(&mut self, pointer: &InternalPtr, cell: Cell) -> Result<(), EngineError> {
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

    #[cfg(test)]
    pub(crate) fn root_scope<R>(&mut self, f: impl FnOnce(&mut RootScope<'_>) -> R) -> R {
        let mut scope = RootScope {
            heap: self,
            created_runtime_roots: Vec::new(),
            retain_runtime_roots: false,
        };

        f(&mut scope)
    }

    pub(crate) fn machine_root_scope<R>(&mut self, f: impl FnOnce(&mut RootScope<'_>) -> R) -> R {
        self.defer_collection = true;
        let result = {
            let mut scope = RootScope {
                heap: self,
                created_runtime_roots: Vec::new(),
                retain_runtime_roots: true,
            };
            f(&mut scope)
        };
        self.defer_collection = false;
        result
    }

    fn register_runtime_root(&mut self, pointer: InternalPtr) -> RootedPtr {
        let slot = match self.free_runtime_root_list.pop() {
            Some(slot) => {
                let root = &mut self.runtime_roots[slot];
                debug_assert!(root.pointer.is_none());
                root.pointer = Some(pointer);
                slot
            }
            None => {
                let slot = self.runtime_roots.len();
                self.runtime_roots.push(RootSlot {
                    generation: 0,
                    pointer: Some(pointer),
                });
                slot
            }
        };
        RootedPtr {
            heap_id: self.id,
            slot,
            generation: self.runtime_roots[slot].generation,
        }
    }

    fn unregister_runtime_root(&mut self, root: RootedPtr) {
        if root.heap_id != self.id {
            return;
        }
        let Some(slot) = self.runtime_roots.get_mut(root.slot) else {
            return;
        };
        if slot.generation != root.generation || slot.pointer.is_none() {
            return;
        }
        slot.pointer = None;
        slot.generation = slot.generation.wrapping_add(1);
        self.free_runtime_root_list.push(root.slot);
    }

    fn resolve_runtime_root(&self, root: RootedPtr) -> InternalPtr {
        debug_assert_eq!(root.heap_id, self.id);
        let slot = &self.runtime_roots[root.slot];
        debug_assert_eq!(slot.generation, root.generation);
        match slot.pointer {
            Some(pointer) => pointer,
            None => panic!("runtime root was released while still in use"),
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
    pointer: Option<InternalPtr>,
}

/// Stable token for a value owned by the single-threaded runtime.
///
/// A `RootedPtr` indexes the runtime root table and does not expose a physical
/// heap address. Collection rewrites the table entry whenever the value moves,
/// so the owning evaluator may keep the token across host-call awaits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RootedPtr {
    heap_id: u64,
    slot: usize,
    generation: u64,
}

/// Exclusive synchronous access to one [`Heap`].
///
/// `RootScope` is the only general evaluator capability for inspecting and
/// allocating Rex values. Its `&mut Heap` proves exclusive collector
/// access. Allocation may collect, and the collector rewrites runtime-root
/// entries before control returns.
///
/// A scope deliberately contains no public heap capability. Scoped code must
/// not call host callbacks, block, or cross an `await` boundary.
pub(crate) struct RootScope<'heap> {
    pub(super) heap: &'heap mut Heap,
    created_runtime_roots: Vec<RootedPtr>,
    retain_runtime_roots: bool,
}

impl Drop for RootScope<'_> {
    fn drop(&mut self) {
        if !self.retain_runtime_roots {
            for root in self.created_runtime_roots.drain(..) {
                self.heap.unregister_runtime_root(root);
            }
        }
    }
}

impl RootScope<'_> {
    pub(crate) fn collection_needed(&self) -> bool {
        self.heap.collect_needed()
    }

    pub(crate) fn collect_if_needed(
        &mut self,
        live_roots: &HashSet<RootedPtr>,
    ) -> Result<(), EngineError> {
        if !self.heap.collect_needed() {
            return Ok(());
        }

        let mut stale = Vec::new();
        for (slot, root) in self.heap.runtime_roots.iter().enumerate() {
            if root.pointer.is_none() {
                continue;
            }
            let token = RootedPtr {
                heap_id: self.heap.id,
                slot,
                generation: root.generation,
            };
            if !live_roots.contains(&token) {
                stale.push(token);
            }
        }
        for root in stale {
            self.heap.unregister_runtime_root(root);
        }
        self.heap.collect()
    }

    pub(super) fn root(&mut self, ptr: InternalPtr) -> RootedPtr {
        let root = self.heap.register_runtime_root(ptr);
        self.created_runtime_roots.push(root);
        root
    }

    pub(super) fn pointer(&self, root: RootedPtr) -> InternalPtr {
        debug_assert_eq!(root.heap_id, self.heap.id);
        let slot = &self.heap.runtime_roots[root.slot];
        debug_assert_eq!(slot.generation, root.generation);
        match slot.pointer {
            Some(pointer) => pointer,
            None => panic!("runtime root was released while still in use"),
        }
    }

    pub(crate) fn type_name(&self, root: RootedPtr) -> Result<&'static str, EngineError> {
        self.get_cell_from_rooted_ptr(root)
            .map(Cell::cell_type_name)
    }

    pub(crate) fn infer_type(&self, root: RootedPtr) -> Result<Type, EngineError> {
        infer_cell_type(self.heap, self.get_cell_from_rooted_ptr(root)?)
    }

    pub(super) fn get_cell_from_rooted_ptr(&self, root: RootedPtr) -> Result<&Cell, EngineError> {
        let pointer = self.pointer(root);
        self.heap.get_cell_from_pointer(&pointer)
    }

    pub(crate) fn list_items(
        &mut self,
        root: RootedPtr,
    ) -> Result<ListItems<RootedPtr>, EngineError> {
        let pointer = self.pointer(root);
        match list_items_from_pointer(self.heap, pointer)? {
            ListItemsSeed::Ready(items) => Ok(items.into_rooted(self)),
            ListItemsSeed::Elements(elements) => {
                if elements
                    .iter()
                    .all(|element| matches!(element, ListElement::InternalPtr(_)))
                {
                    return list_elements_to_rooted_ptr_vec(self, elements)
                        .map(ListItems::Pointers);
                }
                materialize_list_elements(self, elements).map(ListItems::Pointers)
            }
        }
    }

    fn alloc_reference(&mut self, cell: Cell) -> Result<RootedPtr, EngineError> {
        let reference = self.heap.alloc_reference(cell)?;
        let pointer = reference.into_pointer();
        Ok(self.root(pointer))
    }

    pub(crate) fn alloc_root_bool(&mut self, value: bool) -> Result<RootedPtr, EngineError> {
        self.alloc_reference(Cell::Bool(value))
    }

    pub(crate) fn alloc_root_u8(&mut self, value: u8) -> Result<RootedPtr, EngineError> {
        self.alloc_reference(Cell::U8(value))
    }

    pub(crate) fn alloc_root_u16(&mut self, value: u16) -> Result<RootedPtr, EngineError> {
        self.alloc_reference(Cell::U16(value))
    }

    pub(crate) fn alloc_root_u32(&mut self, value: u32) -> Result<RootedPtr, EngineError> {
        self.alloc_reference(Cell::U32(value))
    }

    pub(crate) fn alloc_root_u64(&mut self, value: u64) -> Result<RootedPtr, EngineError> {
        self.alloc_reference(Cell::U64(value))
    }

    pub(crate) fn alloc_root_i8(&mut self, value: i8) -> Result<RootedPtr, EngineError> {
        self.alloc_reference(Cell::I8(value))
    }

    pub(crate) fn alloc_root_i16(&mut self, value: i16) -> Result<RootedPtr, EngineError> {
        self.alloc_reference(Cell::I16(value))
    }

    pub(crate) fn alloc_root_i32(&mut self, value: i32) -> Result<RootedPtr, EngineError> {
        self.alloc_reference(Cell::I32(value))
    }

    pub(crate) fn alloc_root_i64(&mut self, value: i64) -> Result<RootedPtr, EngineError> {
        self.alloc_reference(Cell::I64(value))
    }

    pub(crate) fn alloc_root_f32(&mut self, value: f32) -> Result<RootedPtr, EngineError> {
        self.alloc_reference(Cell::F32(value))
    }

    pub(crate) fn alloc_root_f64(&mut self, value: f64) -> Result<RootedPtr, EngineError> {
        self.alloc_reference(Cell::F64(value))
    }

    pub(crate) fn alloc_root_string(&mut self, value: String) -> Result<RootedPtr, EngineError> {
        self.alloc_reference(Cell::String(value))
    }

    pub(crate) fn alloc_root_uuid(&mut self, value: Uuid) -> Result<RootedPtr, EngineError> {
        self.alloc_reference(Cell::Uuid(value))
    }

    pub(crate) fn alloc_root_hash(&mut self, value: Hash) -> Result<RootedPtr, EngineError> {
        self.alloc_reference(Cell::Hash(value))
    }

    pub(crate) fn alloc_root_datetime(
        &mut self,
        value: DateTime<Utc>,
    ) -> Result<RootedPtr, EngineError> {
        self.alloc_reference(Cell::DateTime(value))
    }

    pub(crate) fn alloc_root_uninitialized(
        &mut self,
        name: Symbol,
    ) -> Result<RootedPtr, EngineError> {
        self.alloc_reference(Cell::Uninitialized(name))
    }

    pub(crate) fn alloc_root_tuple(
        &mut self,
        values: Vec<RootedPtr>,
    ) -> Result<RootedPtr, EngineError> {
        let values = values.into_iter().map(|v| self.pointer(v)).collect();
        self.alloc_reference(Cell::Tuple(values))
    }

    pub(crate) fn alloc_root_dict(
        &mut self,
        values: BTreeMap<String, RootedPtr>,
    ) -> Result<RootedPtr, EngineError> {
        let values = BTreeMap::from_iter(
            values
                .into_iter()
                .map(|(k, v)| (k, self.pointer(v)))
                .collect::<BTreeMap<_, _>>(),
        );
        self.alloc_reference(Cell::Dict(values))
    }

    pub(crate) fn alloc_root_adt(
        &mut self,
        name: Symbol,
        args: Vec<RootedPtr>,
    ) -> Result<RootedPtr, EngineError> {
        let args = args.into_iter().map(|arg| self.pointer(arg)).collect();
        self.alloc_reference(Cell::Adt(name, args))
    }

    pub(crate) fn alloc_root_empty(&mut self) -> Result<RootedPtr, EngineError> {
        self.alloc_reference(Cell::Empty)
    }

    pub(crate) fn alloc_root_cons(
        &mut self,
        head: RootedPtr,
        tail: RootedPtr,
    ) -> Result<RootedPtr, EngineError> {
        let head = self.pointer(head);
        let tail = self.pointer(tail);
        self.alloc_reference(Cell::Cons(head, tail))
    }

    pub(crate) fn alloc_root_data(
        &mut self,
        values: Vec<RootedPtr>,
    ) -> Result<RootedPtr, EngineError> {
        let values = values.into_iter().map(|v| self.pointer(v)).collect();
        self.alloc_reference(Cell::Data(values))
    }

    pub(crate) fn alloc_root_binary_data(
        &mut self,
        values: Vec<u8>,
    ) -> Result<RootedPtr, EngineError> {
        self.alloc_reference(Cell::BinaryData(values))
    }

    pub(crate) fn alloc_root_closure(
        &mut self,
        env: ScopedEnvironment,
        param: Symbol,
        param_ty: Type,
        typ: Type,
        body: Arc<TypedExpr>,
    ) -> Result<RootedPtr, EngineError> {
        let env = InternalEnvironment::from_scoped(&env, self);
        self.alloc_reference(Cell::Closure(Closure {
            env,
            param,
            param_ty,
            typ,
            body,
        }))
    }

    pub(crate) fn alloc_root_native(
        &mut self,
        native_id: u64,
        name: Symbol,
        arity: usize,
        typ: Type,
        applied: Vec<RootedPtr>,
        applied_types: Vec<Type>,
    ) -> Result<RootedPtr, EngineError> {
        let applied = applied.into_iter().map(|x| self.pointer(x)).collect();
        self.alloc_reference(Cell::Native(NativeFn::from_parts(
            native_id,
            name,
            arity,
            typ,
            applied,
            applied_types,
        )))
    }

    pub(crate) fn alloc_root_overloaded(
        &mut self,
        name: Symbol,
        typ: Type,
        applied: Vec<RootedPtr>,
        applied_types: Vec<Type>,
    ) -> Result<RootedPtr, EngineError> {
        let applied = applied.into_iter().map(|x| self.pointer(x)).collect();
        self.alloc_reference(Cell::Overloaded(OverloadedFn::from_parts(
            name,
            typ,
            applied,
            applied_types,
        )))
    }

    pub(crate) fn alloc_root_list_slice(
        &mut self,
        start: usize,
        end: usize,
        elements: RootedPtr,
    ) -> Result<RootedPtr, EngineError> {
        let elements = self.pointer(elements);
        let len = list_slice_backing_len(self.heap.get_cell_from_pointer(&elements)?)?;
        validate_list_slice_bounds(len, start, end)?;
        if start == end {
            return self.alloc_root_empty();
        }
        self.alloc_reference(Cell::ListSlice {
            start,
            end,
            elements,
        })
    }

    pub(crate) fn list_head_tail(
        &mut self,
        pointer: RootedPtr,
    ) -> Result<Option<(RootedPtr, RootedPtr)>, EngineError> {
        let scope = self;

        let Some((a, b, c)) = (|| {
            let cell = scope.get_cell_from_rooted_ptr(pointer)?.clone();
            match cell {
                Cell::Empty => Ok(None),
                Cell::Cons(head, tail) => {
                    let head = scope.root(head);
                    let tail = scope.root(tail);
                    Ok(Some((ListRootElement::RootedPtr(head), None, tail)))
                }
                Cell::ListSlice {
                    start,
                    end,
                    elements,
                } => {
                    let elements = scope.root(elements);
                    let Some(head) = list_slice_head_element(scope, elements, start, end)? else {
                        return Ok(None);
                    };
                    Ok(Some((head, Some((start + 1, end)), elements)))
                }
                _ => Err(EngineError::NativeType {
                    expected: "list".into(),
                    got: cell.cell_type_name().into(),
                }),
            }
        })()?
        else {
            return Ok(None);
        };

        let head: ListRootElement = a;
        let tail_index: Option<(usize, usize)> = b;
        let elements: RootedPtr = c;

        let head: RootedPtr = match head {
            ListRootElement::RootedPtr(pointer) => pointer,
            ListRootElement::U8(value) => {
                return (|| {
                    let head = scope.alloc_root_u8(value)?;
                    match tail_index {
                        Some((start, end)) => {
                            let tail = scope.alloc_root_list_slice(start, end, elements)?;
                            Ok(Some((head, tail)))
                        }
                        None => Ok(Some((head, elements))),
                    }
                })();
            }
        };
        let tail: RootedPtr = match tail_index {
            Some((start, end)) => {
                let tail = scope.alloc_root_list_slice(start, end, elements)?;
                return Ok(Some((head, tail)));
            }
            None => elements,
        };
        Ok(Some((head, tail)))
    }

    pub(crate) fn list_len(&self, root: RootedPtr) -> Result<usize, EngineError> {
        let pointer = self.pointer(root);
        list_len_from_pointer(self.heap, pointer)
    }

    pub(crate) fn alloc_root_list(
        &mut self,
        values: Vec<RootedPtr>,
    ) -> Result<RootedPtr, EngineError> {
        if values.is_empty() {
            return self.alloc_root_empty();
        }
        let len = values.len();
        let data = self.alloc_root_data(values)?;
        self.alloc_root_list_slice(0, len, data)
    }

    pub(crate) fn alloc_root_binary_list(
        &mut self,
        values: Vec<u8>,
    ) -> Result<RootedPtr, EngineError> {
        if values.is_empty() {
            return self.alloc_root_empty();
        }
        let len = values.len();
        let data = self.alloc_root_binary_data(values)?;
        self.alloc_root_list_slice(0, len, data)
    }

    pub(crate) fn root_as_bool(&self, root: RootedPtr) -> Result<bool, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_bool()
    }

    pub(crate) fn root_as_u8(&self, root: RootedPtr) -> Result<u8, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_u8()
    }

    pub(crate) fn root_as_u16(&self, root: RootedPtr) -> Result<u16, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_u16()
    }

    pub(crate) fn root_as_u32(&self, root: RootedPtr) -> Result<u32, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_u32()
    }

    pub(crate) fn root_as_u64(&self, root: RootedPtr) -> Result<u64, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_u64()
    }

    pub(crate) fn root_as_i8(&self, root: RootedPtr) -> Result<i8, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_i8()
    }

    pub(crate) fn root_as_i16(&self, root: RootedPtr) -> Result<i16, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_i16()
    }

    pub(crate) fn root_as_i32(&self, root: RootedPtr) -> Result<i32, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_i32()
    }

    pub(crate) fn root_as_i64(&self, root: RootedPtr) -> Result<i64, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_i64()
    }

    pub(crate) fn root_as_f32(&self, root: RootedPtr) -> Result<f32, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_f32()
    }

    pub(crate) fn root_as_f64(&self, root: RootedPtr) -> Result<f64, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_f64()
    }

    pub(crate) fn root_as_string(&self, root: RootedPtr) -> Result<String, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_string()
    }

    pub(crate) fn root_as_uuid(&self, root: RootedPtr) -> Result<Uuid, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_uuid()
    }

    pub(crate) fn root_as_hash(&self, root: RootedPtr) -> Result<Hash, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_hash()
    }

    pub(crate) fn root_as_datetime(&self, root: RootedPtr) -> Result<DateTime<Utc>, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_datetime()
    }

    pub(crate) fn root_as_tuple(&mut self, root: RootedPtr) -> Result<Vec<RootedPtr>, EngineError> {
        Ok(self
            .get_cell_from_rooted_ptr(root)?
            .cell_as_tuple()?
            .into_iter()
            .map(|x| self.root(x))
            .collect())
    }

    pub(crate) fn root_as_dict(
        &mut self,
        root: RootedPtr,
    ) -> Result<BTreeMap<String, RootedPtr>, EngineError> {
        let dict: BTreeMap<String, InternalPtr> =
            self.get_cell_from_rooted_ptr(root)?.cell_as_dict()?;
        Ok(BTreeMap::from_iter(
            dict.into_iter().map(|(k, v)| (k, self.root(v))),
        ))
    }

    pub(crate) fn root_as_adt(
        &mut self,
        root: RootedPtr,
    ) -> Result<(Symbol, Vec<RootedPtr>), EngineError> {
        let (sym, fields) = self.get_cell_from_rooted_ptr(root)?.cell_as_adt()?;
        let fields = fields.into_iter().map(|x| self.root(x)).collect();
        Ok((sym, fields))
    }

    pub(crate) fn root_as_list(&mut self, root: RootedPtr) -> Result<Vec<RootedPtr>, EngineError> {
        let elements = list_elements_from_pointer(self.heap, self.pointer(root))?;
        materialize_list_elements(self, elements)
    }

    pub(crate) fn root_as_binary_list(&mut self, root: RootedPtr) -> Result<Vec<u8>, EngineError> {
        collect_list_u8(self.heap, &self.pointer(root))
    }

    pub(crate) fn root_as_callable(
        &mut self,
        root: RootedPtr,
    ) -> Result<Option<RootedCallable>, EngineError> {
        let cell = self.get_cell_from_rooted_ptr(root)?.clone();
        Ok(match cell {
            Cell::Closure(Closure {
                env,
                param,
                param_ty,
                typ,
                body,
            }) => Some(RootedCallable::Closure(RootedClosure {
                env: env.to_scoped(self),
                param,
                param_ty,
                typ,
                body,
            })),
            Cell::Native(native) => Some(RootedCallable::Native(NativeFn::from_parts(
                native.native_id,
                native.name,
                native.arity,
                native.typ,
                native
                    .applied
                    .into_iter()
                    .map(|value| self.root(value))
                    .collect(),
                native.applied_types,
            ))),
            Cell::Overloaded(overloaded) => {
                Some(RootedCallable::Overloaded(OverloadedFn::from_parts(
                    overloaded.name,
                    overloaded.typ,
                    overloaded
                        .applied
                        .into_iter()
                        .map(|value| self.root(value))
                        .collect(),
                    overloaded.applied_types,
                )))
            }
            _ => None,
        })
    }

    pub(crate) fn root_as_native(
        &mut self,
        root: RootedPtr,
    ) -> Result<Option<NativeFn<RootedPtr>>, EngineError> {
        let cell = self.get_cell_from_rooted_ptr(root)?.clone();
        Ok(match cell {
            Cell::Native(native) => Some(NativeFn::from_parts(
                native.native_id,
                native.name,
                native.arity,
                native.typ,
                native
                    .applied
                    .into_iter()
                    .map(|value| self.root(value))
                    .collect(),
                native.applied_types,
            )),
            _ => None,
        })
    }

    pub(crate) fn overwrite_root(
        &mut self,
        target: RootedPtr,
        value: RootedPtr,
    ) -> Result<(), EngineError> {
        let cell = self.get_cell_from_rooted_ptr(value)?.clone();
        let target = self.pointer(target);
        self.heap.overwrite(&target, cell)
    }
}

/// Raw moving reference used only for edges owned by the collector.
///
/// An `InternalPtr` identifies a heap, slot, and heap-wide collection epoch. It
/// may be stored in [`Cell`] or used as a short-lived local while `Heap`
/// is exclusively borrowed. It must never cross an allocation that may
/// collect or an `await` point as an unrooted local. Copying collection
/// rewrites every traced cell edge; epoch and heap checks reject stale or
/// foreign pointers that escape that discipline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct InternalPtr {
    heap_id: u64,
    index: u32,
    generation: u64,
}

struct Reference<'a> {
    heap: &'a mut Heap,
    index: u32,
    generation: u64,
}

impl<'a> Reference<'a> {
    fn into_pointer(self) -> InternalPtr {
        InternalPtr {
            heap_id: self.heap.id,
            index: self.index,
            generation: self.generation,
        }
    }
}

#[derive(Clone)]
pub(super) enum Cell {
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
    Hash(Hash),
    DateTime(DateTime<Utc>),
    Tuple(Vec<InternalPtr>),
    Empty,
    Cons(InternalPtr, InternalPtr),
    ListSlice {
        start: usize,
        end: usize,
        elements: InternalPtr,
    },
    Data(Vec<InternalPtr>),
    BinaryData(Vec<u8>),
    Dict(BTreeMap<String, InternalPtr>),
    Adt(Symbol, Vec<InternalPtr>),
    Uninitialized(Symbol),
    Closure(Closure),
    Native(NativeFn<InternalPtr>),
    Overloaded(OverloadedFn<InternalPtr>),
}

impl Cell {
    pub(super) fn cell_type_name(&self) -> &'static str {
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
            Cell::Hash(..) => "hash",
            Cell::DateTime(..) => "datetime",
            Cell::Tuple(..) => "tuple",
            Cell::Empty | Cell::Cons(..) | Cell::ListSlice { .. } => "list",
            Cell::Data(..) => "data",
            Cell::BinaryData(..) => "binary_data",
            Cell::Dict(..) => "dict",
            Cell::Adt(..) => "adt",
            Cell::Uninitialized(name) => {
                let _ = name;
                "uninitialized"
            }
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

    pub(super) fn cell_as_bool(&self) -> Result<bool, EngineError> {
        match self {
            Cell::Bool(v) => Ok(*v),
            _ => Err(self.cell_type_error("bool")),
        }
    }

    pub(super) fn cell_as_u8(&self) -> Result<u8, EngineError> {
        match self {
            Cell::U8(v) => Ok(*v),
            _ => Err(self.cell_type_error("u8")),
        }
    }

    pub(super) fn cell_as_u16(&self) -> Result<u16, EngineError> {
        match self {
            Cell::U16(v) => Ok(*v),
            _ => Err(self.cell_type_error("u16")),
        }
    }

    pub(super) fn cell_as_u32(&self) -> Result<u32, EngineError> {
        match self {
            Cell::U32(v) => Ok(*v),
            _ => Err(self.cell_type_error("u32")),
        }
    }

    pub(super) fn cell_as_u64(&self) -> Result<u64, EngineError> {
        match self {
            Cell::U64(v) => Ok(*v),
            _ => Err(self.cell_type_error("u64")),
        }
    }

    pub(super) fn cell_as_i8(&self) -> Result<i8, EngineError> {
        match self {
            Cell::I8(v) => Ok(*v),
            _ => Err(self.cell_type_error("i8")),
        }
    }

    pub(super) fn cell_as_i16(&self) -> Result<i16, EngineError> {
        match self {
            Cell::I16(v) => Ok(*v),
            _ => Err(self.cell_type_error("i16")),
        }
    }

    pub(super) fn cell_as_i32(&self) -> Result<i32, EngineError> {
        match self {
            Cell::I32(v) => Ok(*v),
            _ => Err(self.cell_type_error("i32")),
        }
    }

    pub(super) fn cell_as_i64(&self) -> Result<i64, EngineError> {
        match self {
            Cell::I64(v) => Ok(*v),
            _ => Err(self.cell_type_error("i64")),
        }
    }

    pub(super) fn cell_as_f32(&self) -> Result<f32, EngineError> {
        match self {
            Cell::F32(v) => Ok(*v),
            _ => Err(self.cell_type_error("f32")),
        }
    }

    pub(super) fn cell_as_f64(&self) -> Result<f64, EngineError> {
        match self {
            Cell::F64(v) => Ok(*v),
            _ => Err(self.cell_type_error("f64")),
        }
    }

    pub(super) fn cell_as_string(&self) -> Result<String, EngineError> {
        match self {
            Cell::String(v) => Ok(v.clone()),
            _ => Err(self.cell_type_error("string")),
        }
    }

    pub(super) fn cell_as_uuid(&self) -> Result<Uuid, EngineError> {
        match self {
            Cell::Uuid(v) => Ok(*v),
            _ => Err(self.cell_type_error("uuid")),
        }
    }

    pub(super) fn cell_as_hash(&self) -> Result<Hash, EngineError> {
        match self {
            Cell::Hash(v) => Ok(*v),
            _ => Err(self.cell_type_error("hash")),
        }
    }

    pub(super) fn cell_as_datetime(&self) -> Result<DateTime<Utc>, EngineError> {
        match self {
            Cell::DateTime(v) => Ok(*v),
            _ => Err(self.cell_type_error("datetime")),
        }
    }

    pub(super) fn cell_as_tuple(&self) -> Result<Vec<InternalPtr>, EngineError> {
        match self {
            Cell::Tuple(v) => Ok(v.clone()),
            _ => Err(self.cell_type_error("tuple")),
        }
    }

    pub(super) fn cell_as_data(&self) -> Result<Vec<InternalPtr>, EngineError> {
        match self {
            Cell::Data(v) => Ok(v.clone()),
            _ => Err(self.cell_type_error("data")),
        }
    }

    pub(super) fn cell_as_dict(&self) -> Result<BTreeMap<String, InternalPtr>, EngineError> {
        match self {
            Cell::Dict(v) => Ok(v.clone()),
            _ => Err(self.cell_type_error("dict")),
        }
    }

    pub(super) fn cell_as_adt(&self) -> Result<(Symbol, Vec<InternalPtr>), EngineError> {
        match self {
            Cell::Adt(name, args) => Ok((name.clone(), args.clone())),
            _ => Err(self.cell_type_error("adt")),
        }
    }
}

fn infer_cell_type(heap: &Heap, cell: &Cell) -> Result<Type, EngineError> {
    let pointer_type = |pointer: &InternalPtr| -> Result<Type, EngineError> {
        infer_cell_type(heap, heap.get_cell_from_pointer(pointer)?)
    };

    match cell {
        Cell::Bool(..) => Ok(Type::builtin(BuiltinTypeId::Bool)),
        Cell::U8(..) => Ok(Type::builtin(BuiltinTypeId::U8)),
        Cell::U16(..) => Ok(Type::builtin(BuiltinTypeId::U16)),
        Cell::U32(..) => Ok(Type::builtin(BuiltinTypeId::U32)),
        Cell::U64(..) => Ok(Type::builtin(BuiltinTypeId::U64)),
        Cell::I8(..) => Ok(Type::builtin(BuiltinTypeId::I8)),
        Cell::I16(..) => Ok(Type::builtin(BuiltinTypeId::I16)),
        Cell::I32(..) => Ok(Type::builtin(BuiltinTypeId::I32)),
        Cell::I64(..) => Ok(Type::builtin(BuiltinTypeId::I64)),
        Cell::F32(..) => Ok(Type::builtin(BuiltinTypeId::F32)),
        Cell::F64(..) => Ok(Type::builtin(BuiltinTypeId::F64)),
        Cell::String(..) => Ok(Type::builtin(BuiltinTypeId::String)),
        Cell::Uuid(..) => Ok(Type::builtin(BuiltinTypeId::Uuid)),
        Cell::Hash(..) => Ok(Type::builtin(BuiltinTypeId::Hash)),
        Cell::DateTime(..) => Ok(Type::builtin(BuiltinTypeId::DateTime)),
        Cell::Tuple(elems) => {
            let mut tys = Vec::with_capacity(elems.len());
            for elem in elems {
                tys.push(pointer_type(elem)?);
            }
            Ok(Type::tuple(tys))
        }
        Cell::Empty => Err(EngineError::UnknownType(Symbol::intern("list"))),
        Cell::Cons(head, _tail) => {
            let elem_ty = pointer_type(head)?;
            Ok(Type::app(Type::builtin(BuiltinTypeId::List), elem_ty))
        }
        Cell::ListSlice {
            start,
            end,
            elements,
        } => {
            let elements_cell = heap.get_cell_from_pointer(elements)?;
            match elements_cell {
                Cell::Data(elems) => {
                    if *start > *end || *end > elems.len() {
                        return Err(EngineError::NativeType {
                            expected: format!("valid list slice within len {}", elems.len()),
                            got: format!("start {start}, end {end}"),
                        });
                    }
                    let first = elems
                        .get(*start)
                        .ok_or_else(|| EngineError::UnknownType(Symbol::intern("list")))?;
                    let elem_ty = pointer_type(first)?;
                    for elem in elems.iter().take(*end).skip(*start + 1) {
                        let ty = pointer_type(elem)?;
                        if ty != elem_ty {
                            return Err(EngineError::NativeType {
                                expected: elem_ty.to_string(),
                                got: ty.to_string(),
                            });
                        }
                    }
                    Ok(Type::app(Type::builtin(BuiltinTypeId::List), elem_ty))
                }
                Cell::BinaryData(bytes) => {
                    if *start > *end || *end > bytes.len() {
                        return Err(EngineError::NativeType {
                            expected: format!("valid binary list slice within len {}", bytes.len()),
                            got: format!("start {start}, end {end}"),
                        });
                    }
                    if start == end {
                        return Err(EngineError::UnknownType(Symbol::intern("list")));
                    }
                    Ok(Type::list(Type::builtin(BuiltinTypeId::U8)))
                }
                _ => Err(EngineError::NativeType {
                    expected: "list slice backing data".into(),
                    got: elements_cell.cell_type_name().into(),
                }),
            }
        }
        Cell::Data(..) | Cell::BinaryData(..) => {
            Err(EngineError::UnknownType(Symbol::intern(match cell {
                Cell::Data(..) => "data",
                Cell::BinaryData(..) => "binary_data",
                _ => unreachable!(),
            })))
        }
        Cell::Dict(map) => {
            let first = map
                .values()
                .next()
                .ok_or_else(|| EngineError::UnknownType(Symbol::intern("dict")))?;
            let elem_ty = pointer_type(first)?;
            for val in map.values().skip(1) {
                let ty = pointer_type(val)?;
                if ty != elem_ty {
                    return Err(EngineError::NativeType {
                        expected: elem_ty.to_string(),
                        got: ty.to_string(),
                    });
                }
            }
            Ok(Type::app(Type::builtin(BuiltinTypeId::Dict), elem_ty))
        }
        Cell::Adt(tag, args) if tag.as_ref() == "Some" && args.len() == 1 => {
            let inner = pointer_type(&args[0])?;
            Ok(Type::app(Type::builtin(BuiltinTypeId::Option), inner))
        }
        Cell::Adt(tag, args) if tag.as_ref() == "None" && args.is_empty() => {
            Err(EngineError::UnknownType(Symbol::intern("option")))
        }
        Cell::Adt(tag, args)
            if (tag.as_ref() == "Ok" || tag.as_ref() == "Err") && args.len() == 1 =>
        {
            Err(EngineError::UnknownType(Symbol::intern("result")))
        }
        Cell::Adt(tag, _args) => Err(EngineError::UnknownType(tag.clone())),
        Cell::Uninitialized(..) => Err(EngineError::UnknownType(Symbol::intern("uninitialized"))),
        Cell::Closure(..) => Err(EngineError::UnknownType(Symbol::intern("closure"))),
        Cell::Native(..) => Err(EngineError::UnknownType(Symbol::intern("native"))),
        Cell::Overloaded(..) => Err(EngineError::UnknownType(Symbol::intern("overloaded"))),
    }
}

impl Collection for Cell {
    fn map_pointers<E>(
        &mut self,
        map: &mut impl FnMut(InternalPtr) -> Result<InternalPtr, E>,
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
            Cell::Closure(closure) => closure.env.map_pointers(map),
            Cell::Native(native) => {
                for pointer in &mut native.applied {
                    *pointer = map(*pointer)?;
                }
                Ok(())
            }
            Cell::Overloaded(overloaded) => {
                for pointer in &mut overloaded.applied {
                    *pointer = map(*pointer)?;
                }
                Ok(())
            }
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
            | Cell::Hash(_)
            | Cell::DateTime(_)
            | Cell::BinaryData(_)
            | Cell::Empty
            | Cell::Uninitialized(_) => Ok(()),
        }
    }
}

pub(super) type InternalPtrKey = (u64, u32, u64);

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

fn pointer_key(pointer: &InternalPtr) -> InternalPtrKey {
    (pointer.heap_id, pointer.index, pointer.generation)
}

fn invalid_pointer(heap_id: u64, index: u32, generation: u64) -> EngineError {
    EngineError::Internal(format!(
        "invalid heap pointer (heap_id={}, index={}, generation={})",
        heap_id, index, generation
    ))
}

pub(super) fn wrong_heap_pointer(
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

fn randomized_gc_destinations(old_indices: &[u32], completed_collections: u64) -> Vec<usize> {
    let len = old_indices.len();
    let mut destinations = (0..len).collect::<Vec<_>>();
    let mut random_state = completed_collections
        .wrapping_add(1)
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ (len as u64).wrapping_mul(0xbf58_476d_1ce4_e5b9);

    for index in (1..len).rev() {
        random_state = random_state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut random = random_state;
        random = (random ^ (random >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        random = (random ^ (random >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        random ^= random >> 31;
        let swap_index = (random % (index as u64 + 1)) as usize;
        destinations.swap(index, swap_index);
    }

    // Repair accidental fixed points against the objects' actual old slots,
    // rather than their positions in the tracing order. Rotating two or more
    // fixed destinations cannot create another fixed point because old slot
    // indices are unique. A lone fixed point can swap with any other object.
    let fixed = destinations
        .iter()
        .enumerate()
        .filter_map(|(live_index, destination)| {
            (*destination == old_indices[live_index] as usize).then_some(live_index)
        })
        .collect::<Vec<_>>();
    match fixed.as_slice() {
        [fixed] if len > 1 => {
            destinations.swap(*fixed, (fixed + 1) % len);
        }
        [_, _, ..] => {
            let mut fixed_destinations = fixed
                .iter()
                .map(|index| destinations[*index])
                .collect::<Vec<_>>();
            fixed_destinations.rotate_left(1);
            for (index, destination) in fixed.into_iter().zip(fixed_destinations) {
                destinations[index] = destination;
            }
        }
        _ => {}
    }

    destinations
}

#[cfg(test)]
mod tests {
    use super::*;
    use static_assertions::assert_impl_all;

    assert_impl_all!(Heap: Send);
    assert_impl_all!(RootedPtr: Send, Sync);

    #[test]
    fn copying_gc_relocates_runtime_roots() {
        let mut heap = Heap::new();
        let root = heap
            .machine_root_scope(|scope| scope.alloc_root_i32(42))
            .expect("value should allocate");
        let before = heap.resolve_runtime_root(root);

        heap.collect().expect("collection should succeed");

        let after = heap.resolve_runtime_root(root);
        assert_ne!(before.generation, after.generation);
        heap.machine_root_scope(|scope| {
            assert_eq!(scope.root_as_i32(root).expect("root should relocate"), 42);
        });
        assert!(heap.get_cell_from_pointer(&before).is_err());
    }

    #[test]
    fn failed_collection_releases_allocation_roots() {
        let mut heap = Heap::new();
        let child = heap
            .push_cell(Cell::I32(42))
            .expect("child should allocate")
            .into_pointer();
        heap.set_gc_slot_threshold(1);
        heap.collection_epoch = u64::MAX;

        let error = match heap.alloc_reference(Cell::Tuple(vec![child])) {
            Ok(_) => panic!("collection epoch exhaustion should fail"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("collection count exhausted"));
        assert!(
            heap.runtime_roots.iter().all(|slot| slot.pointer.is_none()),
            "temporary allocation roots must be released"
        );
    }

    #[test]
    fn extreme_stress_relocates_every_live_root() {
        let mut heap = Heap::new();
        let roots = heap
            .machine_root_scope(|scope| {
                (0..4)
                    .map(|value| scope.alloc_root_i32(value))
                    .collect::<Result<Vec<_>, _>>()
            })
            .expect("values should allocate");
        let before = roots
            .iter()
            .map(|root| heap.resolve_runtime_root(*root))
            .collect::<Vec<_>>();

        heap.set_extreme_stress(true);
        heap.collect().expect("collection should succeed");

        let after = roots
            .iter()
            .map(|root| heap.resolve_runtime_root(*root))
            .collect::<Vec<_>>();
        assert!(
            before
                .iter()
                .zip(&after)
                .all(|(before, after)| before.index != after.index)
        );
        heap.machine_root_scope(|scope| {
            for (expected, root) in roots.iter().enumerate() {
                assert_eq!(
                    scope.root_as_i32(*root).expect("root should relocate"),
                    expected as i32
                );
            }
        });
    }
}
