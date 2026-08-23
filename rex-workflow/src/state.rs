use crate::modules::tools::executor::{
    OciJobExecutor, OciToolExecutor, OciToolImages, ToolExecutionError, ToolExecutionErrorKind,
    ToolExecutionPlan, ToolExecutor, ToolFuture, docker_executor,
};
use rex::{modules::std::storage::StateStore, storage::Store};
use std::sync::Arc;

#[derive(Clone)]
pub struct State {
    pub store: Store,
    pub(crate) tools: Arc<dyn ToolExecutor>,
}

impl StateStore for State {
    fn store(&self) -> Option<&Store> {
        Some(&self.store)
    }
}

impl State {
    pub fn docker(store: Store, images: OciToolImages) -> Self {
        Self {
            store,
            tools: docker_executor(images),
        }
    }

    /// Configure a conforming OCI backend supplied by an embedding host.
    pub fn oci(store: Store, images: OciToolImages, backend: Arc<dyn OciJobExecutor>) -> Self {
        Self {
            store,
            tools: Arc::new(OciToolExecutor::new(images, backend)),
        }
    }

    /// Construct state for parsing, typechecking, and pure evaluation tests.
    /// Any attempted external tool call fails instead of running a host binary.
    pub fn without_tools(store: Store) -> Self {
        Self {
            store,
            tools: Arc::new(UnavailableToolExecutor),
        }
    }
}

struct UnavailableToolExecutor;

impl ToolExecutor for UnavailableToolExecutor {
    fn execute<'a>(&'a self, _store: &'a Store, _plan: ToolExecutionPlan) -> ToolFuture<'a> {
        Box::pin(async {
            Err(ToolExecutionError::with_kind(
                ToolExecutionErrorKind::Unsupported,
                "external tools require a configured OCI executor",
            ))
        })
    }
}
