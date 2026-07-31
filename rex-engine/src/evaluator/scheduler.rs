use crate::{
    config::{NativeAsyncPermit, ParallelismController},
    error::EngineError,
    evaluator::{native_callable::NativeCallScheduling, runtime_core::RuntimeCore},
    handlers::{NativeCall, NativeHandleFuture},
    memory::{
        heap::{Handle, Heap, PersistentRoots, Pointer},
        traits::Collection,
    },
    stack::{FrameId, FrameStore},
};
use futures::future::poll_fn;
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    task::{Context as TaskContext, Poll},
};

pub(crate) struct EvalScheduler<State: Clone + Send + Sync + 'static> {
    ready: VecDeque<EvalWorkItem>,
    deferred_ready: VecDeque<EvalWorkItem>,
    pending_native: Vec<PendingNative>,
    immediate_native: VecDeque<QueuedNative<State>>,
    deferred_native: VecDeque<QueuedNative<State>>,
    parallelism_controller: Arc<dyn ParallelismController>,
}

impl<State> EvalScheduler<State>
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

    pub(crate) fn schedule_next(&mut self, item: EvalWorkItem) {
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

    pub(crate) fn pop_next(&mut self) -> Option<EvalWorkItem> {
        self.admit_deferred_ready();
        let item = self.ready.pop_front();
        self.admit_deferred_ready();
        item
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

    fn refresh_rooted_values(&mut self, heap: &Heap) -> Result<(), EngineError> {
        for item in &mut self.ready {
            item.refresh_rooted_value(heap)?;
        }
        for item in &mut self.deferred_ready {
            item.refresh_rooted_value(heap)?;
        }
        Ok(())
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

    pub(crate) fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        for item in &self.ready {
            item.trace_pointers(out);
        }
        for item in &self.deferred_ready {
            item.trace_pointers(out);
        }
    }

    pub(crate) fn map_pointers<E>(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E> {
        for item in &mut self.ready {
            item.map_pointers(rewrite)?;
        }
        for item in &mut self.deferred_ready {
            item.map_pointers(rewrite)?;
        }
        Ok(())
    }
}

pub(crate) async fn poll_pending_native<State>(
    runtime: &mut RuntimeCore<State>,
    heap: &Heap,
    frames: &mut FrameStore,
    scheduler: &mut EvalScheduler<State>,
    wait: bool,
) -> Result<bool, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if !scheduler.has_pending_native()
        && !scheduler.has_immediate_native()
        && !scheduler.has_deferred_native()
    {
        return Ok(false);
    }

    scheduler.refresh_rooted_values(heap)?;
    let mut protected = Vec::new();
    frames.trace_pointers(&mut protected);
    scheduler.trace_pointers(&mut protected);
    runtime.trace_pointers(&mut protected)?;
    let mut roots = heap.persistent_roots(protected.clone())?;

    scheduler.activate_next_immediate_native(runtime, heap);
    refresh_scheduler_roots(runtime, frames, scheduler, &mut roots, &mut protected)?;
    poll_fn(|cx| Poll::Ready(scheduler.admit_available_deferred_native(cx, runtime, heap))).await?;
    refresh_scheduler_roots(runtime, frames, scheduler, &mut roots, &mut protected)?;
    if !scheduler.has_pending_native() {
        if !wait || (!scheduler.has_immediate_native() && !scheduler.has_deferred_native()) {
            return Ok(false);
        }
        poll_fn(|cx| match scheduler.try_acquire_next_native_permit(cx) {
            Poll::Ready(Ok(true)) => Poll::Ready(Ok(())),
            Poll::Ready(Ok(false)) => Poll::Ready(Ok(())),
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Pending,
        })
        .await?;
        let activated = scheduler.activate_next_permitted_deferred_native(runtime, heap);
        refresh_scheduler_roots(runtime, frames, scheduler, &mut roots, &mut protected)?;
        return Ok(activated);
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

    refresh_scheduler_roots(runtime, frames, scheduler, &mut roots, &mut protected)?;

    let Some(event) = event else {
        return Ok(false);
    };
    let NativeWaitEvent::Completion(index) = event else {
        let activated = scheduler.activate_next_permitted_deferred_native(runtime, heap);
        refresh_scheduler_roots(runtime, frames, scheduler, &mut roots, &mut protected)?;
        return Ok(activated);
    };
    let (frame, handle) = scheduler.take_pending_native_completion(index)?;
    let value = handle.pointer_for_heap(heap)?;
    scheduler.schedule_next(EvalWorkItem::receive_rooted(frame, frame, value, handle));
    Ok(true)
}

fn refresh_scheduler_roots<State>(
    runtime: &mut RuntimeCore<State>,
    frames: &mut FrameStore,
    scheduler: &mut EvalScheduler<State>,
    roots: &mut PersistentRoots,
    originals: &mut [Pointer],
) -> Result<(), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    roots.with_updated_pointers(|updated| {
        if updated.len() != originals.len() {
            return Err(EngineError::Internal(
                "persistent root snapshot length changed".into(),
            ));
        }

        let mut rewrites = HashMap::with_capacity(originals.len());
        for (original, updated) in originals.iter_mut().zip(updated.iter().copied()) {
            rewrites.insert(*original, updated);
            *original = updated;
        }
        let mut rewrite =
            |pointer| Ok::<_, EngineError>(rewrites.get(&pointer).copied().unwrap_or(pointer));
        frames.map_pointers(&mut rewrite)?;
        scheduler.map_pointers(&mut rewrite)?;
        runtime.map_pointers(&mut rewrite)
    })?;
    Ok(())
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

pub(crate) struct EvalWorkItem {
    pub(crate) frame: FrameId,
    pub(crate) returned: Option<EvalReturned>,
}

pub(crate) struct EvalReturned {
    pub(crate) child: FrameId,
    pub(crate) value: Pointer,
    handle: Option<Handle>,
}

impl EvalWorkItem {
    pub(crate) fn enter(frame: FrameId) -> Self {
        Self {
            frame,
            returned: None,
        }
    }

    pub(crate) fn receive(frame: FrameId, child: FrameId, value: Pointer) -> Self {
        Self {
            frame,
            returned: Some(EvalReturned {
                child,
                value,
                handle: None,
            }),
        }
    }

    pub(crate) fn receive_rooted(
        frame: FrameId,
        child: FrameId,
        value: Pointer,
        handle: Handle,
    ) -> Self {
        Self {
            frame,
            returned: Some(EvalReturned {
                child,
                value,
                handle: Some(handle),
            }),
        }
    }

    pub(crate) fn refresh_rooted_value(&mut self, heap: &Heap) -> Result<(), EngineError> {
        if let Some(returned) = self.returned.as_mut()
            && let Some(handle) = returned.handle.as_ref()
        {
            returned.value = handle.pointer_for_heap(heap)?;
        }
        Ok(())
    }

    pub(crate) fn take_returned_handle(&mut self) -> Option<Handle> {
        self.returned
            .as_mut()
            .and_then(|returned| returned.handle.take())
    }

    pub(crate) fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        if let Some(returned) = self.returned.as_ref() {
            out.push(returned.value);
        }
    }

    pub(crate) fn map_pointers<E>(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E> {
        if let Some(returned) = self.returned.as_mut() {
            returned.value = rewrite(returned.value)?;
        }
        Ok(())
    }
}
