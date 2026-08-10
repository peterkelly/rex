use super::{
    ToolExecution, ToolExecutionError, ToolExecutionPlan, ToolExecutor, ToolFuture,
    workspace::ToolWorkspace,
};
use crate::storage::store::Store;
use std::{process::Stdio, sync::Arc};
use tokio::{io::AsyncWriteExt, process::Command};

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

    let (executable, subcommand) = plan.program.command();
    let mut command = Command::new(executable);
    if let Some(subcommand) = subcommand {
        command.arg(subcommand);
    }
    command
        .args(arguments)
        .current_dir(workspace.root())
        .env("MAGICK_TEMPORARY_PATH", workspace.scratch_dir())
        .env("TMPDIR", workspace.scratch_dir())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let stdin_data = match plan.stdin {
        Some(hash) => Some(
            store
                .get(hash)
                .await
                .map_err(|error| ToolExecutionError::new(format!("read stdin object: {error}")))?,
        ),
        None => None,
    };
    command.stdin(if stdin_data.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });

    let mut child = command
        .spawn()
        .map_err(|error| ToolExecutionError::new(format!("spawn `{executable}`: {error}")))?;
    let stdin_writer = if let Some(data) = stdin_data {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| ToolExecutionError::new("spawned tool has no stdin pipe"))?;
        Some(tokio::spawn(async move {
            stdin.write_all(&data).await?;
            stdin.shutdown().await
        }))
    } else {
        None
    };

    let process_output = child
        .wait_with_output()
        .await
        .map_err(|error| ToolExecutionError::new(format!("wait for tool: {error}")))?;
    if let Some(writer) = stdin_writer {
        writer
            .await
            .map_err(|error| ToolExecutionError::new(format!("join stdin writer: {error}")))?
            .map_err(|error| ToolExecutionError::new(format!("write tool stdin: {error}")))?;
    }

    let outputs = workspace.import_outputs(store, &plan.outputs).await?;
    Ok(ToolExecution {
        exit_code: process_output.status.code(),
        stdout: process_output.stdout,
        stderr: process_output.stderr,
        outputs,
    })
}
