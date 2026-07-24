use crate::{
    builder::registry::{NativeId, NativeRegistry, TypeclassRegistry},
    config::{AsyncCallPolicy, ParallelismController},
    error::EngineError,
    evaluator::native_callable::NativeCallable,
    memory::heap::{Heap, Pointer},
};
use rex_ast::Symbol;
use rex_typesystem::{types::Type, typesystem::TypeSystem};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub(crate) struct RuntimeCore<State = ()>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) state: Arc<State>,
    pub(crate) natives: Arc<NativeRegistry<State>>,
    pub(crate) typeclasses: Arc<TypeclassRegistry>,
    pub(crate) type_system: Arc<TypeSystem>,
    pub(crate) typeclass_cache: Arc<Mutex<BTreeMap<(Symbol, Type), Pointer>>>,
    pub(crate) async_call_policy: AsyncCallPolicy,
    pub(crate) parallelism_controller: Arc<dyn ParallelismController>,
    pub(crate) heap: Heap,
}

impl<State> RuntimeCore<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) fn trace_pointers(&self, out: &mut Vec<Pointer>) -> Result<(), EngineError> {
        let cache = self
            .typeclass_cache
            .lock()
            .map_err(|_| EngineError::Internal("typeclass cache poisoned".into()))?;
        out.extend(cache.values().copied());
        Ok(())
    }

    pub(crate) fn map_pointers(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
    ) -> Result<(), EngineError> {
        let mut cache = self
            .typeclass_cache
            .lock()
            .map_err(|_| EngineError::Internal("typeclass cache poisoned".into()))?;
        for pointer in cache.values_mut() {
            *pointer = rewrite(*pointer)?;
        }
        Ok(())
    }
}

impl<State> RuntimeCore<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) fn native_callable(
        &self,
        id: NativeId,
    ) -> Result<NativeCallable<State>, EngineError> {
        self.natives
            .by_id(id)
            .map(|imp| imp.func.clone())
            .ok_or_else(|| EngineError::Internal(format!("unknown native id: {id}")))
    }
}
