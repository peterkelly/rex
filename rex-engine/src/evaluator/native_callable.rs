use crate::{
    error::EngineError,
    evaluator::{
        CallSite, context::Context, native_functions::NativeTask, runtime_core::RuntimeCore,
    },
    handlers::{NativeAsyncCall, NativeHandleFuture},
    memory::heap::Pointer,
};
use rex_typesystem::types::Type;
use std::sync::Arc;

pub(crate) type SyncNativePointerCallable<State> = Arc<
    dyn for<'a> Fn(Context<State>, &'a Type, &'a [Pointer]) -> Result<Pointer, EngineError>
        + Send
        + Sync
        + 'static,
>;

pub(crate) type SchedulerNativeCallable<State> = Arc<
    dyn for<'a> Fn(
            Context<State>,
            Type,
            &'a [Pointer],
        ) -> Result<SchedulerNativeResult, EngineError>
        + Send
        + Sync
        + 'static,
>;

pub(crate) enum SchedulerNativeResult {
    Ready(Pointer),
    Task(NativeTask),
}

pub(crate) type AsyncNativePointerCallable<State> =
    Arc<dyn Fn(Context<State>, Type, Vec<Pointer>) -> NativeHandleFuture + Send + Sync + 'static>;

#[derive(Clone)]
pub(crate) enum NativeCallable<State: Clone + Send + Sync + 'static> {
    Sync(SyncNativePointerCallable<State>),
    Scheduler(SchedulerNativeCallable<State>),
    Async(AsyncNativePointerCallable<State>),
}

impl<State: Clone + Send + Sync + 'static> PartialEq for NativeCallable<State> {
    fn eq(&self, _other: &NativeCallable<State>) -> bool {
        false
    }
}

impl<State: Clone + Send + Sync + 'static> std::fmt::Debug for NativeCallable<State> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self {
            NativeCallable::Sync(_) => write!(f, "Sync"),
            NativeCallable::Scheduler(_) => write!(f, "Scheduler"),
            NativeCallable::Async(_) => write!(f, "Async"),
        }
    }
}

impl<State: Clone + Send + Sync + 'static> NativeCallable<State> {
    pub(crate) fn call_at_site(
        &self,
        runtime: &RuntimeCore<State>,
        typ: Type,
        args: &[Pointer],
        call_site: CallSite,
    ) -> Result<NativeCallResult<State>, EngineError> {
        match self {
            NativeCallable::Sync(f) => {
                let ctx = Context::new_at_call_site(runtime, call_site);
                (f)(ctx, &typ, args).map(NativeCallResult::Ready)
            }
            NativeCallable::Scheduler(_) => Err(EngineError::Internal(
                "scheduler native called through pointer-returning native ABI".into(),
            )),
            NativeCallable::Async(f) => Ok(NativeCallResult::Pending(NativeAsyncCall::new(
                Arc::clone(f),
                call_site,
                typ,
                args.to_vec(),
            ))),
        }
    }
}

pub(crate) enum NativeCallResult<State: Clone + Send + Sync + 'static> {
    Ready(Pointer),
    Pending(NativeAsyncCall<State>),
}
