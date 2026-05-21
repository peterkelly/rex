use crate::{
    config::{NativeAsyncPermit, ParallelismController},
    error::EngineError,
    evaluator::runtime_core::RuntimeCore,
    handlers::{NativeAsyncCall, NativeHandleFuture},
    value::{Handle, Pointer, TempRoots},
};
use futures::future::poll_fn;
use std::{
    collections::VecDeque,
    sync::Arc,
    task::{Context as TaskContext, Poll},
};

pub(crate) struct EvalScheduler<State: Clone + Send + Sync + 'static> {
    ready: VecDeque<EvalWorkItem>,
    deferred_ready: VecDeque<EvalWorkItem>,
    pending_native: Vec<PendingNative>,
    deferred_native: VecDeque<DeferredNative<State>>,
    parallelism_controller: Arc<dyn ParallelismController>,
}

impl<State> EvalScheduler<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) fn new(
        root: Pointer,
        parallelism_controller: Arc<dyn ParallelismController>,
    ) -> Self {
        let mut ready = VecDeque::new();
        ready.push_front(EvalWorkItem::enter(root));
        Self {
            ready,
            deferred_ready: VecDeque::new(),
            pending_native: Vec::new(),
            deferred_native: VecDeque::new(),
            parallelism_controller,
        }
    }

    pub(crate) fn schedule_next(&mut self, item: EvalWorkItem) {
        self.ready.push_front(item);
        self.enforce_ready_limit();
    }

    pub(crate) fn schedule_pending_native(&mut self, frame: Pointer, call: NativeAsyncCall<State>) {
        self.deferred_native
            .push_back(DeferredNative::new(frame, call));
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

    fn activate_permitted_deferred_native(&mut self, runtime: &RuntimeCore<State>) -> bool {
        let mut activated = false;
        while self
            .deferred_native
            .front()
            .is_some_and(|deferred| deferred.permit.is_some())
        {
            let Some(deferred) = self.deferred_native.pop_front() else {
                break;
            };
            self.pending_native.push(deferred.activate(runtime));
            activated = true;
        }
        activated
    }

    fn admit_available_deferred_native(
        &mut self,
        cx: &mut TaskContext<'_>,
        runtime: &RuntimeCore<State>,
    ) -> Result<bool, EngineError> {
        let mut activated = false;
        loop {
            match self.try_acquire_next_native_permit(cx) {
                Poll::Ready(Ok(true)) => {
                    activated |= self.activate_permitted_deferred_native(runtime);
                }
                Poll::Ready(Ok(false)) | Poll::Pending => return Ok(activated),
                Poll::Ready(Err(err)) => return Err(err),
            }
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
    ) -> Result<(Pointer, Handle), EngineError> {
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
        for pending in &self.pending_native {
            pending.trace_pointers(out);
        }
        for pending in &self.deferred_native {
            pending.trace_pointers(out);
        }
    }

    pub(crate) fn refresh_from_roots(
        &mut self,
        roots: &TempRoots,
        cursor: &mut usize,
    ) -> Result<(), EngineError> {
        for item in &mut self.ready {
            item.refresh_from_roots(roots, cursor)?;
        }
        for item in &mut self.deferred_ready {
            item.refresh_from_roots(roots, cursor)?;
        }
        for pending in &mut self.pending_native {
            pending.refresh_from_roots(roots, cursor)?;
        }
        for pending in &mut self.deferred_native {
            pending.refresh_from_roots(roots, cursor)?;
        }
        Ok(())
    }
}

pub(crate) async fn poll_pending_native<State>(
    runtime: &mut RuntimeCore<State>,
    scheduler: &mut EvalScheduler<State>,
    wait: bool,
) -> Result<bool, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    poll_fn(|cx| Poll::Ready(scheduler.admit_available_deferred_native(cx, runtime))).await?;
    if !scheduler.has_pending_native() {
        if !wait || !scheduler.has_deferred_native() {
            return Ok(false);
        }
        poll_fn(|cx| match scheduler.try_acquire_next_native_permit(cx) {
            Poll::Ready(Ok(true)) => Poll::Ready(Ok(())),
            Poll::Ready(Ok(false)) => Poll::Ready(Ok(())),
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Pending,
        })
        .await?;
        return Ok(scheduler.activate_permitted_deferred_native(runtime));
    }

    let mut protected = Vec::new();
    scheduler.trace_pointers(&mut protected);
    runtime.trace_pointers(&mut protected)?;
    let roots = runtime.heap.temp_roots(protected)?;

    let mut cursor = 0;
    scheduler.refresh_from_roots(&roots, &mut cursor)?;
    runtime.refresh_from_roots(&roots, &mut cursor)?;

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

    let mut cursor = 0;
    scheduler.refresh_from_roots(&roots, &mut cursor)?;
    runtime.refresh_from_roots(&roots, &mut cursor)?;

    let Some(event) = event else {
        return Ok(false);
    };
    let NativeWaitEvent::Completion(index) = event else {
        return Ok(scheduler.activate_permitted_deferred_native(runtime));
    };
    let (frame, handle) = scheduler.take_pending_native_completion(index)?;
    let value = handle.pointer_for_heap(&runtime.heap)?;
    scheduler.schedule_next(EvalWorkItem::receive(frame, frame, value));
    Ok(true)
}

