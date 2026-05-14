use crate::{
    builder::export::NativeFuture, handlers::NativeHandleFuture, modules::PRELUDE_MODULE_NAME,
};
use std::{fmt, sync::Arc};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreludeMode {
    Enabled,
    Disabled,
}

#[derive(Clone, Debug)]
pub struct EngineOptions {
    pub prelude: PreludeMode,
    pub default_imports: Vec<String>,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            prelude: PreludeMode::Enabled,
            default_imports: vec![PRELUDE_MODULE_NAME.to_string()],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutionBounds {
    max_ready_work: usize,
    max_pending_async_calls: usize,
}

impl ExecutionBounds {
    pub const DEFAULT_MAX_READY_WORK: usize = 1024;
    pub const DEFAULT_MAX_PENDING_ASYNC_CALLS: usize = 64;

    /// Create scheduler bounds for evaluator work.
    ///
    /// `max_ready_work` limits how many ready Rex frames are admitted to the
    /// active scheduler queue. `max_pending_async_calls` limits how many async
    /// host-call futures are admitted for polling or executor submission at
    /// once. Zero values are normalized to one so evaluation can always make
    /// progress.
    pub const fn new(max_ready_work: usize, max_pending_async_calls: usize) -> Self {
        Self {
            max_ready_work: if max_ready_work == 0 {
                1
            } else {
                max_ready_work
            },
            max_pending_async_calls: if max_pending_async_calls == 0 {
                1
            } else {
                max_pending_async_calls
            },
        }
    }

    /// Disable scheduler admission limits.
    pub const fn unbounded() -> Self {
        Self {
            max_ready_work: usize::MAX,
            max_pending_async_calls: usize::MAX,
        }
    }

    pub const fn max_ready_work(self) -> usize {
        self.max_ready_work
    }

    pub const fn max_pending_async_calls(self) -> usize {
        self.max_pending_async_calls
    }

    pub(crate) fn ready_work_limit(self) -> usize {
        self.max_ready_work
    }

    pub(crate) fn pending_async_limit(self) -> usize {
        self.max_pending_async_calls
    }
}

impl Default for ExecutionBounds {
    fn default() -> Self {
        Self::new(
            Self::DEFAULT_MAX_READY_WORK,
            Self::DEFAULT_MAX_PENDING_ASYNC_CALLS,
        )
    }
}

/// Execution hook for admitted async host calls.
///
/// The evaluator calls this only after an async host call has passed scheduler
/// admission. The pending-async bound is therefore applied before this hook
/// runs and before an executor can receive the future.
///
/// Returning the future unchanged preserves inline polling. Embedders can wrap
/// it to submit work to Tokio, a browser task queue, or another executor.
pub trait AsyncCallExecutor: Send + Sync + 'static {
    fn spawn(&self, future: NativeFuture) -> NativeFuture;
}

struct FnAsyncCallExecutor<F> {
    spawn: F,
}

impl<F> AsyncCallExecutor for FnAsyncCallExecutor<F>
where
    F: Fn(NativeFuture) -> NativeFuture + Send + Sync + 'static,
{
    fn spawn(&self, future: NativeFuture) -> NativeFuture {
        (self.spawn)(future)
    }
}

/// Policy used to run admitted async host-call futures.
///
/// The default `Inline` policy keeps Rex independent of any particular async
/// runtime. With this policy, admitted host futures are polled by the evaluator
/// task itself. This is portable, including to wasm builds, but a future that
/// performs blocking or CPU-heavy work will still block that evaluator task.
///
/// `Executor` lets embedders decide where admitted host futures should run.
/// The executor is deliberately host-provided so native builds can use Tokio,
/// wasm builds can use browser/task-queue integration, and other embedders can
/// use their own runtime without Rex depending on it directly.
#[derive(Clone, Default)]
pub enum AsyncCallPolicy {
    /// Poll admitted host futures in the evaluator task.
    #[default]
    Inline,
    /// Hand admitted host futures to an embedder-provided executor.
    Executor(Arc<dyn AsyncCallExecutor>),
}

impl AsyncCallPolicy {
    pub fn inline() -> Self {
        Self::Inline
    }

    pub fn executor(executor: impl AsyncCallExecutor) -> Self {
        Self::Executor(Arc::new(executor))
    }

    pub fn executor_arc(executor: Arc<dyn AsyncCallExecutor>) -> Self {
        Self::Executor(executor)
    }

    pub fn executor_fn(
        spawn: impl Fn(NativeFuture) -> NativeFuture + Send + Sync + 'static,
    ) -> Self {
        Self::executor(FnAsyncCallExecutor { spawn })
    }

    pub fn is_inline(&self) -> bool {
        matches!(self, Self::Inline)
    }

    pub fn is_executor(&self) -> bool {
        matches!(self, Self::Executor(_))
    }

    /// Apply the policy after an async host call has been admitted by the
    /// evaluator scheduler and produced its future.
    pub(crate) fn prepare(&self, future: NativeHandleFuture) -> NativeHandleFuture {
        match self {
            Self::Inline => future,
            Self::Executor(executor) => executor.spawn(future),
        }
    }
}

impl fmt::Debug for AsyncCallPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inline => f.write_str("Inline"),
            Self::Executor(_) => f.write_str("Executor"),
        }
    }
}

impl PartialEq for AsyncCallPolicy {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Inline, Self::Inline) => true,
            (Self::Executor(lhs), Self::Executor(rhs)) => Arc::ptr_eq(lhs, rhs),
            _ => false,
        }
    }
}

impl Eq for AsyncCallPolicy {}
