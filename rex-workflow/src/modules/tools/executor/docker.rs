use super::{
    ToolBundle, ToolExecution, ToolExecutionError, ToolExecutionPlan, ToolExecutor, ToolFuture,
    catalog::{self, ToolRuntime},
    workspace::ToolWorkspace,
};
use crate::storage::store::Store;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command as StdCommand, Stdio},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{io::AsyncWriteExt, process::Command};

const CONTAINER_WORKSPACE: &str = "/work";
const CONTAINER_SCRATCH: &str = "/work/scratch";
static NEXT_CONTAINER_ID: AtomicU64 = AtomicU64::new(0);

/// Container image references for each supported tool bundle.
///
/// References may use tags or digests, but they must already be available to
/// the Docker daemon. The executor never pulls images while running a tool.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DockerToolImages {
    ffmpeg: String,
    image_magick: String,
    qpdf: String,
    poppler: String,
}

impl DockerToolImages {
    pub fn new(
        ffmpeg: impl Into<String>,
        image_magick: impl Into<String>,
        qpdf: impl Into<String>,
        poppler: impl Into<String>,
    ) -> Self {
        Self {
            ffmpeg: ffmpeg.into(),
            image_magick: image_magick.into(),
            qpdf: qpdf.into(),
            poppler: poppler.into(),
        }
    }

    pub fn image(&self, bundle: ToolBundle) -> &str {
        match bundle {
            ToolBundle::Ffmpeg => &self.ffmpeg,
            ToolBundle::ImageMagick => &self.image_magick,
            ToolBundle::Qpdf => &self.qpdf,
            ToolBundle::Poppler => &self.poppler,
        }
    }
}

/// Executes tool plans in one-shot Docker containers on the local machine.
#[derive(Clone, Debug)]
pub struct DockerToolExecutor {
    docker_binary: PathBuf,
    images: DockerToolImages,
}

impl DockerToolExecutor {
    pub fn new(images: DockerToolImages) -> Self {
        Self {
            docker_binary: PathBuf::from("docker"),
            images,
        }
    }

    /// Override the Docker CLI binary used to start and clean up containers.
    pub fn with_docker_binary(mut self, binary: impl Into<PathBuf>) -> Self {
        self.docker_binary = binary.into();
        self
    }
}

impl ToolExecutor for DockerToolExecutor {
    fn execute<'a>(&'a self, store: &'a Store, plan: ToolExecutionPlan) -> ToolFuture<'a> {
        Box::pin(execute_docker(self, store, plan))
    }
}

pub fn docker_executor(images: DockerToolImages) -> Arc<dyn ToolExecutor> {
    Arc::new(DockerToolExecutor::new(images))
}

