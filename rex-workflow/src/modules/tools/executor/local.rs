use super::{
    DEFAULT_MAX_OUTPUT_BYTES, ToolExecution, ToolExecutionError, ToolExecutionPlan, ToolExecutor,
    ToolFuture, catalog, workspace::ToolWorkspace,
};
use rex::storage::Store;
use std::{process::Stdio, sync::Arc};
use tokio::process::Command;

#[derive(Clone, Default)]
pub struct LocalToolExecutor;

impl ToolExecutor for LocalToolExecutor {
    fn execute<'a>(&'a self, store: &'a Store, plan: ToolExecutionPlan) -> ToolFuture<'a> {
        Box::pin(execute_local(store, plan))
    }
}

pub fn local_executor() -> Arc<dyn ToolExecutor> {
    Arc::new(LocalToolExecutor)
}

async fn execute_local(
    store: &Store,
    plan: ToolExecutionPlan,
) -> Result<ToolExecution, ToolExecutionError> {
    let workspace = ToolWorkspace::prepare(store, &plan.inputs, &plan.outputs).await?;
    let arguments = workspace.render_arguments(&plan.arguments, workspace.root())?;

    let runtime = catalog::runtime(plan.program);
    let executable = runtime.local_executable;
    let wrapper_arguments = workspace.wrapper_arguments(
        workspace.root(),
        executable,
        runtime.prefix_arguments,
        &arguments,
    );
    let mut command = Command::new("/bin/sh");
    command
        .args(wrapper_arguments)
        .current_dir(workspace.root())
        .env("MAGICK_TEMPORARY_PATH", workspace.scratch_dir())
        .env("TMPDIR", workspace.scratch_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    let _ = command
        .status()
        .await
        .map_err(|error| ToolExecutionError::new(format!("wait for tool: {error}")))?;

    let result = workspace.read_result(DEFAULT_MAX_OUTPUT_BYTES)?;
    let outputs = workspace.import_outputs(store, &plan.outputs).await?;
    Ok(ToolExecution {
        exit_code: Some(result.exit_code),
        stdout: result.stdout,
        stderr: result.stderr,
        outputs,
    })
}
