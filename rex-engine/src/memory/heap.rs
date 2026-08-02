//! Heap storage, roots, handles, cells, and public value views.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::fmt;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{DateTime, Utc};
use rex_ast::Symbol;
use rex_typesystem::types::{BuiltinTypeId, Type, TypedExpr};
use uuid::Uuid;

use crate::{
    EngineError, env::ScopedEnvironment, native_fn::NativeFn, overloaded_fn::OverloadedFn,
};

use super::{
    handle_promotion::with_promotable_root_scope,
    lists::{
        ListElement, ListItemsSeed, ListRootElement, ListRootedItems, collect_list_u8,
        format_list_debug, format_list_display, list_cells_eq_inner, list_elements_from_pointer,
        list_elements_to_rooted_ptr_vec, list_items_from_pointer, list_len_from_pointer,
        list_slice_backing_len, list_slice_head_element, materialize_list_elements,
        validate_list_slice_bounds,
    },
    traits::FromRex,
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
    fn parent(&self) -> Option<&InternalEnvironment> {
        self.0.parent.as_ref()
    }

    fn bindings(&self) -> &BTreeMap<Symbol, InternalPtr> {
        &self.0.bindings
    }

    fn from_scoped<'scope>(env: &ScopedEnvironment<'scope>, scope: &RootScope<'_, 'scope>) -> Self {
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

    fn to_scoped<'scope>(&self, scope: &mut RootScope<'_, 'scope>) -> ScopedEnvironment<'scope> {
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

pub(crate) struct RootedClosure<'scope> {
    pub(crate) env: ScopedEnvironment<'scope>,
    pub(crate) param: Symbol,
    pub(crate) param_ty: Type,
    pub(crate) typ: Type,
    pub(crate) body: Arc<TypedExpr>,
}

pub(crate) enum RootedCallable<'scope> {
    Closure(RootedClosure<'scope>),
    Native(NativeFn<RootedPtr<'scope>>),
    Overloaded(OverloadedFn<RootedPtr<'scope>>),
}

/// Mutable storage and collector state for one Rex heap.
///
/// `HeapState` is accessible only while the owning [`Heap`] mutex is locked.
/// Exclusive `&mut HeapState` access is the underlying proof that a synchronous
/// operation has sole access to the collector; evaluator code receives that
/// proof through [`RootScope`] rather than receiving `Heap` or locking again.
/// A scope must never invoke host code, block, or cross an `await` boundary.
///
/// The collector recognizes three kinds of edges:
///
/// - [`InternalPtr`] values occur only in heap cells and short-lived locals.
///   Collection traces and rewrites every reachable internal edge.
/// - The temporary-root stack backs [`RootedPtr`] values used during one locked
///   synchronous scope. Collection rewrites these stack entries.
/// - Registered root slots back [`Handle`] values and the roots owned by
///   [`PersistentRootStore`]. Handles are used at public, host, and outer
///   runtime boundaries. Collection rewrites the slots, while their
///   generational identifiers remain stable.
///
/// Any allocation can collect before creating its result. An `InternalPtr`
/// local therefore cannot survive an allocation unless it has first been
/// placed in a traced cell or converted to one of the rooted representations.
pub(super) struct HeapState {
    id: u64,
    slots: Vec<HeapSlot>,
    root_slots: Vec<RootSlot>,
    temporary_roots: Vec<InternalPtr>,
    free_root_list: Vec<u64>,
    next_persistent_store_id: u64,
    next_gc_slot_count: usize,
    collect_on_every_alloc: bool,
    collection_epoch: u64,
}

const DEFAULT_GC_SLOT_THRESHOLD: usize = 4_096;
const GC_SLOT_GROWTH_NUMERATOR: usize = 3;
const GC_SLOT_GROWTH_DENOMINATOR: usize = 2;
const GC_EXTREME_STRESS: bool = false;

impl HeapState {
    fn new() -> Self {
        static NEXT_HEAP_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_HEAP_ID.fetch_add(1, Ordering::Relaxed);
        Self {
            id,
            slots: Vec::new(),
            root_slots: Vec::new(),
            temporary_roots: Vec::new(),
            free_root_list: Vec::new(),
            next_persistent_store_id: 0,
            next_gc_slot_count: DEFAULT_GC_SLOT_THRESHOLD,
            collect_on_every_alloc: GC_EXTREME_STRESS,
            collection_epoch: 0,
        }
    }

    fn collect_needed(&self) -> bool {
        self.collect_on_every_alloc || self.slots.len() >= self.next_gc_slot_count
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

    pub(crate) fn set_collect_on_every_alloc(&mut self, enabled: bool) {
        self.collect_on_every_alloc = enabled;
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

    pub(super) fn register_root(&mut self, pointer: InternalPtr) -> Result<RootId, EngineError> {
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

    pub(super) fn register_roots(
        &mut self,
        pointers: impl IntoIterator<Item = InternalPtr>,
    ) -> Result<Vec<RootId>, EngineError> {
        let mut root_ids = Vec::new();
        for pointer in pointers {
            match self.register_root(pointer) {
                Ok(root_id) => root_ids.push(root_id),
                Err(error) => {
                    if let Err(cleanup_error) = self.unregister_roots(root_ids) {
                        return Err(EngineError::Internal(format!(
                            "failed to register heap roots: {error}; cleanup also failed: {cleanup_error}"
                        )));
                    }
                    return Err(error);
                }
            }
        }
        Ok(root_ids)
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

    fn resolve_root(&self, root_id: RootId) -> Result<InternalPtr, EngineError> {
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
        let next_epoch = self
            .collection_epoch
            .checked_add(1)
            .ok_or_else(|| EngineError::Internal("heap collection count exhausted".into()))?;
        let mut forwarding = vec![None; self.slots.len()];
        let persistent_roots = self
            .root_slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| slot.pointer.map(|pointer| (index, pointer)))
            .collect::<Vec<_>>();
        let mut work = persistent_roots
            .iter()
            .map(|(_, pointer)| *pointer)
            .chain(self.temporary_roots.iter().copied())
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
        let destinations = if GC_EXTREME_STRESS {
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

        let relocated_persistent_roots = persistent_roots
            .iter()
            .map(|(index, pointer)| Ok((*index, self.forward_for_gc(*pointer, &forwarding)?)))
            .collect::<Result<Vec<_>, EngineError>>()?;
        let relocated_temporary_roots = self
            .temporary_roots
            .iter()
            .map(|pointer| self.forward_for_gc(*pointer, &forwarding))
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

        for (index, pointer) in relocated_persistent_roots {
            self.root_slots[index].pointer = Some(pointer);
        }
        self.temporary_roots = relocated_temporary_roots;
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

        for (root_index, pointer) in self.temporary_roots.iter().copied().enumerate() {
            self.get_slot_checked(&pointer).map_err(|err| {
                EngineError::Internal(format!(
                    "GC verification failed for temporary root {root_index}: {err}"
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

    fn get_cell_from_root(&self, root_id: RootId) -> Result<&Cell, EngineError> {
        let pointer = self.resolve_root(root_id)?;
        self.get_cell_from_pointer(&pointer)
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

    pub(super) fn type_name(&self, pointer: &InternalPtr) -> Result<&'static str, EngineError> {
        self.get_cell_from_pointer(pointer)
            .map(Cell::cell_type_name)
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

    pub(super) fn pointer_as_list(
        &mut self,
        pointer: &InternalPtr,
    ) -> Result<Vec<InternalPtr>, EngineError> {
        let elements = list_elements_from_pointer(self, *pointer)?;
        self.root_scope(|scope| {
            materialize_list_elements(scope, elements)
                .map(|roots| roots.into_iter().map(|r| scope.pointer(r)).collect())
        })
    }

    pub(super) fn root_scope<R>(
        &mut self,
        f: impl for<'scope> FnOnce(&mut RootScope<'_, 'scope>) -> R,
    ) -> R {
        let base = self.temporary_roots.len();

        let mut scope = RootScope {
            heap: self,
            base,
            _brand: PhantomData,
        };

        f(&mut scope)
    }

    pub(super) fn id(&self) -> u64 {
        self.id
    }

    pub(crate) fn persistent_root_store(&mut self) -> Result<PersistentRootStore, EngineError> {
        let store_id = self.next_persistent_store_id;
        self.next_persistent_store_id = self
            .next_persistent_store_id
            .checked_add(1)
            .ok_or_else(|| EngineError::Internal("persistent root store id exhausted".into()))?;
        Ok(PersistentRootStore {
            heap_id: self.id,
            store_id,
            slots: Vec::new(),
            free_slots: Vec::new(),
            live_count: 0,
        })
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct RootId {
    heap_id: u64,
    index: u64,
    generation: u64,
}

/// Opaque evaluator-owned reference to a registered heap root.
///
/// The token contains no raw heap location and no capability for acquiring the
/// heap mutex. It is resolved only through its owning [`PersistentRootStore`]
/// while a [`RootScope`] proves exclusive `HeapState` access. Evaluator state
/// may retain the token while the heap is unlocked, across `await` points, and
/// while its future moves between executor threads. Heap, store, slot, and
/// generation validation prevents a token from being resolved by the wrong
/// arena or after its root has been removed.
#[derive(Debug, PartialEq, Eq, Hash)]
pub(crate) struct PersistentPtr {
    heap_id: u64,
    store_id: u64,
    index: u64,
    generation: u64,
}

#[derive(Debug)]
struct PersistentRootSlot {
    generation: u64,
    root_id: Option<RootId>,
}

/// Invariant lifetime brand that also keeps synchronous roots on one thread.
type RootScopeBrand<'scope> = PhantomData<Rc<std::cell::Cell<&'scope ()>>>;

/// Evaluator-owned arena for values that survive between synchronous cycles.
///
/// Every live arena slot owns one registered heap root. Collection rewrites the
/// root slot rather than the [`PersistentPtr`] token. Registration,
/// duplication, replacement, and removal are explicit and must happen through
/// an active [`RootScope`]. The store deliberately has no destructor-based
/// cleanup because cleanup must not re-enter the heap mutex.
#[derive(Debug)]
pub(crate) struct PersistentRootStore {
    heap_id: u64,
    store_id: u64,
    slots: Vec<PersistentRootSlot>,
    free_slots: Vec<u64>,
    live_count: usize,
}

/// Temporary reference to a value during one locked synchronous operation.
///
/// A `RootedPtr` indexes the temporary-root stack owned by its [`RootScope`]; it
/// does not contain an [`InternalPtr`]. The collector rewrites the stack entry
/// whenever the value moves. The invariant lifetime brand prevents the token
/// from escaping the higher-ranked scope closure or being mixed with another
/// scope. It must not cross a mutex unlock, thread boundary, or `await` point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RootedPtr<'scope> {
    slot: usize,
    _brand: RootScopeBrand<'scope>,
}

/// Exclusive synchronous access to one locked [`HeapState`].
///
/// `RootScope` is the only general evaluator capability for inspecting and
/// allocating Rex values during a locked cycle. Its `&mut HeapState` proves
/// exclusive collector access, and its lifetime brand limits every
/// [`RootedPtr`] it creates to this scope. Allocation may collect, but the
/// collector rewrites the scope's temporary-root stack before control returns.
/// Dropping the scope restores the stack to its entry depth.
///
/// A scope deliberately contains neither a public [`Heap`] capability nor a
/// public handle owner. Locked code must not call host callbacks, block, await,
/// or perform an operation whose destructor can reacquire the heap mutex.
pub(crate) struct RootScope<'heap, 'scope> {
    pub(super) heap: &'heap mut HeapState,
    base: usize,
    _brand: RootScopeBrand<'scope>,
}

impl Drop for RootScope<'_, '_> {
    fn drop(&mut self) {
        debug_assert!(
            self.heap.temporary_roots.len() >= self.base,
            "root scope shadow stack underflow: base {}, length {}",
            self.base,
            self.heap.temporary_roots.len()
        );
        self.heap.temporary_roots.truncate(self.base);
    }
}

impl<'h, 'scope> RootScope<'h, 'scope> {
    pub(crate) fn root_handle(
        &mut self,
        handle: &Handle,
    ) -> Result<RootedPtr<'scope>, EngineError> {
        let pointer = handle.pointer(self.heap)?;
        Ok(self.root(pointer))
    }

    pub(crate) fn root_handles(
        &mut self,
        handles: &[Handle],
    ) -> Result<Vec<RootedPtr<'scope>>, EngineError> {
        handles
            .iter()
            .map(|handle| self.root_handle(handle))
            .collect()
    }

    pub(crate) fn persistent_root_store(&mut self) -> Result<PersistentRootStore, EngineError> {
        self.heap.persistent_root_store()
    }

    pub(super) fn root(&mut self, ptr: InternalPtr) -> RootedPtr<'scope> {
        // This operation itself must not invoke the language GC.
        let slot = self.heap.temporary_roots.len();
        self.heap.temporary_roots.push(ptr);

        RootedPtr {
            slot,
            _brand: PhantomData,
        }
    }

    pub(super) fn pointer(&self, root: RootedPtr<'scope>) -> InternalPtr {
        debug_assert!(
            root.slot >= self.base && root.slot < self.heap.temporary_roots.len(),
            "rooted pointer slot {} is outside active root scope {}..{}",
            root.slot,
            self.base,
            self.heap.temporary_roots.len()
        );
        self.heap.temporary_roots[root.slot]
    }

    pub(crate) fn type_name(&self, root: RootedPtr<'scope>) -> Result<&'static str, EngineError> {
        self.get_cell_from_rooted_ptr(root)
            .map(Cell::cell_type_name)
    }

    pub(crate) fn infer_type(&self, root: RootedPtr<'scope>) -> Result<Type, EngineError> {
        infer_cell_type(self.heap, self.get_cell_from_rooted_ptr(root)?)
    }

    pub(super) fn get_cell_from_rooted_ptr(
        &self,
        root: RootedPtr<'scope>,
    ) -> Result<&Cell, EngineError> {
        let pointer = self.pointer(root);
        self.heap.get_cell_from_pointer(&pointer)
    }

    pub(crate) fn list_items(
        &mut self,
        root: RootedPtr<'scope>,
    ) -> Result<ListRootedItems<'scope>, EngineError> {
        let pointer = self.pointer(root);
        match list_items_from_pointer(self.heap, pointer)? {
            ListItemsSeed::Ready(items) => Ok(items.into_list_rooted_items(self)),
            ListItemsSeed::Elements(elements) => {
                if elements
                    .iter()
                    .all(|element| matches!(element, ListElement::InternalPtr(_)))
                {
                    return list_elements_to_rooted_ptr_vec(self, elements)
                        .map(ListRootedItems::Pointers);
                }
                materialize_list_elements(self, elements).map(ListRootedItems::Pointers)
            }
        }
    }

    fn alloc_reference<'a>(&'a mut self, cell: Cell) -> Result<RootedPtr<'scope>, EngineError> {
        let reference = self.heap.alloc_reference(cell)?;
        let pointer = reference.into_pointer();
        Ok(self.root(pointer))
    }

    pub(crate) fn alloc_root_bool(
        &mut self,
        value: bool,
    ) -> Result<RootedPtr<'scope>, EngineError> {
        self.alloc_reference(Cell::Bool(value))
    }

    pub(crate) fn alloc_root_u8(&mut self, value: u8) -> Result<RootedPtr<'scope>, EngineError> {
        self.alloc_reference(Cell::U8(value))
    }

    pub(crate) fn alloc_root_u16(&mut self, value: u16) -> Result<RootedPtr<'scope>, EngineError> {
        self.alloc_reference(Cell::U16(value))
    }

    pub(crate) fn alloc_root_u32(&mut self, value: u32) -> Result<RootedPtr<'scope>, EngineError> {
        self.alloc_reference(Cell::U32(value))
    }

    pub(crate) fn alloc_root_u64(&mut self, value: u64) -> Result<RootedPtr<'scope>, EngineError> {
        self.alloc_reference(Cell::U64(value))
    }

    pub(crate) fn alloc_root_i8(&mut self, value: i8) -> Result<RootedPtr<'scope>, EngineError> {
        self.alloc_reference(Cell::I8(value))
    }

    pub(crate) fn alloc_root_i16(&mut self, value: i16) -> Result<RootedPtr<'scope>, EngineError> {
        self.alloc_reference(Cell::I16(value))
    }

    pub(crate) fn alloc_root_i32(&mut self, value: i32) -> Result<RootedPtr<'scope>, EngineError> {
        self.alloc_reference(Cell::I32(value))
    }

    pub(crate) fn alloc_root_i64(&mut self, value: i64) -> Result<RootedPtr<'scope>, EngineError> {
        self.alloc_reference(Cell::I64(value))
    }

    pub(crate) fn alloc_root_f32(&mut self, value: f32) -> Result<RootedPtr<'scope>, EngineError> {
        self.alloc_reference(Cell::F32(value))
    }

    pub(crate) fn alloc_root_f64(&mut self, value: f64) -> Result<RootedPtr<'scope>, EngineError> {
        self.alloc_reference(Cell::F64(value))
    }

    pub(crate) fn alloc_root_string(
        &mut self,
        value: String,
    ) -> Result<RootedPtr<'scope>, EngineError> {
        self.alloc_reference(Cell::String(value))
    }

    pub(crate) fn alloc_root_uuid(
        &mut self,
        value: Uuid,
    ) -> Result<RootedPtr<'scope>, EngineError> {
        self.alloc_reference(Cell::Uuid(value))
    }

    pub(crate) fn alloc_root_datetime(
        &mut self,
        value: DateTime<Utc>,
    ) -> Result<RootedPtr<'scope>, EngineError> {
        self.alloc_reference(Cell::DateTime(value))
    }

    pub(crate) fn alloc_root_uninitialized(
        &mut self,
        name: Symbol,
    ) -> Result<RootedPtr<'scope>, EngineError> {
        self.alloc_reference(Cell::Uninitialized(name))
    }

    pub(crate) fn alloc_root_tuple(
        &mut self,
        values: Vec<RootedPtr<'scope>>,
    ) -> Result<RootedPtr<'scope>, EngineError> {
        let values = values.into_iter().map(|v| self.pointer(v)).collect();
        self.alloc_reference(Cell::Tuple(values))
    }

    pub(crate) fn alloc_root_dict(
        &mut self,
        values: BTreeMap<Symbol, RootedPtr<'scope>>,
    ) -> Result<RootedPtr<'scope>, EngineError> {
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
        args: Vec<RootedPtr<'scope>>,
    ) -> Result<RootedPtr<'scope>, EngineError> {
        let args = args.into_iter().map(|arg| self.pointer(arg)).collect();
        self.alloc_reference(Cell::Adt(name, args))
    }

    pub(crate) fn alloc_root_empty(&mut self) -> Result<RootedPtr<'scope>, EngineError> {
        self.alloc_reference(Cell::Empty)
    }

    pub(crate) fn alloc_root_cons(
        &mut self,
        head: RootedPtr<'scope>,
        tail: RootedPtr<'scope>,
    ) -> Result<RootedPtr<'scope>, EngineError> {
        let head = self.pointer(head);
        let tail = self.pointer(tail);
        self.alloc_reference(Cell::Cons(head, tail))
    }

    pub(crate) fn alloc_root_data(
        &mut self,
        values: Vec<RootedPtr<'scope>>,
    ) -> Result<RootedPtr<'scope>, EngineError> {
        let values = values.into_iter().map(|v| self.pointer(v)).collect();
        self.alloc_reference(Cell::Data(values))
    }

    pub(crate) fn alloc_root_binary_data(
        &mut self,
        values: Vec<u8>,
    ) -> Result<RootedPtr<'scope>, EngineError> {
        self.alloc_reference(Cell::BinaryData(values))
    }

    pub(crate) fn alloc_root_closure(
        &mut self,
        env: ScopedEnvironment<'scope>,
        param: Symbol,
        param_ty: Type,
        typ: Type,
        body: Arc<TypedExpr>,
    ) -> Result<RootedPtr<'scope>, EngineError> {
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
        applied: Vec<RootedPtr<'scope>>,
        applied_types: Vec<Type>,
    ) -> Result<RootedPtr<'scope>, EngineError> {
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
        applied: Vec<RootedPtr<'scope>>,
        applied_types: Vec<Type>,
    ) -> Result<RootedPtr<'scope>, EngineError> {
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
        elements: RootedPtr<'scope>,
    ) -> Result<RootedPtr<'scope>, EngineError> {
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
        pointer: RootedPtr<'scope>,
    ) -> Result<Option<(RootedPtr<'scope>, RootedPtr<'scope>)>, EngineError> {
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
        let elements: RootedPtr<'_> = c;

        let head: RootedPtr<'_> = match head {
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
        let tail: RootedPtr<'_> = match tail_index {
            Some((start, end)) => {
                let tail = scope.alloc_root_list_slice(start, end, elements)?;
                return Ok(Some((head, tail)));
            }
            None => elements,
        };
        Ok(Some((head, tail)))
    }

    pub(crate) fn list_len(&self, root: RootedPtr<'scope>) -> Result<usize, EngineError> {
        let pointer = self.pointer(root);
        list_len_from_pointer(self.heap, pointer)
    }

    pub(crate) fn alloc_root_list(
        &mut self,
        values: Vec<RootedPtr<'scope>>,
    ) -> Result<RootedPtr<'scope>, EngineError> {
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
    ) -> Result<RootedPtr<'scope>, EngineError> {
        if values.is_empty() {
            return self.alloc_root_empty();
        }
        let len = values.len();
        let data = self.alloc_root_binary_data(values)?;
        self.alloc_root_list_slice(0, len, data)
    }

    pub(crate) fn root_as_bool(&self, root: RootedPtr<'scope>) -> Result<bool, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_bool()
    }

    pub(crate) fn root_as_u8(&self, root: RootedPtr<'scope>) -> Result<u8, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_u8()
    }

    pub(crate) fn root_as_u16(&self, root: RootedPtr<'scope>) -> Result<u16, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_u16()
    }

    pub(crate) fn root_as_u32(&self, root: RootedPtr<'scope>) -> Result<u32, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_u32()
    }

    pub(crate) fn root_as_u64(&self, root: RootedPtr<'scope>) -> Result<u64, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_u64()
    }

    pub(crate) fn root_as_i8(&self, root: RootedPtr<'scope>) -> Result<i8, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_i8()
    }

    pub(crate) fn root_as_i16(&self, root: RootedPtr<'scope>) -> Result<i16, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_i16()
    }

    pub(crate) fn root_as_i32(&self, root: RootedPtr<'scope>) -> Result<i32, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_i32()
    }

    pub(crate) fn root_as_i64(&self, root: RootedPtr<'scope>) -> Result<i64, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_i64()
    }

    pub(crate) fn root_as_f32(&self, root: RootedPtr<'scope>) -> Result<f32, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_f32()
    }

    pub(crate) fn root_as_f64(&self, root: RootedPtr<'scope>) -> Result<f64, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_f64()
    }

    pub(crate) fn root_as_string(&self, root: RootedPtr<'scope>) -> Result<String, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_string()
    }

    pub(crate) fn root_as_uuid(&self, root: RootedPtr<'scope>) -> Result<Uuid, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_uuid()
    }

    pub(crate) fn root_as_datetime(
        &self,
        root: RootedPtr<'scope>,
    ) -> Result<DateTime<Utc>, EngineError> {
        self.get_cell_from_rooted_ptr(root)?.cell_as_datetime()
    }

    pub(crate) fn root_as_tuple(
        &mut self,
        root: RootedPtr<'scope>,
    ) -> Result<Vec<RootedPtr<'scope>>, EngineError> {
        Ok(self
            .get_cell_from_rooted_ptr(root)?
            .cell_as_tuple()?
            .into_iter()
            .map(|x| self.root(x))
            .collect())
    }

    pub(crate) fn root_as_dict(
        &mut self,
        root: RootedPtr<'scope>,
    ) -> Result<BTreeMap<Symbol, RootedPtr<'scope>>, EngineError> {
        let dict: BTreeMap<Symbol, InternalPtr> =
            self.get_cell_from_rooted_ptr(root)?.cell_as_dict()?;
        Ok(BTreeMap::from_iter(
            dict.into_iter().map(|(k, v)| (k, self.root(v))),
        ))
    }

    pub(crate) fn root_as_adt(
        &mut self,
        root: RootedPtr<'scope>,
    ) -> Result<(Symbol, Vec<RootedPtr<'scope>>), EngineError> {
        let (sym, fields) = self.get_cell_from_rooted_ptr(root)?.cell_as_adt()?;
        let fields = fields.into_iter().map(|x| self.root(x)).collect();
        Ok((sym, fields))
    }

    pub(crate) fn root_as_list(
        &mut self,
        root: RootedPtr<'scope>,
    ) -> Result<Vec<RootedPtr<'scope>>, EngineError> {
        let elements = list_elements_from_pointer(self.heap, self.pointer(root))?;
        materialize_list_elements(self, elements)
    }

    pub(crate) fn root_as_callable(
        &mut self,
        root: RootedPtr<'scope>,
    ) -> Result<Option<RootedCallable<'scope>>, EngineError> {
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
        root: RootedPtr<'scope>,
    ) -> Result<Option<NativeFn<RootedPtr<'scope>>>, EngineError> {
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
        target: RootedPtr<'scope>,
        value: RootedPtr<'scope>,
    ) -> Result<(), EngineError> {
        let cell = self.get_cell_from_rooted_ptr(value)?.clone();
        let target = self.pointer(target);
        self.heap.overwrite(&target, cell)
    }
}

