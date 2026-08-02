use crate::{
    config::{NativeAsyncPermit, ParallelismController},
    error::EngineError,
    evaluator::{native_callable::NativeCallScheduling, runtime_core::RuntimeCore},
    handlers::{NativeCall, NativeHandleFuture},
    memory::heap::{Handle, Heap},
    stack::FrameId,
};
use futures::future::poll_fn;
use std::{
    collections::VecDeque,
    sync::Arc,
    task::{Context as TaskContext, Poll},
};

pub(crate) struct EvalScheduler<State: Clone + Send + Sync + 'static, P> {
    ready: VecDeque<EvalWorkItem<P>>,
    deferred_ready: VecDeque<EvalWorkItem<P>>,
    pending_native: Vec<PendingNative>,
    immediate_native: VecDeque<QueuedNative<State>>,
    deferred_native: VecDeque<QueuedNative<State>>,
    parallelism_controller: Arc<dyn ParallelismController>,
}

impl<State, P> EvalScheduler<State, P>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) fn new(
        root: FrameId,
        parallelism_controller: Arc<dyn ParallelismController>,
    ) -> Self {
        let mut ready = VecDeque::new();
        ready.push_front(EvalWorkItem::enter(root));
        Self {
            ready,
            deferred_ready: VecDeque::new(),
            pending_native: Vec::new(),
            immediate_native: VecDeque::new(),
            deferred_native: VecDeque::new(),
            parallelism_controller,
        }
    }

    pub(crate) fn schedule_next(&mut self, item: EvalWorkItem<P>) {
        self.ready.push_front(item);
        self.enforce_ready_limit();
    }

    pub(crate) fn schedule_native(&mut self, frame: FrameId, call: NativeCall<State>) {
        let queued = QueuedNative::new(frame, call);
        match queued.call.scheduling() {
            NativeCallScheduling::Immediate => self.immediate_native.push_back(queued),
            NativeCallScheduling::Deferred => self.deferred_native.push_back(queued),
        }
    }

    pub(crate) fn pop_next(&mut self) -> Option<EvalWorkItem<P>> {
        self.admit_deferred_ready();
        let item = self.ready.pop_front();
        self.admit_deferred_ready();
        item
    }

    pub(crate) fn has_ready_work(&self) -> bool {
        !self.ready.is_empty() || !self.deferred_ready.is_empty()
    }

    pub(crate) fn has_queued_native_work(&self) -> bool {
        self.has_immediate_native() || self.has_deferred_native()
    }

    pub(crate) fn has_pending_native_work(&self) -> bool {
        self.has_pending_native()
    }

    fn has_pending_native(&self) -> bool {
        !self.pending_native.is_empty()
    }

    fn has_immediate_native(&self) -> bool {
        !self.immediate_native.is_empty()
    }

    fn has_deferred_native(&self) -> bool {
        !self.deferred_native.is_empty()
    }

    fn enforce_ready_limit(&mut self) {
        // ready_work_limit is only an internal evaluator queue-pressure
        // control. It does not reserve external compute capacity and should
        // not be used as backpressure for host jobs; native async permits do
        // that at the point where host callbacks are actually invoked.
        let limit = self.parallelism_controller.ready_work_limit().max(1);
        while self.ready.len() > limit {
            if let Some(item) = self.ready.pop_back() {
                self.deferred_ready.push_front(item);
            }
        }
    }

    fn admit_deferred_ready(&mut self) {
        // This moves already-created Rex frames between internal queues. It
        // intentionally remains separate from native async admission, which
        // is where embedders can reserve scarce cluster or executor capacity.
        let limit = self.parallelism_controller.ready_work_limit().max(1);
        while self.ready.len() < limit {
            let Some(item) = self.deferred_ready.pop_front() else {
                break;
            };
            self.ready.push_back(item);
        }
    }

    fn try_acquire_next_native_permit(
        &mut self,
        cx: &mut TaskContext<'_>,
    ) -> Poll<Result<bool, EngineError>> {
        let Some(deferred) = self.deferred_native.front_mut() else {
            return Poll::Ready(Ok(false));
        };
        if deferred.permit.is_some() {
            return Poll::Ready(Ok(true));
        }
        match self.parallelism_controller.poll_acquire_native_async(cx) {
            Poll::Ready(Ok(permit)) => {
                deferred.permit = Some(permit);
                Poll::Ready(Ok(true))
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Pending,
        }
    }

    fn activate_next_immediate_native(
        &mut self,
        runtime: &RuntimeCore<State>,
        heap: &Heap,
    ) -> bool {
        let Some(queued) = self.immediate_native.pop_front() else {
            return false;
        };
        self.pending_native
            .push(queued.activate_immediate(runtime, heap));
        true
    }

    fn activate_next_permitted_deferred_native(
        &mut self,
        runtime: &RuntimeCore<State>,
        heap: &Heap,
    ) -> bool {
        let Some(next) = self.deferred_native.front() else {
            return false;
        };
        if next.permit.is_none() {
            return false;
        }
        let Some(queued) = self.deferred_native.pop_front() else {
            return false;
        };
        self.pending_native
            .push(queued.activate_deferred(runtime, heap));
        true
    }

    fn admit_available_deferred_native(
        &mut self,
        cx: &mut TaskContext<'_>,
        runtime: &RuntimeCore<State>,
        heap: &Heap,
    ) -> Result<bool, EngineError> {
        match self.try_acquire_next_native_permit(cx) {
            Poll::Ready(Ok(true)) => {
                Ok(self.activate_next_permitted_deferred_native(runtime, heap))
            }
            Poll::Ready(Ok(false)) | Poll::Pending => Ok(false),
            Poll::Ready(Err(err)) => Err(err),
        }
    }

    fn poll_pending_native_once(&mut self, cx: &mut TaskContext<'_>) -> Option<usize> {
        for (index, pending) in self.pending_native.iter_mut().enumerate() {
            if pending.poll(cx) {
                return Some(index);
            }
        }
        None
    }

    fn take_pending_native_completion(
        &mut self,
        index: usize,
    ) -> Result<(FrameId, Handle), EngineError> {
        if index >= self.pending_native.len() {
            return Err(EngineError::Internal(
                "pending native completion index out of bounds".into(),
            ));
        }
        self.pending_native.remove(index).into_completion()
    }

    pub(crate) fn map_values<Q, E>(
        self,
        map: &mut impl FnMut(P) -> Result<Q, E>,
    ) -> Result<EvalScheduler<State, Q>, E> {
        Ok(EvalScheduler {
            ready: self
                .ready
                .into_iter()
                .map(|item| item.map_value(map))
                .collect::<Result<_, _>>()?,
            deferred_ready: self
                .deferred_ready
                .into_iter()
                .map(|item| item.map_value(map))
                .collect::<Result<_, _>>()?,
            pending_native: self.pending_native,
            immediate_native: self.immediate_native,
            deferred_native: self.deferred_native,
            parallelism_controller: self.parallelism_controller,
        })
    }
}

