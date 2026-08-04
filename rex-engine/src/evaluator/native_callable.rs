use crate::{
    Value,
    builder::registry::NativeId,
    error::EngineError,
    evaluator::{context::Context, native_functions::NativeTask},
    handlers::{NativeCallRequest, NativeValueFuture},
    memory::heap::{RootScope, RootedPtr},
};
use rex_typesystem::types::Type;
use std::sync::Arc;

pub(crate) type HostValueCallable<State> =
    Arc<dyn Fn(Context<State>, Type, Vec<Value>) -> NativeValueFuture + Send + Sync + 'static>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeCallScheduling {
    Immediate,
    Deferred,
}

pub(crate) type SchedulerNativeCallable = Arc<
    dyn for<'a, 'heap> Fn(
            &'a mut RootScope<'heap>,
            Type,
            &'a [RootedPtr],
        ) -> Result<SchedulerNativeResult, EngineError>
        + Send
        + Sync
        + 'static,
>;

pub(crate) enum SchedulerNativeResult {
    Ready(RootedPtr),
    Task(NativeTask<RootedPtr>),
}

#[derive(Clone)]
pub(crate) enum NativeCallable<State: Clone + Send + Sync + 'static> {
    Host {
        callable: HostValueCallable<State>,
        scheduling: NativeCallScheduling,
    },
    Constant(RootedPtr),
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
            NativeCallable::Constant(_) => write!(f, "Constant"),
            NativeCallable::Scheduler(_) => write!(f, "Scheduler"),
        }
    }
}

impl<State: Clone + Send + Sync + 'static> NativeCallable<State> {
    pub(crate) fn call(
        &self,
        native_id: NativeId,
        typ: Type,
        args: &[RootedPtr],
    ) -> Result<NativeCallRequest, EngineError> {
        match self {
            NativeCallable::Host { scheduling, .. } => Ok(NativeCallRequest::new(
                native_id,
                *scheduling,
                typ,
                args.to_vec(),
            )),
            NativeCallable::Constant(_) => Err(EngineError::Internal(
                "constant called through host native ABI".into(),
            )),
            NativeCallable::Scheduler(_) => Err(EngineError::Internal(
                "scheduler native called through host native ABI".into(),
            )),
        }
    }
}
