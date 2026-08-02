//! Narrow conversion from synchronous rooted values to public handles.

use crate::EngineError;

use super::heap::{Handle, Heap, RootScope, RootedPtr, handle_from_registered_root};

/// Narrow authority for promoting scope-rooted values to public handles.
///
/// This capability deliberately exposes neither `Heap` nor any operation that
/// locks, inspects heap values, allocates Rex values, or collects. It is
/// created only by [`with_promotable_root_scope`] and borrowed for that single
/// locked operation.
pub(crate) struct HandlePromoter<'heap> {
    heap: &'heap Heap,
}

impl HandlePromoter<'_> {
    fn validate_scope(&self, scope: &RootScope<'_, '_>) -> Result<(), EngineError> {
        if self.heap.id != scope.heap.id() {
            return Err(EngineError::Internal(format!(
                "root scope belongs to heap {}, not heap {}",
                scope.heap.id(),
                self.heap.id
            )));
        }
        Ok(())
    }

    /// Promote a synchronous rooted value without reacquiring the heap lock.
    pub(crate) fn promote<'scope>(
        &self,
        scope: &mut RootScope<'_, 'scope>,
        value: RootedPtr<'scope>,
    ) -> Result<Handle, EngineError> {
        self.validate_scope(scope)?;
        let root_id = scope.heap.register_root(scope.pointer(value))?;
        Ok(handle_from_registered_root(self.heap, root_id))
    }

    /// Promote several synchronous rooted values under the existing guard.
    pub(crate) fn promote_all<'scope>(
        &self,
        scope: &mut RootScope<'_, 'scope>,
        values: &[RootedPtr<'scope>],
    ) -> Result<Vec<Handle>, EngineError> {
        self.validate_scope(scope)?;
        let pointers = values
            .iter()
            .map(|value| scope.pointer(*value))
            .collect::<Vec<_>>();
        let root_ids = scope.heap.register_roots(pointers)?;
        Ok(root_ids
            .into_iter()
            .map(|root_id| handle_from_registered_root(self.heap, root_id))
            .collect())
    }
}

/// Run one synchronous operation under a branded root scope with narrow
/// authority to promote rooted values without reacquiring the heap mutex.
pub(crate) fn with_promotable_root_scope<R>(
    heap: &Heap,
    f: impl for<'scope> FnOnce(
        &mut RootScope<'_, 'scope>,
        &HandlePromoter<'_>,
    ) -> Result<R, EngineError>,
) -> Result<R, EngineError> {
    let mut state = heap
        .state
        .lock()
        .map_err(|_| EngineError::HeapStatePoisoned)?;
    let promoter = HandlePromoter { heap };
    state.root_scope(|scope| f(scope, &promoter))
}