async fn execute_docker(
    executor: &DockerToolExecutor,
    store: &Store,
    plan: ToolExecutionPlan,
) -> Result<ToolExecution, ToolExecutionError> {
    let workspace = ToolWorkspace::prepare(store, &plan.inputs, &plan.outputs).await?;
    let runtime = catalog::runtime(plan.program);
    let arguments = workspace.render_arguments(&plan.arguments, Path::new(CONTAINER_WORKSPACE))?;
    let host_workspace = std::fs::canonicalize(workspace.root()).map_err(|error| {
        ToolExecutionError::new(format!("resolve Docker tool workspace: {error}"))
    })?;
    let container_name = next_container_name();
    let image = executor.images.image(runtime.bundle);
    validate_image_reference(runtime.bundle, image)?;
    let docker_arguments = docker_arguments(
        &host_workspace,
        &container_name,
        image,
        runtime,
        &arguments,
        plan.stdin.is_some(),
    );

    let stdin_data = match plan.stdin {
        Some(hash) => Some(
            store
                .get(hash)
                .await
                .map_err(|error| ToolExecutionError::new(format!("read stdin object: {error}")))?,
        ),
        None => None,
    };

    let mut command = Command::new(&executor.docker_binary);
    command
        .args(docker_arguments)
        .stdin(if stdin_data.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = command.spawn().map_err(|error| {
        ToolExecutionError::new(format!(
            "spawn Docker CLI `{}`: {error}",
            executor.docker_binary.display()
        ))
    })?;
    let mut cleanup = ContainerCleanup::new(&executor.docker_binary, container_name);

    let stdin_writer = if let Some(data) = stdin_data {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| ToolExecutionError::new("Docker CLI has no stdin pipe"))?;
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
        .map_err(|error| ToolExecutionError::new(format!("wait for Docker tool: {error}")))?;
    cleanup.disarm();

    if let Some(writer) = stdin_writer {
        writer
            .await
            .map_err(|error| ToolExecutionError::new(format!("join stdin writer: {error}")))?
            .map_err(|error| {
                ToolExecutionError::new(format!("write Docker tool stdin: {error}"))
            })?;
    }

    if matches!(process_output.status.code(), Some(125..=127)) {
        return Err(ToolExecutionError::new(format!(
            "Docker could not start the tool: {}",
            String::from_utf8_lossy(&process_output.stderr).trim()
        )));
    }

    let outputs = workspace.import_outputs(store, &plan.outputs).await?;
    Ok(ToolExecution {
        exit_code: process_output.status.code(),
        stdout: process_output.stdout,
        stderr: process_output.stderr,
        outputs,
    })
}

fn validate_image_reference(bundle: ToolBundle, image: &str) -> Result<(), ToolExecutionError> {
    if image.is_empty()
        || image.starts_with('-')
        || image
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(ToolExecutionError::new(format!(
            "invalid Docker image reference for {bundle:?}"
        )));
    }
    Ok(())
}

fn docker_arguments(
    host_workspace: &Path,
    container_name: &str,
    image: &str,
    runtime: ToolRuntime,
    arguments: &[String],
    has_stdin: bool,
) -> Vec<OsString> {
    let mut mount = OsString::from("type=bind,source=");
    mount.push(host_workspace);
    mount.push(",destination=/work");

    let mut docker_arguments = vec![
        OsString::from("run"),
        OsString::from("--rm"),
        OsString::from("--name"),
        OsString::from(container_name),
        OsString::from("--pull=never"),
        OsString::from("--network=none"),
        OsString::from("--read-only"),
        OsString::from("--cap-drop=ALL"),
        OsString::from("--security-opt=no-new-privileges"),
        OsString::from("--no-healthcheck"),
        OsString::from("--mount"),
        mount,
        OsString::from("--workdir"),
        OsString::from(CONTAINER_WORKSPACE),
        OsString::from("--env"),
        OsString::from(format!("MAGICK_TEMPORARY_PATH={CONTAINER_SCRATCH}")),
        OsString::from("--env"),
        OsString::from(format!("TMPDIR={CONTAINER_SCRATCH}")),
        OsString::from("--entrypoint"),
        OsString::from(runtime.executable),
    ];
    if has_stdin {
        docker_arguments.push(OsString::from("--interactive"));
    }
    docker_arguments.push(OsString::from(image));
    docker_arguments.extend(runtime.prefix_arguments.iter().map(OsString::from));
    docker_arguments.extend(arguments.iter().map(OsString::from));
    docker_arguments
}

fn next_container_name() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = NEXT_CONTAINER_ID.fetch_add(1, Ordering::Relaxed);
    format!("rex-tool-{}-{timestamp}-{sequence}", std::process::id())
}

struct ContainerCleanup {
    docker_binary: PathBuf,
    container_name: String,
    armed: bool,
}