impl PersistentRootStore {
    fn validate_heap(&self, heap: &HeapState) -> Result<(), EngineError> {
        if self.heap_id != heap.id {
            return Err(EngineError::Internal(format!(
                "persistent root store belongs to heap {}, not heap {}",
                self.heap_id, heap.id
            )));
        }
        Ok(())
    }

    fn root_id(
        &self,
        heap: &HeapState,
        value: &PersistentPtr,
    ) -> Result<(usize, RootId), EngineError> {
        self.validate_heap(heap)?;
        if value.heap_id != self.heap_id || value.store_id != self.store_id {
            return Err(invalid_persistent_ptr(value));
        }
        let index = usize::try_from(value.index)
            .map_err(|_| EngineError::Internal("persistent root index overflow".into()))?;
        let slot = self
            .slots
            .get(index)
            .ok_or_else(|| invalid_persistent_ptr(value))?;
        if slot.generation != value.generation {
            return Err(invalid_persistent_ptr(value));
        }
        let root_id = slot.root_id.ok_or_else(|| invalid_persistent_ptr(value))?;
        Ok((index, root_id))
    }

    fn install_root_id(&mut self, root_id: RootId) -> Result<PersistentPtr, EngineError> {
        let next_live_count = self
            .live_count
            .checked_add(1)
            .ok_or_else(|| EngineError::Internal("persistent root count exhausted".into()))?;

        let (index, generation) = if let Some(index) = self.free_slots.last().copied() {
            let slot_index = usize::try_from(index)
                .map_err(|_| EngineError::Internal("persistent root index overflow".into()))?;
            let slot = self.slots.get(slot_index).ok_or_else(|| {
                EngineError::Internal("persistent root free-list corruption".into())
            })?;
            if slot.root_id.is_some() {
                return Err(EngineError::Internal(
                    "persistent root free-list referenced a live slot".into(),
                ));
            }
            let generation = slot.generation;
            self.free_slots.pop();
            self.slots[slot_index].root_id = Some(root_id);
            (index, generation)
        } else {
            let index = u64::try_from(self.slots.len())
                .map_err(|_| EngineError::Internal("persistent root arena exhausted".into()))?;
            self.slots.push(PersistentRootSlot {
                generation: 0,
                root_id: Some(root_id),
            });
            (index, 0)
        };

        self.live_count = next_live_count;
        Ok(PersistentPtr {
            heap_id: self.heap_id,
            store_id: self.store_id,
            index,
            generation,
        })
    }