pub(crate) enum NativePoll {
    Idle,
    Progress,
    Completed { frame: FrameId, handle: Handle },
}

pub(crate) async fn poll_pending_native<State, P>(
    runtime: &RuntimeCore<State>,
    heap: &Heap,
    scheduler: &mut EvalScheduler<State, P>,
    wait: bool,
) -> Result<NativePoll, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if !scheduler.has_pending_native()
        && !scheduler.has_immediate_native()
        && !scheduler.has_deferred_native()
    {
        return Ok(NativePoll::Idle);
    }

    let mut progressed = scheduler.activate_next_immediate_native(runtime, heap);
    progressed |=
        poll_fn(|cx| Poll::Ready(scheduler.admit_available_deferred_native(cx, runtime, heap)))
            .await?;
    if !scheduler.has_pending_native() {
        if !wait || (!scheduler.has_immediate_native() && !scheduler.has_deferred_native()) {
            return Ok(if progressed {
                NativePoll::Progress
            } else {
                NativePoll::Idle
            });
        }
        poll_fn(|cx| match scheduler.try_acquire_next_native_permit(cx) {
            Poll::Ready(Ok(true)) => Poll::Ready(Ok(())),
            Poll::Ready(Ok(false)) => Poll::Ready(Ok(())),
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Pending,
        })
        .await?;
        let activated = scheduler.activate_next_permitted_deferred_native(runtime, heap);
        return Ok(if activated {
            NativePoll::Progress
        } else {
            NativePoll::Idle
        });
    }

    enum NativeWaitEvent {
        Completion(usize),
        Permit,
        Idle,
    }

    let event = if wait {
        Some(
            poll_fn(|cx| match scheduler.poll_pending_native_once(cx) {
                Some(index) => Poll::Ready(Ok(NativeWaitEvent::Completion(index))),
                None => match scheduler.try_acquire_next_native_permit(cx) {
                    Poll::Ready(Ok(true)) => Poll::Ready(Ok(NativeWaitEvent::Permit)),
                    Poll::Ready(Ok(false)) => {
                        if scheduler.has_pending_native() {
                            Poll::Pending
                        } else {
                            Poll::Ready(Ok(NativeWaitEvent::Idle))
                        }
                    }
                    Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
                    Poll::Pending => {
                        if scheduler.has_pending_native() {
                            Poll::Pending
                        } else {
                            Poll::Ready(Ok(NativeWaitEvent::Idle))
                        }
                    }
                },
            })
            .await?,
        )
    } else {
        poll_fn(|cx| {
            Poll::Ready(Ok::<Option<NativeWaitEvent>, EngineError>(
                scheduler
                    .poll_pending_native_once(cx)
                    .map(NativeWaitEvent::Completion),
            ))
        })
        .await?
    };

    let Some(event) = event else {
        return Ok(if progressed {
            NativePoll::Progress
        } else {
            NativePoll::Idle
        });
    };
    let NativeWaitEvent::Completion(index) = event else {
        let activated = scheduler.activate_next_permitted_deferred_native(runtime, heap);
        return Ok(if activated {
            NativePoll::Progress
        } else {
            NativePoll::Idle
        });
    };
    let (frame, handle) = scheduler.take_pending_native_completion(index)?;
    Ok(NativePoll::Completed { frame, handle })
}