enum PendingNativeState {
    Polling(NativeHandleFuture),
    Ready(Result<Handle, EngineError>),
}

struct PendingNative {
    frame: Pointer,
    state: PendingNativeState,
    _permit: Option<NativeAsyncPermit>,
}

struct DeferredNative<State: Clone + Send + Sync + 'static> {
    frame: Pointer,
    call: NativeAsyncCall<State>,
    permit: Option<NativeAsyncPermit>,
}

impl PendingNative {
    fn new(frame: Pointer, future: NativeHandleFuture, permit: NativeAsyncPermit) -> Self {
        Self {
            frame,
            state: PendingNativeState::Polling(future),
            _permit: Some(permit),
        }
    }

    fn ready(frame: Pointer, result: Result<Handle, EngineError>) -> Self {
        Self {
            frame,
            state: PendingNativeState::Ready(result),
            _permit: None,
        }
    }

    fn ready_with_permit(
        frame: Pointer,
        result: Result<Handle, EngineError>,
        permit: NativeAsyncPermit,
    ) -> Self {
        Self {
            frame,
            state: PendingNativeState::Ready(result),
            _permit: Some(permit),
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

    fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        out.push(self.frame);
    }

    fn refresh_from_roots(
        &mut self,
        roots: &TempRoots,
        cursor: &mut usize,
    ) -> Result<(), EngineError> {
        self.frame = roots.get(*cursor)?;
        *cursor += 1;
        Ok(())
    }

    fn into_completion(self) -> Result<(Pointer, Handle), EngineError> {
        match self.state {
            PendingNativeState::Ready(result) => result.map(|handle| (self.frame, handle)),
            PendingNativeState::Polling(_) => Err(EngineError::Internal(
                "pending native future completed without a ready result".into(),
            )),
        }
    }
}

impl<State> DeferredNative<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn new(frame: Pointer, call: NativeAsyncCall<State>) -> Self {
        Self {
            frame,
            call,
            permit: None,
        }
    }

    fn activate(self, runtime: &RuntimeCore<State>) -> PendingNative {
        let Some(permit) = self.permit else {
            return PendingNative::ready(
                self.frame,
                Err(EngineError::Internal(
                    "deferred native activated without an admission permit".into(),
                )),
            );
        };
        match self.call.invoke(runtime) {
            Ok(future) => PendingNative::new(self.frame, future, permit),
            Err(err) => PendingNative::ready_with_permit(self.frame, Err(err), permit),
        }
    }

    fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        out.push(self.frame);
        self.call.trace_pointers(out);
    }

    fn refresh_from_roots(
        &mut self,
        roots: &TempRoots,
        cursor: &mut usize,
    ) -> Result<(), EngineError> {
        self.frame = roots.get(*cursor)?;
        *cursor += 1;
        self.call.refresh_from_roots(roots, cursor)?;
        Ok(())
    }
}

pub(crate) struct EvalWorkItem {
    pub(crate) frame: Pointer,
    pub(crate) returned: Option<EvalReturned>,
}

#[derive(Clone, Copy)]
pub(crate) struct EvalReturned {
    pub(crate) child: Pointer,
    pub(crate) value: Pointer,
}

impl EvalWorkItem {
    pub(crate) fn enter(frame: Pointer) -> Self {
        Self {
            frame,
            returned: None,
        }
    }

    pub(crate) fn receive(frame: Pointer, child: Pointer, value: Pointer) -> Self {
        Self {
            frame,
            returned: Some(EvalReturned { child, value }),
        }
    }

    pub(crate) fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        out.push(self.frame);
        if let Some(returned) = self.returned.as_ref() {
            out.push(returned.child);
            out.push(returned.value);
        }
    }

    pub(crate) fn refresh_from_roots(
        &mut self,
        roots: &TempRoots,
        cursor: &mut usize,
    ) -> Result<(), EngineError> {
        self.frame = roots.get(*cursor)?;
        *cursor += 1;
        if let Some(returned) = self.returned.as_mut() {
            returned.child = roots.get(*cursor)?;
            *cursor += 1;
            returned.value = roots.get(*cursor)?;
            *cursor += 1;
        }
        Ok(())
    }
}