    fn cleanup_failed_install(
        heap: &mut HeapState,
        root_id: RootId,
        error: EngineError,
    ) -> EngineError {
        match heap.unregister_root(root_id) {
            Ok(()) => error,
            Err(cleanup_error) => EngineError::Internal(format!(
                "failed to install persistent root: {error}; cleanup also failed: {cleanup_error}"
            )),
        }
    }

    /// Register a rooted synchronous value for use after the current scope.
    pub(crate) fn insert<'heap, 'scope>(
        &mut self,
        scope: &mut RootScope<'heap, 'scope>,
        value: RootedPtr<'scope>,
    ) -> Result<PersistentPtr, EngineError> {
        self.validate_heap(scope.heap)?;
        let pointer = scope.pointer(value);
        let root_id = scope.heap.register_root(pointer)?;
        self.install_root_id(root_id)
            .map_err(|error| Self::cleanup_failed_install(scope.heap, root_id, error))
    }

    /// Resolve a persistent value into the active scope's shadow-root stack.
    pub(crate) fn resolve<'heap, 'scope>(
        &self,
        scope: &mut RootScope<'heap, 'scope>,
        value: &PersistentPtr,
    ) -> Result<RootedPtr<'scope>, EngineError> {
        let (_, root_id) = self.root_id(scope.heap, value)?;
        let pointer = scope.heap.resolve_root(root_id)?;
        Ok(scope.root(pointer))
    }

    /// Create an independently owned persistent root for the same value.
    #[allow(dead_code)] // Part of the arena API; the cycle migration currently rebuilds arenas.
    pub(crate) fn duplicate<'heap, 'scope>(
        &mut self,
        scope: &mut RootScope<'heap, 'scope>,
        value: &PersistentPtr,
    ) -> Result<PersistentPtr, EngineError> {
        let rooted = self.resolve(scope, value)?;
        self.insert(scope, rooted)
    }

    /// Replace one persistent value, rooting the replacement before release.
    #[allow(dead_code)] // Part of the arena API; the cycle migration currently rebuilds arenas.
    pub(crate) fn replace<'heap, 'scope>(
        &mut self,
        scope: &mut RootScope<'heap, 'scope>,
        old: PersistentPtr,
        new: RootedPtr<'scope>,
    ) -> Result<PersistentPtr, EngineError> {
        let (index, old_root_id) = self.root_id(scope.heap, &old)?;
        let next_generation = old
            .generation
            .checked_add(1)
            .ok_or_else(|| EngineError::Internal("persistent root generation exhausted".into()))?;
        let new_root_id = scope.heap.register_root(scope.pointer(new))?;
        if let Err(error) = scope.heap.unregister_root(old_root_id) {
            return Err(Self::cleanup_failed_install(scope.heap, new_root_id, error));
        }

        // `root_id` validated this index before either heap-root mutation, and
        // no arena operation above can resize `slots`.
        let slot = &mut self.slots[index];
        slot.generation = next_generation;
        slot.root_id = Some(new_root_id);
        Ok(PersistentPtr {
            heap_id: self.heap_id,
            store_id: self.store_id,
            index: old.index,
            generation: next_generation,
        })
    }

    /// Explicitly unregister one persistent root while the heap is locked.
    pub(crate) fn remove<'heap, 'scope>(
        &mut self,
        scope: &mut RootScope<'heap, 'scope>,
        value: PersistentPtr,
    ) -> Result<(), EngineError> {
        let (index, root_id) = self.root_id(scope.heap, &value)?;
        let next_generation = value
            .generation
            .checked_add(1)
            .ok_or_else(|| EngineError::Internal("persistent root generation exhausted".into()))?;
        let next_live_count = self
            .live_count
            .checked_sub(1)
            .ok_or_else(|| EngineError::Internal("persistent root count underflow".into()))?;

        scope.heap.unregister_root(root_id)?;
        // `root_id` validated this index before unregistering the heap root,
        // and no arena operation above can resize `slots`.
        let slot = &mut self.slots[index];
        slot.generation = next_generation;
        slot.root_id = None;
        self.free_slots.push(value.index);
        self.live_count = next_live_count;
        Ok(())
    }

    /// Explicitly unregister every root owned by this evaluator arena.
    pub(crate) fn clear<'heap, 'scope>(
        &mut self,
        scope: &mut RootScope<'heap, 'scope>,
    ) -> Result<(), EngineError> {
        self.validate_heap(scope.heap)?;
        let mut live = Vec::with_capacity(self.live_count);
        for (index, slot) in self.slots.iter().enumerate() {
            if slot.root_id.is_some() {
                live.push(PersistentPtr {
                    heap_id: self.heap_id,
                    store_id: self.store_id,
                    index: u64::try_from(index).map_err(|_| {
                        EngineError::Internal("persistent root index overflow".into())
                    })?,
                    generation: slot.generation,
                });
            }
        }
        for value in live {
            self.remove(scope, value)?;
        }
        Ok(())
    }
}