enum PendingNativeState {
    Polling(NativeHandleFuture),
    Ready(Result<Handle, EngineError>),
}

struct PendingNative {
    frame: FrameId,
    state: PendingNativeState,
    _permit: Option<NativeAsyncPermit>,
}

struct QueuedNative<State: Clone + Send + Sync + 'static> {
    frame: FrameId,
    call: NativeCall<State>,
    permit: Option<NativeAsyncPermit>,
}

impl PendingNative {
    fn new(frame: FrameId, future: NativeHandleFuture, permit: Option<NativeAsyncPermit>) -> Self {
        Self {
            frame,
            state: PendingNativeState::Polling(future),
            _permit: permit,
        }
    }

    fn ready(frame: FrameId, result: Result<Handle, EngineError>) -> Self {
        Self {
            frame,
            state: PendingNativeState::Ready(result),
            _permit: None,
        }
    }

    fn poll(&mut self, cx: &mut TaskContext<'_>) -> bool {
        match &mut self.state {
            PendingNativeState::Polling(future) => match future.as_mut().poll(cx) {
                Poll::Ready(result) => {
                    self.state = PendingNativeState::Ready(result);
                    true
                }
                Poll::Pending => false,
            },
            PendingNativeState::Ready(_) => true,
        }
    }

    fn into_completion(self) -> Result<(FrameId, Handle), EngineError> {
        match self.state {
            PendingNativeState::Ready(result) => result.map(|handle| (self.frame, handle)),
            PendingNativeState::Polling(_) => Err(EngineError::Internal(
                "pending native future completed without a ready result".into(),
            )),
        }
    }
}

impl<State> QueuedNative<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn new(frame: FrameId, call: NativeCall<State>) -> Self {
        Self {
            frame,
            call,
            permit: None,
        }
    }

    fn activate_immediate(self, runtime: &RuntimeCore<State>, heap: &Heap) -> PendingNative {
        PendingNative::new(self.frame, self.call.invoke(runtime, heap), None)
    }

    fn activate_deferred(self, runtime: &RuntimeCore<State>, heap: &Heap) -> PendingNative {
        let Some(permit) = self.permit else {
            return PendingNative::ready(
                self.frame,
                Err(EngineError::Internal(
                    "deferred native activated without an admission permit".into(),
                )),
            );
        };
        PendingNative::new(self.frame, self.call.invoke(runtime, heap), Some(permit))
    }
}

pub(crate) struct EvalWorkItem<P> {
    pub(crate) frame: FrameId,
    pub(crate) returned: Option<EvalReturned<P>>,
}

pub(crate) struct EvalReturned<P> {
    pub(crate) child: FrameId,
    pub(crate) value: P,
}

impl<P> EvalWorkItem<P> {
    pub(crate) fn enter(frame: FrameId) -> Self {
        Self {
            frame,
            returned: None,
        }
    }

    pub(crate) fn receive(frame: FrameId, child: FrameId, value: P) -> Self {
        Self {
            frame,
            returned: Some(EvalReturned { child, value }),
        }
    }

    pub(crate) fn map_value<Q, E>(
        self,
        map: &mut impl FnMut(P) -> Result<Q, E>,
    ) -> Result<EvalWorkItem<Q>, E> {
        Ok(EvalWorkItem {
            frame: self.frame,
            returned: self
                .returned
                .map(|returned| {
                    Ok(EvalReturned {
                        child: returned.child,
                        value: map(returned.value)?,
                    })
                })
                .transpose()?,
        })
    }
}
