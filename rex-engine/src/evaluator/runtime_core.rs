use crate::{
    builder::registry::{NativeId, NativeRegistry, TypeclassRegistry},
    config::{AsyncCallPolicy, ParallelismController},
    error::EngineError,
    evaluator::native_callable::NativeCallable,
};
use rex_typesystem::typesystem::TypeSystem;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct RuntimeCore<State = ()>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) state: Arc<State>,
    pub(crate) natives: Arc<NativeRegistry<State>>,
    pub(crate) typeclasses: Arc<TypeclassRegistry>,
    pub(crate) type_system: Arc<TypeSystem>,
    pub(crate) async_call_policy: AsyncCallPolicy,
    pub(crate) parallelism_controller: Arc<dyn ParallelismController>,
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