/// Rex heap and allocation API.
///
/// `Heap` is a cloneable, thread-safe capability for locking and allocating in
/// shared `HeapState`. Embedders may retain it across threads and `await`
/// points. Public allocation methods return [`Handle`] values and may run a
/// copying collection before creating their result.
///
/// Because its allocation and inspection methods acquire the heap mutex, a
/// `Heap` must not be used from code that already owns a `RootScope`. The
/// synchronous evaluator receives only the scope; the outer async coordinator
/// and public host
/// [`Context`](crate::Context) retain the `Heap` capability.
#[derive(Clone)]
pub struct Heap {
    pub(super) id: u64,
    pub(super) state: Arc<Mutex<HeapState>>,
}

/// A rooted reference to a Rex heap value.
///
/// A handle owns a generational registered-root identifier rather than a raw
/// heap location. Collection rewrites the registered slot, so embedders and
/// host functions may clone and retain handles while allocating, crossing
/// threads, or suspending in async code. The value remains visible to the
/// collector until the last clone is dropped.
///
/// Public handle inspection and conversion operations acquire the heap mutex,
/// and dropping the last owner unregisters its root under that mutex.
/// Synchronous code that already owns a `RootScope` must use `RootedPtr`
/// instead and must not destroy the last owner of a handle while the lock is
/// held.
#[derive(Clone)]
pub struct Handle {
    root: Arc<HandleRoot>,
}

/// Public view of one Rex value.
///
/// Scalar payloads are copied out of the heap. Composite child references are
/// returned as registered [`Handle`] roots, so the view remains valid after
/// the heap lock is released and across later collections. Callable and
/// uninitialized variants intentionally reveal only their broad runtime kind,
/// never their moving internal representation.
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
    Closure,
    Native,
    Overloaded,
}

/// A public value view whose child references are registered roots but have
/// not yet been wrapped in handle owners.
///
/// This representation can safely leave the locked heap operation: dropping a
/// `RootId` does not lock, while constructing the corresponding `Handle`
/// values only happens after the mutex guard has been released.
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
    Tuple(Vec<RootId>),
    Empty,
    Cons(RootId, RootId),
    ListSlice {
        start: usize,
        end: usize,
        elements: RootId,
    },
    Data(Vec<RootId>),
    BinaryData(Vec<u8>),
    Dict(Vec<Symbol>, Vec<RootId>),
    Adt(Symbol, Vec<RootId>),
    Uninitialized(Symbol),
    Closure,
    Native,
    Overloaded,
}

impl ValueSeed {
    fn into_value(self, heap: &Heap) -> Value {
        match self {
            Self::Bool(value) => Value::Bool(value),
            Self::U8(value) => Value::U8(value),
            Self::U16(value) => Value::U16(value),
            Self::U32(value) => Value::U32(value),
            Self::U64(value) => Value::U64(value),
            Self::I8(value) => Value::I8(value),
            Self::I16(value) => Value::I16(value),
            Self::I32(value) => Value::I32(value),
            Self::I64(value) => Value::I64(value),
            Self::F32(value) => Value::F32(value),
            Self::F64(value) => Value::F64(value),
            Self::String(value) => Value::String(value),
            Self::Uuid(value) => Value::Uuid(value),
            Self::DateTime(value) => Value::DateTime(value),
            Self::Tuple(values) => Value::Tuple(heap.handles_from_root_ids(values)),
            Self::Empty => Value::Empty,
            Self::Cons(head, tail) => Value::Cons(
                handle_from_registered_root(heap, head),
                handle_from_registered_root(heap, tail),
            ),
            Self::ListSlice {
                start,
                end,
                elements,
            } => Value::ListSlice {
                start,
                end,
                elements: handle_from_registered_root(heap, elements),
            },
            Self::Data(values) => Value::Data(heap.handles_from_root_ids(values)),
            Self::BinaryData(values) => Value::BinaryData(values),
            Self::Dict(names, roots) => Value::Dict(
                names
                    .into_iter()
                    .zip(heap.handles_from_root_ids(roots))
                    .collect(),
            ),
            Self::Adt(name, args) => Value::Adt(name, heap.handles_from_root_ids(args)),
            Self::Uninitialized(name) => Value::Uninitialized(name),
            Self::Closure => Value::Closure,
            Self::Native => Value::Native,
            Self::Overloaded => Value::Overloaded,
        }
    }
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

pub(super) fn handle_from_registered_root(heap: &Heap, root_id: RootId) -> Handle {
    Handle {
        root: Arc::new(HandleRoot {
            heap: heap.clone(),
            root_id,
        }),
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
        self.root
            .heap
            .with_locked(|heap| Self::type_name_l(heap, self.root.root_id))
    }

    fn type_name_l(heap: &HeapState, root_id: RootId) -> Result<&'static str, EngineError> {
        let pointer = heap.resolve_root(root_id)?;
        heap.type_name(&pointer)
    }

    pub fn value(&self) -> Result<Value, EngineError> {
        let heap = &self.root.heap;
        let seed = heap.with_locked(|state| {
            let cell = state.get_cell_from_root(self.root.root_id)?.clone();
            Ok(match cell {
                Cell::Bool(value) => ValueSeed::Bool(value),
                Cell::U8(value) => ValueSeed::U8(value),
                Cell::U16(value) => ValueSeed::U16(value),
                Cell::U32(value) => ValueSeed::U32(value),
                Cell::U64(value) => ValueSeed::U64(value),
                Cell::I8(value) => ValueSeed::I8(value),
                Cell::I16(value) => ValueSeed::I16(value),
                Cell::I32(value) => ValueSeed::I32(value),
                Cell::I64(value) => ValueSeed::I64(value),
                Cell::F32(value) => ValueSeed::F32(value),
                Cell::F64(value) => ValueSeed::F64(value),
                Cell::String(value) => ValueSeed::String(value),
                Cell::Uuid(value) => ValueSeed::Uuid(value),
                Cell::DateTime(value) => ValueSeed::DateTime(value),
                Cell::Tuple(values) => ValueSeed::Tuple(state.register_roots(values)?),
                Cell::Empty => ValueSeed::Empty,
                Cell::Cons(head, tail) => {
                    let roots = state.register_roots([head, tail])?;
                    ValueSeed::Cons(roots[0], roots[1])
                }
                Cell::ListSlice {
                    start,
                    end,
                    elements,
                } => ValueSeed::ListSlice {
                    start,
                    end,
                    elements: state.register_root(elements)?,
                },
                Cell::Data(values) => ValueSeed::Data(state.register_roots(values)?),
                Cell::BinaryData(values) => ValueSeed::BinaryData(values),
                Cell::Dict(values) => {
                    let (names, pointers): (Vec<_>, Vec<_>) = values.into_iter().unzip();
                    ValueSeed::Dict(names, state.register_roots(pointers)?)
                }
                Cell::Adt(name, args) => ValueSeed::Adt(name, state.register_roots(args)?),
                Cell::Uninitialized(name) => ValueSeed::Uninitialized(name),
                Cell::Closure(_) => ValueSeed::Closure,
                Cell::Native(_) => ValueSeed::Native,
                Cell::Overloaded(_) => ValueSeed::Overloaded,
            })
        })?;
        Ok(seed.into_value(heap))
    }

    pub fn as_bool(&self) -> Result<bool, EngineError> {
        let heap = self.lock()?;
        let cell = heap.get_cell_from_root(self.root.root_id)?;
        cell.cell_as_bool()
    }

    pub fn as_u8(&self) -> Result<u8, EngineError> {
        let heap = self.lock()?;
        let cell = heap.get_cell_from_root(self.root.root_id)?;
        cell.cell_as_u8()
    }

