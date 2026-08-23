use crate::modules::tools::executor::{
    OciJobExecutor, OciToolExecutor, OciToolImages, ToolExecutionError, ToolExecutionErrorKind,
    ToolExecutionPlan, ToolExecutor, ToolFuture, docker_executor,
};
use rex::{modules::std::storage::StateStore, storage::Store};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

/// Configuration used to locate and invoke independently installed Rex tool binaries.
#[derive(Clone, Debug)]
pub struct ExternalTools {
    pub directory: PathBuf,
    pub environment: BTreeMap<String, String>,
}

#[derive(Clone)]
pub struct State {
    pub store: Store,
    pub(crate) tools: Arc<dyn ToolExecutor>,
    pub(crate) external_tools: Option<ExternalTools>,
}

impl StateStore for State {
    fn store(&self) -> Option<&Store> {
        Some(&self.store)
    }
}

impl State {
    /// Execute a prepared OCI tool plan against this state's content-addressed store.
    pub async fn execute_tool(
        &self,
        plan: ToolExecutionPlan,
    ) -> Result<crate::modules::tools::executor::ToolExecution, ToolExecutionError> {
        self.tools.execute(&self.store, plan).await
    }

    pub fn docker(store: Store, images: OciToolImages) -> Self {
        Self {
            store,
            tools: docker_executor(images),
            external_tools: None,
        }
    }

    /// Configure a conforming OCI backend supplied by an embedding host.
    pub fn oci(store: Store, images: OciToolImages, backend: Arc<dyn OciJobExecutor>) -> Self {
        Self {
            store,
            tools: Arc::new(OciToolExecutor::new(images, backend)),
            external_tools: None,
        }
    }

    /// Construct state for parsing, typechecking, and pure evaluation tests.
    /// Any attempted external tool call fails instead of running a host binary.
    pub fn without_tools(store: Store) -> Self {
        Self {
            store,
            tools: Arc::new(UnavailableToolExecutor),
            external_tools: None,
        }
    }

    /// Look for additional `rex-tool-NAME` executables in `directory`.
    pub fn with_tool_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.external_tools = Some(ExternalTools {
            directory: directory.into(),
            environment: BTreeMap::new(),
        });
        self
    }

    /// Set an environment variable passed only to installed tool processes.
    pub fn with_tool_environment(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        let tools = self.external_tools.get_or_insert_with(|| ExternalTools {
            directory: PathBuf::from("."),
            environment: BTreeMap::new(),
        });
        tools.environment.insert(name.into(), value.into());
        self
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
