use crate::{
    builder::registry::NativeId,
    error::EngineError,
    evaluator::{CallSite, context::Context, native_functions::NativeTask},
    handlers::{NativeCallRequest, NativeHandleFuture},
    memory::heap::{Handle, RootScope, RootedPtr},
};
use rex_typesystem::types::Type;
use std::sync::Arc;

pub(crate) type NativeHandleCallable<State> =
    Arc<dyn Fn(Context<State>, Type, Vec<Handle>) -> NativeHandleFuture + Send + Sync + 'static>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeCallScheduling {
    Immediate,
    Deferred,
}

pub(crate) type SchedulerNativeCallable = Arc<
    dyn for<'a, 'heap, 'scope> Fn(
            &'a mut RootScope<'heap, 'scope>,
            Type,
            &'a [RootedPtr<'scope>],
        ) -> Result<SchedulerNativeResult<'scope>, EngineError>
        + Send
        + Sync
        + 'static,
>;

pub(crate) enum SchedulerNativeResult<'scope> {
    Ready(RootedPtr<'scope>),
    Task(NativeTask<RootedPtr<'scope>>),
}

#[derive(Clone)]
pub(crate) enum NativeCallable<State: Clone + Send + Sync + 'static> {
    Host {
        callable: NativeHandleCallable<State>,
        scheduling: NativeCallScheduling,
    },
    Scheduler(SchedulerNativeCallable),
}

impl<State: Clone + Send + Sync + 'static> PartialEq for NativeCallable<State> {
    fn eq(&self, _other: &NativeCallable<State>) -> bool {
        false
    }
}

impl<State: Clone + Send + Sync + 'static> std::fmt::Debug for NativeCallable<State> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self {
            NativeCallable::Host { scheduling, .. } => {
                f.debug_tuple("Host").field(scheduling).finish()
            }
            NativeCallable::Scheduler(_) => write!(f, "Scheduler"),
        }
    }
}

impl<State: Clone + Send + Sync + 'static> NativeCallable<State> {
    pub(crate) fn call_at_site<'scope>(
        &self,
        native_id: NativeId,
        typ: Type,
        args: &[RootedPtr<'scope>],
        call_site: CallSite,
    ) -> Result<NativeCallRequest<'scope>, EngineError> {
        match self {
            NativeCallable::Host { scheduling, .. } => Ok(NativeCallRequest::new(
                native_id,
                *scheduling,
                call_site,
                typ,
                args.to_vec(),
            )),
            NativeCallable::Scheduler(_) => Err(EngineError::Internal(
                "scheduler native called through host native ABI".into(),
            )),
        }
    }
}