    pub fn as_u16(&self) -> Result<u16, EngineError> {
        let heap = self.lock()?;
        let cell = heap.get_cell_from_root(self.root.root_id)?;
        cell.cell_as_u16()
    }

    pub fn as_u32(&self) -> Result<u32, EngineError> {
        let heap = self.lock()?;
        let cell = heap.get_cell_from_root(self.root.root_id)?;
        cell.cell_as_u32()
    }

    pub fn as_u64(&self) -> Result<u64, EngineError> {
        let heap = self.lock()?;
        let cell = heap.get_cell_from_root(self.root.root_id)?;
        cell.cell_as_u64()
    }

    pub fn as_i8(&self) -> Result<i8, EngineError> {
        let heap = self.lock()?;
        let cell = heap.get_cell_from_root(self.root.root_id)?;
        cell.cell_as_i8()
    }

    pub fn as_i16(&self) -> Result<i16, EngineError> {
        let heap = self.lock()?;
        let cell = heap.get_cell_from_root(self.root.root_id)?;
        cell.cell_as_i16()
    }

    pub fn as_i32(&self) -> Result<i32, EngineError> {
        let heap = self.lock()?;
        let cell = heap.get_cell_from_root(self.root.root_id)?;
        cell.cell_as_i32()
    }

    pub fn as_i64(&self) -> Result<i64, EngineError> {
        let heap = self.lock()?;
        let cell = heap.get_cell_from_root(self.root.root_id)?;
        cell.cell_as_i64()
    }

    pub fn as_f32(&self) -> Result<f32, EngineError> {
        let heap = self.lock()?;
        let cell = heap.get_cell_from_root(self.root.root_id)?;
        cell.cell_as_f32()
    }

    pub fn as_f64(&self) -> Result<f64, EngineError> {
        let heap = self.lock()?;
        let cell = heap.get_cell_from_root(self.root.root_id)?;
        cell.cell_as_f64()
    }

    pub fn as_string(&self) -> Result<String, EngineError> {
        let heap = self.lock()?;
        let cell = heap.get_cell_from_root(self.root.root_id)?;
        cell.cell_as_string()
    }

    pub fn as_uuid(&self) -> Result<Uuid, EngineError> {
        let heap = self.lock()?;
        let cell = heap.get_cell_from_root(self.root.root_id)?;
        cell.cell_as_uuid()
    }

    pub fn as_datetime(&self) -> Result<DateTime<Utc>, EngineError> {
        let heap = self.lock()?;
        let cell = heap.get_cell_from_root(self.root.root_id)?;
        cell.cell_as_datetime()
    }

    pub fn as_tuple(&self) -> Result<Vec<Handle>, EngineError> {
        match self.value()? {
            Value::Tuple(values) => Ok(values),
            _ => Err(self
                .root
                .heap
                .with_locked_ok(|heap| Self::type_error(heap, self.root.root_id, "tuple"))?),
        }
    }

    pub fn as_list(&self) -> Result<Vec<Handle>, EngineError> {
        let root_ids = self.heap().with_locked(|heap| {
            let pointer = self.pointer(heap)?;
            let values = heap.pointer_as_list(&pointer)?;
            heap.register_roots(values)
        })?;
        Ok(self.heap().handles_from_root_ids(root_ids))
    }

    pub fn as_binary_list(&self) -> Result<Vec<u8>, EngineError> {
        self.heap().with_locked(|heap| {
            let pointer = self.pointer(heap)?;
            let bytes = collect_list_u8(heap, &pointer)?;
            Ok(bytes)
        })
    }

    pub fn as_dict(&self) -> Result<BTreeMap<Symbol, Handle>, EngineError> {
        match self.value()? {
            Value::Dict(values) => Ok(values),
            _ => Err(self
                .root
                .heap
                .with_locked_ok(|heap| Self::type_error(heap, self.root.root_id, "dict"))?),
        }
    }

    pub fn as_adt(&self) -> Result<(Symbol, Vec<Handle>), EngineError> {
        match self.value()? {
            Value::Adt(tag, args) => Ok((tag, args)),
            _ => Err(self
                .root
                .heap
                .with_locked_ok(|heap| Self::type_error(heap, self.root.root_id, "adt"))?),
        }
    }

    pub fn to_rust<T: FromRex>(&self) -> Result<T, EngineError> {
        T::from_rex(self)
    }

    pub fn display(&self) -> Result<String, EngineError> {
        self.display_with(ValueDisplayOptions::default())
    }

    pub fn display_with(&self, opts: ValueDisplayOptions) -> Result<String, EngineError> {
        let heap = self.lock()?;
        let pointer = heap.resolve_root(self.root.root_id)?;
        pointer_display_with(&heap, &pointer, opts)
    }

    pub fn debug(&self) -> Result<String, EngineError> {
        let heap = self.lock()?;
        let pointer = heap.resolve_root(self.root.root_id)?;
        pointer_debug(&heap, &pointer)
    }

    pub fn value_eq(&self, other: &Handle) -> Result<bool, EngineError> {
        self.heap().with_locked(|heap| {
            let self_pointer = self.pointer(heap)?;
            let other_pointer = other.pointer(heap)?;
            pointer_eq(heap, &self_pointer, &other_pointer)
        })
    }

    fn type_error(heap: &HeapState, root_id: RootId, expected: &'static str) -> EngineError {
        EngineError::NativeType {
            expected: expected.to_string(),
            got: Self::type_name_l(heap, root_id)
                .unwrap_or("<invalid handle>")
                .to_string(),
        }
    }

    pub fn heap(&self) -> &Heap {
        &self.root.heap
    }

    fn pointer(&self, heap: &HeapState) -> Result<InternalPtr, EngineError> {
        let root_id = self.root_id_for_heap(heap.id())?;
        heap.resolve_root(root_id)
    }

    pub(crate) fn ensure_heap(&self, heap: &Heap) -> Result<(), EngineError> {
        self.root_id_for_heap(heap.id).map(|_| ())
    }

    fn root_id_for_heap(&self, heap_id: u64) -> Result<RootId, EngineError> {
        if self.root.root_id.heap_id != heap_id {
            return Err(wrong_heap_handle(self.root.root_id, heap_id));
        }
        Ok(self.root.root_id)
    }

    fn lock(&self) -> Result<MutexGuard<'_, HeapState>, EngineError> {
        self.root
            .heap
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)
    }
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

impl Heap {
    /// Create a new heap and pass it to one closure.
    ///
    /// A returned [`Handle`] may outlive the closure because each handle keeps
    /// the heap alive. This helper scopes construction, not the lifetime of
    /// values allocated in the heap.
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

    fn with_locked<R>(
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

    /// Run one synchronous operation under a branded root scope.
    pub(crate) fn with_root_scope<R>(
        &self,
        f: impl for<'scope> FnOnce(&mut RootScope<'_, 'scope>) -> Result<R, EngineError>,
    ) -> Result<R, EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        state.root_scope(f)
    }

    fn with_locked_ok<R>(&self, f: impl FnOnce(&mut HeapState) -> R) -> Result<R, EngineError> {
        // Keep all heap reads inside this access object while the lock is held.
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        Ok(f(&mut state))
    }

    #[cfg(test)]
    fn handle(&self, pointer: InternalPtr) -> Result<Handle, EngineError> {
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

    /// Enable or disable collection before every allocation on this heap.
    ///
    /// This is intended for collector stress tests. Normal execution uses the
    /// heap-growth threshold and should retain the default setting.
    pub fn set_collect_on_every_alloc(&self, enabled: bool) -> Result<(), EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::HeapStatePoisoned)?;
        state.set_collect_on_every_alloc(enabled);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn collection_count(&self) -> Result<u64, EngineError> {
        self.with_locked_ok(|heap| heap.collection_count())
    }

    #[cfg(test)]
    pub(crate) fn is_unlocked_for_test(&self) -> bool {
        self.state.try_lock().is_ok()
    }

    pub fn alloc_bool(&self, value: bool) -> Result<Handle, EngineError> {
        with_promotable_root_scope(self, |scope, promoter| {
            let root = scope.alloc_root_bool(value)?;
            promoter.promote(scope, root)
        })
    }

    pub fn alloc_u8(&self, value: u8) -> Result<Handle, EngineError> {
        with_promotable_root_scope(self, |scope, promoter| {
            let root = scope.alloc_root_u8(value)?;
            promoter.promote(scope, root)
        })
    }

    pub fn alloc_u16(&self, value: u16) -> Result<Handle, EngineError> {
        with_promotable_root_scope(self, |scope, promoter| {
            let root = scope.alloc_root_u16(value)?;
            promoter.promote(scope, root)
        })
    }

    pub fn alloc_u32(&self, value: u32) -> Result<Handle, EngineError> {
        with_promotable_root_scope(self, |scope, promoter| {
            let root = scope.alloc_root_u32(value)?;
            promoter.promote(scope, root)
        })
    }

    pub fn alloc_u64(&self, value: u64) -> Result<Handle, EngineError> {
        with_promotable_root_scope(self, |scope, promoter| {
            let root = scope.alloc_root_u64(value)?;
            promoter.promote(scope, root)
        })
    }

    pub fn alloc_i8(&self, value: i8) -> Result<Handle, EngineError> {
        with_promotable_root_scope(self, |scope, promoter| {
            let root = scope.alloc_root_i8(value)?;
            promoter.promote(scope, root)
        })
    }

    pub fn alloc_i16(&self, value: i16) -> Result<Handle, EngineError> {
        with_promotable_root_scope(self, |scope, promoter| {
            let root = scope.alloc_root_i16(value)?;
            promoter.promote(scope, root)
        })
    }

    pub fn alloc_i32(&self, value: i32) -> Result<Handle, EngineError> {
        with_promotable_root_scope(self, |scope, promoter| {
            let root = scope.alloc_root_i32(value)?;
            promoter.promote(scope, root)
        })
    }

    pub fn alloc_i64(&self, value: i64) -> Result<Handle, EngineError> {
        with_promotable_root_scope(self, |scope, promoter| {
            let root = scope.alloc_root_i64(value)?;
            promoter.promote(scope, root)
        })
    }