impl ContainerCleanup {
    fn new(docker_binary: &Path, container_name: String) -> Self {
        Self {
            docker_binary: docker_binary.to_owned(),
            container_name,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ContainerCleanup {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let _ = StdCommand::new(&self.docker_binary)
            .args(["container", "rm", "--force", &self.container_name])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::tools::executor::{ExpectedOutput, OutputKind, ToolProgram};

    fn images() -> DockerToolImages {
        DockerToolImages::new(
            "example/rex-ffmpeg@sha256:111",
            "example/rex-imagemagick@sha256:222",
            "example/rex-qpdf@sha256:333",
            "example/rex-poppler@sha256:444",
        )
    }

    #[test]
    fn image_configuration_covers_every_bundle() {
        let images = images();
        assert_eq!(
            images.image(ToolBundle::Ffmpeg),
            "example/rex-ffmpeg@sha256:111"
        );
        assert_eq!(
            images.image(ToolBundle::ImageMagick),
            "example/rex-imagemagick@sha256:222"
        );
        assert_eq!(
            images.image(ToolBundle::Qpdf),
            "example/rex-qpdf@sha256:333"
        );
        assert_eq!(
            images.image(ToolBundle::Poppler),
            "example/rex-poppler@sha256:444"
        );
    }

    #[test]
    fn image_references_cannot_be_interpreted_as_docker_options() {
        for image in ["", "--privileged", "image with spaces", "image\n--volume"] {
            assert!(validate_image_reference(ToolBundle::Ffmpeg, image).is_err());
        }
        assert!(
            validate_image_reference(ToolBundle::Ffmpeg, "example/rex-ffmpeg@sha256:111").is_ok()
        );
    }

    #[test]
    fn docker_invocation_is_offline_hardened_and_uses_guest_paths() {
        let runtime = catalog::runtime(ToolProgram::ImageMagickIdentify);
        let arguments = vec![
            "/work/inputs/input-0000.png".to_string(),
            "/work/outputs/output-0000.txt".to_string(),
        ];
        let arguments = docker_arguments(
            Path::new("/host/workspace"),
            "rex-tool-test",
            images().image(runtime.bundle),
            runtime,
            &arguments,
            true,
        );
        let arguments: Vec<_> = arguments
            .iter()
            .map(|argument| argument.to_string_lossy())
            .collect();

        assert_eq!(
            arguments,
            [
                "run",
                "--rm",
                "--name",
                "rex-tool-test",
                "--pull=never",
                "--network=none",
                "--read-only",
                "--cap-drop=ALL",
                "--security-opt=no-new-privileges",
                "--no-healthcheck",
                "--mount",
                "type=bind,source=/host/workspace,destination=/work",
                "--workdir",
                "/work",
                "--env",
                "MAGICK_TEMPORARY_PATH=/work/scratch",
                "--env",
                "TMPDIR=/work/scratch",
                "--entrypoint",
                "magick",
                "--interactive",
                "example/rex-imagemagick@sha256:222",
                "identify",
                "/work/inputs/input-0000.png",
                "/work/outputs/output-0000.txt",
            ]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn executor_preserves_stdin_stdout_stderr_and_exit_status() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().unwrap();
        let fake_docker = temporary.path().join("docker");
        std::fs::write(
            &fake_docker,
            "#!/bin/sh\n\
             mount=''\n\
             while [ \"$#\" -gt 0 ]; do\n\
               if [ \"$1\" = '--mount' ]; then mount=$2; shift 2; else shift; fi\n\
             done\n\
             workspace=${mount#type=bind,source=}\n\
             workspace=${workspace%,destination=/work}\n\
             printf 'fake output' > \"$workspace/outputs/output-0000.bin\"\n\
             printf 'fake diagnostic' >&2\n\
             cat\n\
             exit 7\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake_docker).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&fake_docker, permissions).unwrap();

        let store = Store::new_in_memory();
        let stdin = store.put(b"fake stdin".to_vec()).await.unwrap();
        let executor = DockerToolExecutor::new(images()).with_docker_binary(fake_docker);
        let execution = executor
            .execute(
                &store,
                ToolExecutionPlan {
                    program: ToolProgram::Ffmpeg,
                    arguments: Vec::new(),
                    inputs: Vec::new(),
                    outputs: vec![ExpectedOutput {
                        kind: OutputKind::Single,
                        extension: "bin".to_string(),
                    }],
                    stdin: Some(stdin),
                },
            )
            .await
            .unwrap();

        assert_eq!(execution.exit_code, Some(7));
        assert_eq!(execution.stdout, b"fake stdin");
        assert_eq!(execution.stderr, b"fake diagnostic");
        let output = execution.outputs.get(&0).unwrap();
        assert_eq!(output.len(), 1);
        assert_eq!(store.get(output[0]).await.unwrap(), b"fake output");
    }
}