    pub fn alloc_f32(&self, value: f32) -> Result<Handle, EngineError> {
        with_promotable_root_scope(self, |scope, promoter| {
            let root = scope.alloc_root_f32(value)?;
            promoter.promote(scope, root)
        })
    }

    pub fn alloc_f64(&self, value: f64) -> Result<Handle, EngineError> {
        with_promotable_root_scope(self, |scope, promoter| {
            let root = scope.alloc_root_f64(value)?;
            promoter.promote(scope, root)
        })
    }

    pub fn alloc_string(&self, value: String) -> Result<Handle, EngineError> {
        with_promotable_root_scope(self, |scope, promoter| {
            let root = scope.alloc_root_string(value)?;
            promoter.promote(scope, root)
        })
    }

    pub fn alloc_uuid(&self, value: Uuid) -> Result<Handle, EngineError> {
        with_promotable_root_scope(self, |scope, promoter| {
            let root = scope.alloc_root_uuid(value)?;
            promoter.promote(scope, root)
        })
    }

    pub fn alloc_datetime(&self, value: DateTime<Utc>) -> Result<Handle, EngineError> {
        with_promotable_root_scope(self, |scope, promoter| {
            let root = scope.alloc_root_datetime(value)?;
            promoter.promote(scope, root)
        })
    }

    pub(crate) fn alloc_uninitialized(&self, name: Symbol) -> Result<Handle, EngineError> {
        with_promotable_root_scope(self, |scope, promoter| {
            let root = scope.alloc_root_uninitialized(name)?;
            promoter.promote(scope, root)
        })
    }

    pub fn alloc_tuple(&self, values: Vec<Handle>) -> Result<Handle, EngineError> {
        with_promotable_root_scope(self, |scope, promoter| {
            let values = scope.root_handles(&values)?;
            let root = scope.alloc_root_tuple(values)?;
            promoter.promote(scope, root)
        })
    }

    pub fn alloc_list(&self, values: Vec<Handle>) -> Result<Handle, EngineError> {
        with_promotable_root_scope(self, |scope, promoter| {
            let values = scope.root_handles(&values)?;
            let root = scope.alloc_root_list(values)?;
            promoter.promote(scope, root)
        })
    }

    pub fn alloc_binary_list(&self, values: Vec<u8>) -> Result<Handle, EngineError> {
        with_promotable_root_scope(self, |scope, promoter| {
            let root = scope.alloc_root_binary_list(values)?;
            promoter.promote(scope, root)
        })
    }

    pub fn alloc_empty(&self) -> Result<Handle, EngineError> {
        with_promotable_root_scope(self, |scope, promoter| {
            let root = scope.alloc_root_empty()?;
            promoter.promote(scope, root)
        })
    }

    pub fn alloc_cons(&self, head: Handle, tail: Handle) -> Result<Handle, EngineError> {
        with_promotable_root_scope(self, |scope, promoter| {
            let head = scope.root_handle(&head)?;
            let tail = scope.root_handle(&tail)?;
            let root = scope.alloc_root_cons(head, tail)?;
            promoter.promote(scope, root)
        })
    }

    pub fn alloc_data(&self, values: Vec<Handle>) -> Result<Handle, EngineError> {
        with_promotable_root_scope(self, |scope, promoter| {
            let values = scope.root_handles(&values)?;
            let root = scope.alloc_root_data(values)?;
            promoter.promote(scope, root)
        })
    }

    pub fn alloc_binary_data(&self, values: Vec<u8>) -> Result<Handle, EngineError> {
        with_promotable_root_scope(self, |scope, promoter| {
            let root = scope.alloc_root_binary_data(values)?;
            promoter.promote(scope, root)
        })
    }

    pub fn alloc_list_slice(
        &self,
        start: usize,
        end: usize,
        elements: Handle,
    ) -> Result<Handle, EngineError> {
        with_promotable_root_scope(self, |scope, promoter| {
            let elements = scope.root_handle(&elements)?;
            let root = scope.alloc_root_list_slice(start, end, elements)?;
            promoter.promote(scope, root)
        })
    }

    pub fn alloc_dict(&self, values: BTreeMap<Symbol, Handle>) -> Result<Handle, EngineError> {
        with_promotable_root_scope(self, |scope, promoter| {
            let values = values
                .iter()
                .map(|(name, handle)| Ok((name.clone(), scope.root_handle(handle)?)))
                .collect::<Result<_, EngineError>>()?;
            let root = scope.alloc_root_dict(values)?;
            promoter.promote(scope, root)
        })
    }

    pub fn alloc_adt(&self, name: Symbol, args: Vec<Handle>) -> Result<Handle, EngineError> {
        with_promotable_root_scope(self, |scope, promoter| {
            let args = scope.root_handles(&args)?;
            let root = scope.alloc_root_adt(name, args)?;
            promoter.promote(scope, root)
        })
    }

    #[cfg(test)]
    fn clone_cell(&self, pointer: &InternalPtr) -> Result<Cell, EngineError> {
        self.with_locked(|heap| Ok(heap.get_cell_from_pointer(pointer)?.clone()))
    }

    fn handles_from_root_ids(&self, root_ids: Vec<RootId>) -> Vec<Handle> {
        root_ids
            .into_iter()
            .map(|root_id| Handle {
                root: Arc::new(HandleRoot {
                    heap: self.clone(),
                    root_id,
                }),
            })
            .collect()
    }
}

/// Raw moving reference used only for edges owned by the collector.
///
/// An `InternalPtr` identifies a heap, slot, and heap-wide collection epoch. It
/// may be stored in [`Cell`] or used as a short-lived local while `HeapState`
/// is exclusively borrowed. It must never cross an allocation, heap unlock,
/// thread boundary, or `await` point as an unrooted local. Copying collection
/// rewrites every traced cell edge; epoch and heap checks reject stale or
/// foreign pointers that escape that discipline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct InternalPtr {
    heap_id: u64,
    index: u32,
    generation: u64,
}

struct Reference<'a> {
    heap: &'a mut HeapState,
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
    Dict(BTreeMap<Symbol, InternalPtr>),
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
            Cell::DateTime(..) => "datetime",
            Cell::Tuple(..) => "tuple",
            Cell::Empty | Cell::Cons(..) | Cell::ListSlice { .. } => "list",
            Cell::Data(..) => "data",
            Cell::BinaryData(..) => "binary_data",
            Cell::Dict(..) => "dict",
            Cell::Adt(..) => "adt",
            Cell::Uninitialized(..) => "uninitialized",
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

    pub(super) fn cell_as_dict(&self) -> Result<BTreeMap<Symbol, InternalPtr>, EngineError> {
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

fn infer_cell_type(heap: &HeapState, cell: &Cell) -> Result<Type, EngineError> {
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
            | Cell::DateTime(_)
            | Cell::BinaryData(_)
            | Cell::Empty
            | Cell::Uninitialized(_) => Ok(()),
        }
    }
}

pub(super) type InternalPtrKey = (u64, u32, u64);
pub(super) type InternalPtrPairKey = (InternalPtrKey, InternalPtrKey);

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

fn pointer_key(pointer: &InternalPtr) -> InternalPtrKey {
    (pointer.heap_id, pointer.index, pointer.generation)
}

fn canonical_pointer_pair(lhs: InternalPtrKey, rhs: InternalPtrKey) -> InternalPtrPairKey {
    if lhs <= rhs { (lhs, rhs) } else { (rhs, lhs) }
}

pub(super) fn pointer_debug_inner(
    heap: &HeapState,
    pointer: &InternalPtr,
    active: &mut HashSet<InternalPtrKey>,
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

pub(super) fn pointer_display_inner(
    heap: &HeapState,
    pointer: &InternalPtr,
    active: &mut HashSet<InternalPtrKey>,
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
    env: &InternalEnvironment,
    active: &mut HashSet<InternalPtrKey>,
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
    active: &mut HashSet<InternalPtrKey>,
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
    active: &mut HashSet<InternalPtrKey>,
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
        Cell::Closure(closure) => closure_debug_inner(heap, closure, active)?,
        Cell::Native(native) => format!("<native:{}>", native.name()),
        Cell::Overloaded(over) => format!("<overloaded:{}>", over.name()),
    })
}

fn cell_display_inner(
    heap: &HeapState,
    cell: &Cell,
    active: &mut HashSet<InternalPtrKey>,
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
        Cell::Closure(..) => "<closure>".to_string(),
        Cell::Native(native) => format!("<native:{}>", native.name()),
        Cell::Overloaded(over) => format!("<overloaded:{}>", over.name()),
    })
}

pub(super) fn pointer_debug(
    heap: &HeapState,
    pointer: &InternalPtr,
) -> Result<String, EngineError> {
    let mut active = HashSet::new();
    pointer_debug_inner(heap, pointer, &mut active)
}

pub(super) fn pointer_display_with(
    heap: &HeapState,
    pointer: &InternalPtr,
    opts: ValueDisplayOptions,
) -> Result<String, EngineError> {
    let mut active = HashSet::new();
    pointer_display_inner(heap, pointer, &mut active, opts)
}

pub(super) fn pointer_eq_inner(
    heap: &HeapState,
    lhs: &InternalPtr,
    rhs: &InternalPtr,
    seen: &mut HashSet<InternalPtrPairKey>,
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
    lhs: &InternalEnvironment,
    rhs: &InternalEnvironment,
    seen: &mut HashSet<InternalPtrPairKey>,
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
    seen: &mut HashSet<InternalPtrPairKey>,
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
    seen: &mut HashSet<InternalPtrPairKey>,
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
        (Cell::Closure(lhs), Cell::Closure(rhs)) => closure_eq_inner(heap, lhs, rhs, seen),
        (Cell::Native(lhs), Cell::Native(rhs)) => Ok(lhs == rhs),
        (Cell::Overloaded(lhs), Cell::Overloaded(rhs)) => Ok(lhs == rhs),
        _ => Ok(false),
    }
}

pub(super) fn pointer_eq(
    heap: &HeapState,
    lhs: &InternalPtr,
    rhs: &InternalPtr,
) -> Result<bool, EngineError> {
    let mut seen = HashSet::new();
    pointer_eq_inner(heap, lhs, rhs, &mut seen)
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

fn wrong_heap_handle(root_id: RootId, heap_id: u64) -> EngineError {
    EngineError::Internal(format!(
        "heap handle belongs to different heap (handle_heap_id={}, heap_id={}, root_index={}, root_generation={})",
        root_id.heap_id, heap_id, root_id.index, root_id.generation
    ))
}

fn invalid_root(root_id: RootId) -> EngineError {
    EngineError::Internal(format!(
        "invalid heap root (heap_id={}, index={}, generation={})",
        root_id.heap_id, root_id.index, root_id.generation
    ))
}

fn invalid_persistent_ptr(value: &PersistentPtr) -> EngineError {
    EngineError::Internal(format!(
        "invalid persistent root (heap_id={}, store_id={}, index={}, generation={})",
        value.heap_id, value.store_id, value.index, value.generation
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
    use crate::memory::traits::{FromRex, IntoRex};

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn boundary_safe_references_are_send_and_sync() {
        assert_send_sync::<Heap>();
        assert_send_sync::<Handle>();
        assert_send_sync::<PersistentPtr>();
        assert_send_sync::<PersistentRootStore>();
    }

    fn new_persistent_store(heap: &Heap) -> PersistentRootStore {
        heap.with_locked(HeapState::persistent_root_store)
            .expect("persistent root store should be created")
    }

    fn persist_handle(
        heap: &Heap,
        store: &mut PersistentRootStore,
        handle: &Handle,
    ) -> PersistentPtr {
        heap.with_locked(|heap| {
            heap.root_scope(|scope| {
                let pointer = handle.pointer(scope.heap)?;
                let rooted = scope.root(pointer);
                store.insert(scope, rooted)
            })
        })
        .expect("handle should become persistent")
    }

    fn remove_persistent(heap: &Heap, store: &mut PersistentRootStore, value: PersistentPtr) {
        heap.with_locked(|heap| heap.root_scope(|scope| store.remove(scope, value)))
            .expect("persistent root should be removed");
    }

    fn persistent_i32(heap: &Heap, store: &PersistentRootStore, value: &PersistentPtr) -> i32 {
        heap.with_locked(|heap| {
            heap.root_scope(|scope| {
                let rooted = store.resolve(scope, value)?;
                scope.root_as_i32(rooted)
            })
        })
        .expect("persistent i32 should resolve")
    }

    fn copy_persistent_ptr(value: &PersistentPtr) -> PersistentPtr {
        PersistentPtr {
            heap_id: value.heap_id,
            store_id: value.store_id,
            index: value.index,
            generation: value.generation,
        }
    }

    #[test]
    fn persistent_store_reuses_slots_with_generation_bump() {
        let heap = Heap::new();
        let first = heap.alloc_i32(1).expect("first value should allocate");
        let second = heap.alloc_i32(2).expect("second value should allocate");
        let mut store = new_persistent_store(&heap);

        let first_ptr = persist_handle(&heap, &mut store, &first);
        let stale = copy_persistent_ptr(&first_ptr);
        let stale_for_remove = copy_persistent_ptr(&first_ptr);
        let first_index = first_ptr.index;
        let first_generation = first_ptr.generation;
        remove_persistent(&heap, &mut store, first_ptr);

        let second_ptr = persist_handle(&heap, &mut store, &second);
        assert_eq!(second_ptr.index, first_index);
        assert_eq!(second_ptr.generation, first_generation + 1);
        heap.with_locked(|heap| {
            heap.root_scope(|scope| {
                assert!(
                    store.resolve(scope, &stale).is_err(),
                    "a token from the previous slot generation must be rejected"
                );
                assert!(
                    store.remove(scope, stale_for_remove).is_err(),
                    "a stale token must not unregister the reused slot"
                );
                Ok(())
            })
        })
        .unwrap();

        remove_persistent(&heap, &mut store, second_ptr);
    }

    #[test]
    fn persistent_store_duplicate_has_independent_ownership() {
        let heap = Heap::new();
        let handle = heap.alloc_i32(42).expect("value should allocate");
        let mut store = new_persistent_store(&heap);
        let original = persist_handle(&heap, &mut store, &handle);
        let duplicate = heap
            .with_locked(|heap| heap.root_scope(|scope| store.duplicate(scope, &original)))
            .expect("persistent root should duplicate");
        drop(handle);

        assert_eq!(
            heap.with_locked(|heap| Ok(heap.root_count())).unwrap(),
            2,
            "each persistent owner should have its own registered root"
        );
        remove_persistent(&heap, &mut store, original);
        heap.with_locked(|heap| heap.collect())
            .expect("collection should succeed");
        assert_eq!(persistent_i32(&heap, &store, &duplicate), 42);

        remove_persistent(&heap, &mut store, duplicate);
        assert_eq!(heap.with_locked(|heap| Ok(heap.root_count())).unwrap(), 0);
    }

    #[test]
    fn persistent_store_root_is_rewritten_by_collection() {
        let heap = Heap::new();
        let handle = heap.alloc_i32(42).expect("value should allocate");
        let mut store = new_persistent_store(&heap);
        let value = persist_handle(&heap, &mut store, &handle);
        drop(handle);

        let generation_before = heap
            .with_locked(|heap| {
                let (_, root_id) = store.root_id(heap, &value)?;
                Ok(heap.resolve_root(root_id)?.generation)
            })
            .unwrap();
        for _ in 0..3 {
            heap.with_locked(|heap| heap.collect())
                .expect("collection should succeed");
        }
        let generation_after = heap
            .with_locked(|heap| {
                let (_, root_id) = store.root_id(heap, &value)?;
                Ok(heap.resolve_root(root_id)?.generation)
            })
            .unwrap();

        assert!(generation_after > generation_before);
        assert_eq!(persistent_i32(&heap, &store, &value), 42);
        remove_persistent(&heap, &mut store, value);
    }

    #[test]
    fn persistent_store_replace_invalidates_old_token() {
        let heap = Heap::new();
        let old_handle = heap.alloc_i32(1).expect("old value should allocate");
        let new_handle = heap.alloc_i32(2).expect("new value should allocate");
        let mut store = new_persistent_store(&heap);
        let old = persist_handle(&heap, &mut store, &old_handle);
        let stale = copy_persistent_ptr(&old);
        drop(old_handle);

        let replacement = heap
            .with_locked(|heap| {
                heap.root_scope(|scope| {
                    let pointer = new_handle.pointer(scope.heap)?;
                    let rooted = scope.root(pointer);
                    store.replace(scope, old, rooted)
                })
            })
            .expect("persistent root should be replaced");
        assert_eq!(
            heap.with_locked(|heap| Ok(heap.root_count())).unwrap(),
            2,
            "replacement should exchange roots without retaining the old value"
        );
        drop(new_handle);

        heap.with_locked(|heap| {
            heap.root_scope(|scope| {
                assert!(store.resolve(scope, &stale).is_err());
                Ok(())
            })
        })
        .unwrap();
        assert_eq!(persistent_i32(&heap, &store, &replacement), 2);
        remove_persistent(&heap, &mut store, replacement);
        assert_eq!(heap.with_locked(|heap| Ok(heap.root_count())).unwrap(), 0);
    }

    #[test]
    fn persistent_store_clear_unregisters_every_root() {
        let heap = Heap::new();
        let handles = (0..3)
            .map(|value| heap.alloc_i32(value).expect("value should allocate"))
            .collect::<Vec<_>>();
        let mut store = new_persistent_store(&heap);
        let stale = handles
            .iter()
            .map(|handle| persist_handle(&heap, &mut store, handle))
            .collect::<Vec<_>>();
        drop(handles);

        assert_eq!(heap.with_locked(|heap| Ok(heap.root_count())).unwrap(), 3);
        heap.with_locked(|heap| heap.root_scope(|scope| store.clear(scope)))
            .expect("persistent arena teardown should succeed");
        assert_eq!(store.live_count, 0);
        assert!(store.slots.iter().all(|slot| slot.root_id.is_none()));
        assert_eq!(heap.with_locked(|heap| Ok(heap.root_count())).unwrap(), 0);
        heap.with_locked(|heap| {
            heap.root_scope(|scope| {
                for value in &stale {
                    assert!(store.resolve(scope, value).is_err());
                }
                Ok(())
            })
        })
        .unwrap();
    }

    #[test]
    fn persistent_store_rejects_foreign_heap_and_store() {
        let first_heap = Heap::new();
        let second_heap = Heap::new();
        let handle = first_heap.alloc_i32(42).expect("value should allocate");
        let mut first_store = new_persistent_store(&first_heap);
        let second_store = new_persistent_store(&first_heap);
        let value = persist_handle(&first_heap, &mut first_store, &handle);

        first_heap
            .with_locked(|heap| {
                heap.root_scope(|scope| {
                    assert!(second_store.resolve(scope, &value).is_err());
                    Ok(())
                })
            })
            .unwrap();
        second_heap
            .with_locked(|heap| {
                heap.root_scope(|scope| {
                    assert!(first_store.resolve(scope, &value).is_err());
                    Ok(())
                })
            })
            .unwrap();

        remove_persistent(&first_heap, &mut first_store, value);
    }

    #[test]
    fn handle_roots_value_until_last_clone_drops() {
        let heap = Heap::new();
        let pointer = heap
            .with_locked(|heap| {
                heap.root_scope(|scope| {
                    let root = scope.alloc_root_i32(42)?;
                    Ok(scope.pointer(root))
                })
            })
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
            .with_locked(|heap| {
                heap.root_scope(|scope| {
                    let root = scope.alloc_root_i32(1)?;
                    Ok(scope.pointer(root))
                })
            })
            .expect("alloc_i32 should succeed");
        let first = heap
            .handle(first_pointer)
            .expect("handle should root pointer");
        let first_root_id = first.root.root_id;
        drop(first);

        let second_pointer = heap
            .with_locked(|heap| {
                heap.root_scope(|scope| {
                    let root = scope.alloc_root_i32(2)?;
                    Ok(scope.pointer(root))
                })
            })
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
        let handle = heap.alloc_i32(1).expect("first i32 should allocate");
        let replacement = heap.alloc_i32(2).expect("second i32 should allocate");

        heap.with_locked(|state| {
            let first_pointer = handle.pointer(state)?;
            let second_pointer = replacement.pointer(state)?;
            let slot = state
                .root_slots
                .get_mut(handle.root.root_id.index as usize)
                .expect("root slot should exist");
            assert_eq!(slot.pointer, Some(first_pointer));
            slot.pointer = Some(second_pointer);
            Ok(())
        })
        .expect("root slot should be replaceable");

        assert_eq!(handle.as_i32().expect("handle should follow root slot"), 2);
    }

    #[test]
    fn handle_rejects_pointer_from_different_heap() {
        let heap_a = Heap::new();
        let heap_b = Heap::new();
        let pointer = heap_a
            .with_locked(|heap| {
                heap.root_scope(|scope| {
                    let root = scope.alloc_root_i32(42)?;
                    Ok(scope.pointer(root))
                })
            })
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
        let first = heap.alloc_i32(42).expect("first i32 should allocate");
        let second = heap.alloc_i32(7).expect("second i32 should allocate");
        let epoch_before = heap
            .with_locked_ok(|heap| heap.collection_count())
            .expect("collection epoch");
        let stale = heap
            .with_locked(|heap| first.pointer(heap))
            .expect("first pointer should resolve");

        assert_eq!(stale.generation, epoch_before);
        assert_eq!(
            heap.with_locked(|heap| second.pointer(heap))
                .expect("second pointer should resolve")
                .generation,
            epoch_before
        );

        heap.with_locked(|heap| heap.collect())
            .expect("collection should succeed");

        let epoch_after = heap
            .with_locked_ok(|heap| heap.collection_count())
            .expect("collection epoch");
        assert_eq!(epoch_after, epoch_before + 1);
        assert_eq!(
            first
                .as_i32()
                .expect("first handle should follow moved value"),
            42
        );
        assert_eq!(
            heap.with_locked(|heap| first.pointer(heap))
                .expect("first pointer should resolve after collection")
                .generation,
            epoch_after
        );
        assert_eq!(
            heap.with_locked(|heap| second.pointer(heap))
                .expect("second pointer should resolve after collection")
                .generation,
            epoch_after
        );
        assert!(
            heap.with_locked(|heap| {
                heap.root_scope(|scope| {
                    let stale = scope.root(stale);
                    scope.root_as_i32(stale)
                })
            })
            .is_err(),
            "raw pointer from before collection should be stale"
        );
    }

    #[test]
    fn copying_gc_updates_temporary_root_stack() {
        let mut heap = HeapState::new();
        let _garbage = heap
            .root_scope(|scope| {
                let root = scope.alloc_root_i32(7)?;
                Ok::<InternalPtr, EngineError>(scope.pointer(root))
            })
            .expect("garbage should allocate");
        let stale = heap
            .root_scope(|scope| {
                let root = scope.alloc_root_i32(42)?;
                Ok::<InternalPtr, EngineError>(scope.pointer(root))
            })
            .expect("rooted value should allocate");

        heap.root_scope(|scope| {
            let rooted = scope.root(stale);
            scope.heap.collect()?;

            let refreshed = scope.pointer(rooted);
            assert_eq!(refreshed.index, 0);
            assert_ne!(refreshed.generation, stale.generation);
            let refreshed_root = scope.root(refreshed);
            assert_eq!(scope.root_as_i32(refreshed_root)?, 42);
            let stale_root = scope.root(stale);
            assert!(scope.root_as_i32(stale_root).is_err());
            Ok::<_, EngineError>(())
        })
        .expect("collection should update the temporary root");

        assert!(heap.temporary_roots.is_empty());
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "root scope shadow stack underflow")]
    fn root_scope_detects_shadow_stack_underflow() {
        let mut heap = HeapState::new();
        heap.root_scope(|outer| {
            let _rooted = outer
                .alloc_root_i32(42)
                .expect("rooted value should allocate");
            outer.heap.root_scope(|inner| {
                inner.heap.temporary_roots.clear();
            });
        });
    }

    #[test]
    fn alloc_triggers_collection_after_heap_growth() {
        let heap = Heap::new();
        let rooted = heap.alloc_i32(7).expect("alloc_i32 handle");
        heap.with_locked_ok(|heap| heap.set_gc_slot_threshold(1))
            .expect("set threshold");

        let _garbage = heap
            .with_locked(|heap| {
                heap.root_scope(|scope| {
                    let root = scope.alloc_root_i32(99)?;
                    Ok(scope.pointer(root))
                })
            })
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
        heap.set_collect_on_every_alloc(false)
            .expect("disable automatic collection");
        heap.with_locked_ok(|heap| heap.set_gc_slot_threshold(usize::MAX))
            .expect("set threshold");
        let values = (0..2048)
            .map(|value| heap.alloc_i32(value).expect("alloc_i32 should succeed"))
            .collect::<Vec<_>>();
        heap.with_locked_ok(|heap| heap.set_gc_slot_threshold(1))
            .expect("set threshold");

        let list = heap
            .alloc_list(values)
            .expect("list allocation should protect inputs");
        let values = list.as_list().expect("list should decode");

        assert_eq!(values.len(), 2048);
        assert_eq!(values.first().expect("first value").as_i32().unwrap(), 0);
        assert_eq!(values.last().expect("last value").as_i32().unwrap(), 2047);
    }

    #[test]
    fn alloc_ptr_list_uses_vector_backed_slice_representation() {
        let heap = Heap::new();
        let values = [1, 2, 3]
            .into_iter()
            .map(|value| heap.alloc_i32(value).expect("alloc_i32 should succeed"))
            .collect::<Vec<_>>();

        let list = heap.alloc_list(values.clone()).expect("list allocation");
        heap.with_locked(|state| {
            let list = list.pointer(state)?;
            let Cell::ListSlice {
                start,
                end,
                elements,
            } = state.get_cell_from_pointer(&list)?.clone()
            else {
                panic!("expected vector-backed list slice");
            };
            assert_eq!(start, 0);
            assert_eq!(end, values.len());
            let Cell::Data(backing) = state.get_cell_from_pointer(&elements)? else {
                panic!("expected list data backing");
            };
            let expected = values
                .iter()
                .map(|value| value.pointer(state))
                .collect::<Result<Vec<_>, _>>()?;
            assert_eq!(backing, &expected);
            Ok(())
        })
        .expect("list representation should be valid");
    }

    #[test]
    fn list_slice_tail_shares_data_backing() {
        let heap = Heap::new();
        let values = [1, 2, 3]
            .into_iter()
            .map(|value| heap.alloc_i32(value).expect("alloc_i32 should succeed"))
            .collect::<Vec<_>>();
        let list = heap.alloc_list(values).expect("list allocation");

        heap.with_locked(|state| {
            state.root_scope(|scope| {
                let list = scope.root_handle(&list)?;
                let Cell::ListSlice {
                    elements: original_data,
                    ..
                } = scope.get_cell_from_rooted_ptr(list)?.clone()
                else {
                    panic!("expected vector-backed list slice");
                };
                let original_data = scope.root(original_data);
                let (_head, tail) = scope
                    .list_head_tail(list)?
                    .expect("list should be non-empty");
                let Cell::ListSlice {
                    start,
                    end,
                    elements,
                } = scope.get_cell_from_rooted_ptr(tail)?
                else {
                    panic!("expected tail list slice");
                };
                assert_eq!(*start, 1);
                assert_eq!(*end, 3);
                assert_eq!(*elements, scope.pointer(original_data));
                Ok(())
            })
        })
        .expect("head/tail should decode");
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

        let bytes_pointer = heap.with_locked_ok(|heap| bytes.pointer(heap)).unwrap();
        let Cell::ListSlice {
            start,
            end,
            elements,
        } = heap
            .clone_cell(&bytes_pointer.expect("bytes pointer"))
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
        let bytes_pointer = heap
            .with_locked(|heap| bytes.pointer(heap))
            .expect("bytes pointer");
        let (head, tail) = heap
            .with_locked(|heap| {
                heap.root_scope(|scope| {
                    let bytes_pointer = scope.root(bytes_pointer);
                    Ok(scope
                        .list_head_tail(bytes_pointer)?
                        .map(|(head, tail)| (scope.pointer(head), scope.pointer(tail))))
                })
            })
            .expect("head/tail should decode")
            .expect("list should be non-empty");

        assert_eq!(
            heap.with_locked(|heap| {
                heap.root_scope(|scope| {
                    let head = scope.root(head);
                    scope.root_as_u8(head)
                })
            })
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
        heap.set_collect_on_every_alloc(false)
            .expect("disable automatic collection during graph construction");
        heap.with_locked_ok(|heap| heap.set_gc_slot_threshold(usize::MAX))
            .expect("set threshold");
        let values = (0..10_000)
            .map(|value| {
                heap.with_locked(|heap| {
                    heap.root_scope(|scope| {
                        let root = scope.alloc_root_i32(value)?;
                        Ok(scope.pointer(root))
                    })
                })
                .expect("alloc_i32 should succeed")
            })
            .collect::<Vec<_>>();
        let list = heap
            .handle(
                heap.with_locked(|heap| {
                    heap.root_scope(|scope| {
                        let values = values.into_iter().map(|x| scope.root(x)).collect();
                        let root = scope.alloc_root_list(values)?;
                        Ok(scope.pointer(root))
                    })
                })
                .expect("list allocation should succeed"),
            )
            .expect("list should be rootable");

        heap.with_locked(|heap| heap.collect())
            .expect("deep collection should succeed");

        let pointer = heap
            .with_locked(|heap| list.pointer(heap))
            .expect("list pointer");
        assert_eq!(
            heap.with_locked(|heap| heap.pointer_as_list(&pointer))
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
        let first = heap.alloc_i32(1).expect("alloc_i32 should succeed");
        let second = heap
            .alloc_string("two".into())
            .expect("alloc_string should succeed");
        let tuple = heap
            .alloc_tuple(vec![first, second])
            .expect("alloc_tuple should succeed");

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
        let payload = heap.alloc_bool(true).expect("alloc_bool should succeed");

        let mut fields = BTreeMap::new();
        fields.insert(Symbol::intern("ready"), payload.clone());
        let dict = heap.alloc_dict(fields).expect("alloc_dict should succeed");
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
            .alloc_adt(Symbol::intern("Some"), vec![payload])
            .expect("alloc_adt should succeed");
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
