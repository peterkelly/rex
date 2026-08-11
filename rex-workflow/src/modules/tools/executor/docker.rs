use super::{
    ToolBundle, ToolExecution, ToolExecutionError, ToolExecutionPlan, ToolExecutor, ToolFuture,
    catalog::{self, ToolRuntime},
    workspace::ToolWorkspace,
};
use crate::storage::store::Store;
use serde::Deserialize;
use std::{
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
    process::{Command as StdCommand, ExitStatus, Stdio},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    task::JoinHandle,
    time::timeout,
};
use uuid::Uuid;

const CONTAINER_WORKSPACE: &str = "/work";
const CONTAINER_INPUTS: &str = "/work/inputs";
const CONTAINER_OUTPUTS: &str = "/work/outputs";
const CONTAINER_TMP: &str = "/work/tmp";
const DEFAULT_EXECUTION_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const DEFAULT_MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_TMPFS_BYTES: u64 = 512 * 1024 * 1024;
const DEFAULT_PID_LIMIT: u32 = 512;
const TRUNCATION_MARKER: &[u8] = b"\n[rex: tool output truncated]\n";

/// Container image references for each supported tool bundle.
///
/// `new` requires immutable digest-qualified references. Mutable tags are
/// accepted only through `development`, which makes the weaker provisioning
/// policy explicit at the embedding boundary. Images must already be present;
/// executing a workflow never pulls them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DockerToolImages {
    ffmpeg: String,
    image_magick: String,
    qpdf: String,
    poppler: String,
    allow_tags: bool,
}

impl DockerToolImages {
    pub fn new(
        ffmpeg: impl Into<String>,
        image_magick: impl Into<String>,
        qpdf: impl Into<String>,
        poppler: impl Into<String>,
    ) -> Self {
        Self::configured(ffmpeg, image_magick, qpdf, poppler, false)
    }

    /// Configure mutable image tags for local image development.
    pub fn development(
        ffmpeg: impl Into<String>,
        image_magick: impl Into<String>,
        qpdf: impl Into<String>,
        poppler: impl Into<String>,
    ) -> Self {
        Self::configured(ffmpeg, image_magick, qpdf, poppler, true)
    }

    fn configured(
        ffmpeg: impl Into<String>,
        image_magick: impl Into<String>,
        qpdf: impl Into<String>,
        poppler: impl Into<String>,
        allow_tags: bool,
    ) -> Self {
        Self {
            ffmpeg: ffmpeg.into(),
            image_magick: image_magick.into(),
            qpdf: qpdf.into(),
            poppler: poppler.into(),
            allow_tags,
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

    pub fn iter(&self) -> impl Iterator<Item = (ToolBundle, &str)> {
        ToolBundle::ALL
            .into_iter()
            .map(|bundle| (bundle, self.image(bundle)))
    }

    pub fn allows_tags(&self) -> bool {
        self.allow_tags
    }

    pub fn with_image(mut self, bundle: ToolBundle, image: impl Into<String>) -> Self {
        let image = image.into();
        match bundle {
            ToolBundle::Ffmpeg => self.ffmpeg = image,
            ToolBundle::ImageMagick => self.image_magick = image,
            ToolBundle::Qpdf => self.qpdf = image,
            ToolBundle::Poppler => self.poppler = image,
        }
        self
    }

    pub fn with_tags_allowed(mut self, allow_tags: bool) -> Self {
        self.allow_tags = allow_tags;
        self
    }

    pub fn validate(&self) -> Result<(), ToolExecutionError> {
        for (bundle, image) in self.iter() {
            validate_image_reference(bundle, image, self.allow_tags)?;
        }
        Ok(())
    }
}

/// Executes tool plans in one-shot Docker containers on the local machine.
#[derive(Clone, Debug)]
pub struct DockerToolExecutor {
    docker_binary: PathBuf,
    images: DockerToolImages,
    execution_timeout: Duration,
    max_output_bytes: usize,
    tmpfs_bytes: u64,
    pid_limit: u32,
}

impl DockerToolExecutor {
    pub fn new(images: DockerToolImages) -> Self {
        Self {
            docker_binary: PathBuf::from("docker"),
            images,
            execution_timeout: DEFAULT_EXECUTION_TIMEOUT,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            tmpfs_bytes: DEFAULT_TMPFS_BYTES,
            pid_limit: DEFAULT_PID_LIMIT,
        }
    }

    /// Override the Docker CLI binary used to manage containers.
    pub fn with_docker_binary(mut self, binary: impl Into<PathBuf>) -> Self {
        self.docker_binary = binary.into();
        self
    }

    pub fn with_execution_timeout(mut self, execution_timeout: Duration) -> Self {
        self.execution_timeout = execution_timeout;
        self
    }

    pub fn with_max_output_bytes(mut self, max_output_bytes: usize) -> Self {
        self.max_output_bytes = max_output_bytes;
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
    executor.images.validate()?;
    let workspace = ToolWorkspace::prepare(store, &plan.inputs, &plan.outputs).await?;
    let runtime = catalog::runtime(plan.program);
    let arguments = workspace.render_arguments(&plan.arguments, Path::new(CONTAINER_WORKSPACE))?;
    let host_inputs = canonical_directory(&workspace.input_dir(), "Docker input directory")?;
    let host_outputs = canonical_directory(&workspace.output_dir(), "Docker output directory")?;
    let invocation_id = Uuid::new_v4().to_string();
    let container_name = format!("rex-tool-{invocation_id}");
    let image = executor.images.image(runtime.bundle);
    let create_arguments = create_arguments(
        &host_inputs,
        &host_outputs,
        &container_name,
        &invocation_id,
        image,
        runtime,
        &arguments,
        executor.tmpfs_bytes,
        executor.pid_limit,
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

    let mut cleanup = ContainerCleanup::new(&executor.docker_binary, container_name.clone());
    let create = docker_output(
        &executor.docker_binary,
        &create_arguments,
        "create Docker tool container",
    )
    .await?;
    if !create.status.success() {
        return Err(docker_failure(
            "create Docker tool container",
            create.status,
            &create.stderr,
        ));
    }

    let mut start_arguments = vec![OsString::from("start"), OsString::from("--attach")];
    if stdin_data.is_some() {
        start_arguments.push(OsString::from("--interactive"));
    }
    start_arguments.push(OsString::from(&container_name));

    let mut command = Command::new(&executor.docker_binary);
    command
        .args(&start_arguments)
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
            "start Docker CLI `{}`: {error}",
            executor.docker_binary.display()
        ))
    })?;

    let stdin_writer = spawn_stdin_writer(&mut child, stdin_data)?;
    let stdout_reader = spawn_output_reader(
        child
            .stdout
            .take()
            .ok_or_else(|| ToolExecutionError::new("Docker CLI has no stdout pipe"))?,
        executor.max_output_bytes,
    );
    let stderr_reader = spawn_output_reader(
        child
            .stderr
            .take()
            .ok_or_else(|| ToolExecutionError::new("Docker CLI has no stderr pipe"))?,
        executor.max_output_bytes,
    );

    let start_status = match timeout(executor.execution_timeout, child.wait()).await {
        Ok(result) => result
            .map_err(|error| ToolExecutionError::new(format!("wait for Docker tool: {error}")))?,
        Err(_) => {
            let _ = child.kill().await;
            let cleanup_result = cleanup.remove().await;
            let _ = join_writer(stdin_writer).await;
            let _ = join_reader(stdout_reader, "stdout").await;
            let _ = join_reader(stderr_reader, "stderr").await;
            return Err(match cleanup_result {
                Ok(()) => ToolExecutionError::new(format!(
                    "Docker tool exceeded the {:?} execution timeout",
                    executor.execution_timeout
                )),
                Err(error) => ToolExecutionError::new(format!(
                    "Docker tool exceeded the {:?} execution timeout; cleanup also failed: {error}",
                    executor.execution_timeout
                )),
            });
        }
    };
    join_writer(stdin_writer).await?;
    let stdout = join_reader(stdout_reader, "stdout").await?;
    let stderr = join_reader(stderr_reader, "stderr").await?;

    let state_result = inspect_state(&executor.docker_binary, &container_name).await;
    let cleanup_result = cleanup.remove().await;
    let state = state_result?;
    cleanup_result?;

    validate_completed_state(&state, start_status, &stderr)?;
    let outputs = workspace.import_outputs(store, &plan.outputs).await?;
    Ok(ToolExecution {
        exit_code: Some(state.exit_code),
        stdout,
        stderr,
        outputs,
    })
}

fn canonical_directory(path: &Path, description: &str) -> Result<PathBuf, ToolExecutionError> {
    let canonical = std::fs::canonicalize(path)
        .map_err(|error| ToolExecutionError::new(format!("resolve {description}: {error}")))?;
    if canonical.as_os_str().as_encoded_bytes().contains(&b',') {
        return Err(ToolExecutionError::new(format!(
            "{description} contains a comma, which cannot be represented safely in a Docker --mount argument"
        )));
    }
    Ok(canonical)
}

fn spawn_stdin_writer(
    child: &mut tokio::process::Child,
    data: Option<Vec<u8>>,
) -> Result<Option<JoinHandle<io::Result<()>>>, ToolExecutionError> {
    let Some(data) = data else {
        return Ok(None);
    };
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| ToolExecutionError::new("Docker CLI has no stdin pipe"))?;
    Ok(Some(tokio::spawn(async move {
        stdin.write_all(&data).await?;
        stdin.shutdown().await
    })))
}

fn spawn_output_reader<R>(reader: R, limit: usize) -> JoinHandle<io::Result<Vec<u8>>>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(read_bounded(reader, limit))
}

async fn read_bounded<R>(mut reader: R, limit: usize) -> io::Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let mut output = Vec::with_capacity(limit.min(8192));
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        let remaining = limit.saturating_sub(output.len());
        output.extend_from_slice(&buffer[..count.min(remaining)]);
        truncated |= count > remaining;
    }
    if truncated && limit >= TRUNCATION_MARKER.len() {
        output.truncate(limit - TRUNCATION_MARKER.len());
        output.extend_from_slice(TRUNCATION_MARKER);
    }
    Ok(output)
}

async fn join_writer(writer: Option<JoinHandle<io::Result<()>>>) -> Result<(), ToolExecutionError> {
    let Some(writer) = writer else {
        return Ok(());
    };
    match writer.await {
        Err(error) => Err(ToolExecutionError::new(format!(
            "join Docker stdin writer: {error}"
        ))),
        Ok(Err(error)) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Ok(Err(error)) => Err(ToolExecutionError::new(format!(
            "write Docker tool stdin: {error}"
        ))),
        Ok(Ok(())) => Ok(()),
    }
}

async fn join_reader(
    reader: JoinHandle<io::Result<Vec<u8>>>,
    stream: &str,
) -> Result<Vec<u8>, ToolExecutionError> {
    reader
        .await
        .map_err(|error| ToolExecutionError::new(format!("join Docker {stream} reader: {error}")))?
        .map_err(|error| ToolExecutionError::new(format!("read Docker {stream}: {error}")))
}

pub(super) fn validate_image_reference(
    bundle: ToolBundle,
    image: &str,
    allow_tags: bool,
) -> Result<(), ToolExecutionError> {
    if image.is_empty()
        || image.starts_with('-')
        || image
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(ToolExecutionError::new(format!(
            "invalid Docker image reference for {bundle}"
        )));
    }
    if !allow_tags && !has_sha256_digest(image) {
        return Err(ToolExecutionError::new(format!(
            "Docker image for {bundle} must be digest-qualified; mutable tags are available only through DockerToolImages::development"
        )));
    }
    Ok(())
}

fn has_sha256_digest(image: &str) -> bool {
    let Some((name, digest)) = image.rsplit_once("@sha256:") else {
        return false;
    };
    !name.is_empty() && digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[allow(clippy::too_many_arguments)]
fn create_arguments(
    host_inputs: &Path,
    host_outputs: &Path,
    container_name: &str,
    invocation_id: &str,
    image: &str,
    runtime: ToolRuntime,
    arguments: &[String],
    tmpfs_bytes: u64,
    pid_limit: u32,
) -> Vec<OsString> {
    let input_mount = bind_mount(host_inputs, CONTAINER_INPUTS, true);
    let output_mount = bind_mount(host_outputs, CONTAINER_OUTPUTS, false);
    let user = host_user();
    let mut docker_arguments = vec![
        OsString::from("create"),
        OsString::from("--name"),
        OsString::from(container_name),
        OsString::from("--pull=never"),
        OsString::from("--network=none"),
        OsString::from("--read-only"),
        OsString::from("--cap-drop=ALL"),
        OsString::from("--security-opt=no-new-privileges"),
        OsString::from("--no-healthcheck"),
        OsString::from("--pids-limit"),
        OsString::from(pid_limit.to_string()),
        OsString::from("--user"),
        OsString::from(user),
        OsString::from("--label"),
        OsString::from("rex.workflow=true"),
        OsString::from("--label"),
        OsString::from(format!("rex.workflow.invocation={invocation_id}")),
        OsString::from("--mount"),
        input_mount,
        OsString::from("--mount"),
        output_mount,
        OsString::from("--tmpfs"),
        OsString::from(format!(
            "{CONTAINER_TMP}:rw,noexec,nosuid,nodev,mode=1777,size={tmpfs_bytes}"
        )),
        OsString::from("--workdir"),
        OsString::from(CONTAINER_WORKSPACE),
        OsString::from("--env"),
        OsString::from(format!("HOME={CONTAINER_TMP}")),
        OsString::from("--env"),
        OsString::from(format!("MAGICK_TEMPORARY_PATH={CONTAINER_TMP}")),
        OsString::from("--env"),
        OsString::from(format!("TMPDIR={CONTAINER_TMP}")),
        OsString::from("--env"),
        OsString::from("LANG=C"),
        OsString::from("--env"),
        OsString::from("LC_ALL=C"),
        OsString::from("--env"),
        OsString::from("TZ=UTC"),
        OsString::from("--entrypoint"),
        OsString::from(runtime.container_executable),
        OsString::from(image),
    ];
    docker_arguments.extend(runtime.prefix_arguments.iter().map(OsString::from));
    docker_arguments.extend(arguments.iter().map(OsString::from));
    docker_arguments
}

fn bind_mount(source: &Path, destination: &str, read_only: bool) -> OsString {
    let mut mount = OsString::from("type=bind,source=");
    mount.push(source);
    mount.push(",destination=");
    mount.push(destination);
    if read_only {
        mount.push(",readonly");
    }
    mount
}

#[cfg(unix)]
fn host_user() -> String {
    format!(
        "{}:{}",
        nix::unistd::geteuid().as_raw(),
        nix::unistd::getegid().as_raw()
    )
}

#[cfg(not(unix))]
fn host_user() -> String {
    "65532:65532".to_owned()
}

async fn docker_output<I, S>(
    docker_binary: &Path,
    arguments: I,
    action: &str,
) -> Result<std::process::Output, ToolExecutionError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(docker_binary)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|error| {
            ToolExecutionError::new(format!(
                "{action} through `{}`: {error}",
                docker_binary.display()
            ))
        })
}

fn docker_failure(action: &str, status: ExitStatus, stderr: &[u8]) -> ToolExecutionError {
    let diagnostic = String::from_utf8_lossy(stderr);
    let diagnostic = diagnostic.trim();
    let diagnostic = if diagnostic.is_empty() {
        "Docker returned no diagnostic"
    } else {
        diagnostic
    };
    ToolExecutionError::new(format!(
        "{action} failed with status {status}: {diagnostic}"
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ContainerState {
    running: bool,
    #[serde(rename = "OOMKilled")]
    oom_killed: bool,
    dead: bool,
    exit_code: i32,
    error: String,
}

async fn inspect_state(
    docker_binary: &Path,
    container_name: &str,
) -> Result<ContainerState, ToolExecutionError> {
    let output = docker_output(
        docker_binary,
        [
            OsStr::new("inspect"),
            OsStr::new("--format={{json .State}}"),
            OsStr::new(container_name),
        ],
        "inspect Docker tool container",
    )
    .await?;
    if !output.status.success() {
        return Err(docker_failure(
            "inspect Docker tool container",
            output.status,
            &output.stderr,
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| ToolExecutionError::new(format!("parse Docker container state: {error}")))
}

fn validate_completed_state(
    state: &ContainerState,
    start_status: ExitStatus,
    start_stderr: &[u8],
) -> Result<(), ToolExecutionError> {
    if state.running {
        return Err(ToolExecutionError::new(
            "Docker attach ended while the tool container was still running",
        ));
    }
    if state.dead {
        return Err(ToolExecutionError::new(
            "Docker reported the tool container as dead",
        ));
    }
    if state.oom_killed {
        return Err(ToolExecutionError::new(
            "Docker killed the tool container after it exhausted memory",
        ));
    }
    if !state.error.is_empty() {
        return Err(ToolExecutionError::new(format!(
            "Docker reported a container runtime error: {}",
            state.error
        )));
    }
    if !start_status.success() && start_status.code() != Some(state.exit_code) {
        return Err(docker_failure(
            "attach to Docker tool container",
            start_status,
            start_stderr,
        ));
    }
    Ok(())
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

    async fn remove(&mut self) -> Result<(), ToolExecutionError> {
        if !self.armed {
            return Ok(());
        }
        let output = docker_output(
            &self.docker_binary,
            [
                OsStr::new("container"),
                OsStr::new("rm"),
                OsStr::new("--force"),
                OsStr::new(&self.container_name),
            ],
            "remove Docker tool container",
        )
        .await?;
        if !output.status.success() {
            return Err(docker_failure(
                "remove Docker tool container",
                output.status,
                &output.stderr,
            ));
        }
        self.armed = false;
        Ok(())
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

    const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn images() -> DockerToolImages {
        DockerToolImages::new(
            format!("example/rex-ffmpeg@sha256:{DIGEST}"),
            format!("example/rex-imagemagick@sha256:{DIGEST}"),
            format!("example/rex-qpdf@sha256:{DIGEST}"),
            format!("example/rex-poppler@sha256:{DIGEST}"),
        )
    }

    #[test]
    fn image_configuration_covers_every_bundle() {
        let images = images();
        assert_eq!(images.iter().count(), ToolBundle::ALL.len());
        assert!(images.validate().is_ok());
        assert!(!images.allows_tags());
    }

    #[test]
    fn mutable_image_references_are_explicitly_development_only() {
        let images = DockerToolImages::new(
            "ffmpeg:latest",
            "magick:latest",
            "qpdf:latest",
            "poppler:latest",
        );
        assert!(images.validate().is_err());
        let images = DockerToolImages::development(
            "ffmpeg:latest",
            "magick:latest",
            "qpdf:latest",
            "poppler:latest",
        );
        assert!(images.validate().is_ok());
        assert!(images.allows_tags());
    }

    #[test]
    fn image_references_cannot_be_interpreted_as_docker_options() {
        for image in ["", "--privileged", "image with spaces", "image\n--volume"] {
            assert!(validate_image_reference(ToolBundle::Ffmpeg, image, true).is_err());
        }
    }

    #[test]
    fn docker_create_is_offline_hardened_and_separates_mounts() {
        let runtime = catalog::runtime(ToolProgram::ImageMagickIdentify);
        let arguments = create_arguments(
            Path::new("/host/inputs"),
            Path::new("/host/outputs"),
            "rex-tool-test",
            "invocation-id",
            images().image(runtime.bundle),
            runtime,
            &["/work/inputs/input.png".to_owned()],
            1024,
            12,
        );
        let arguments: Vec<_> = arguments
            .iter()
            .map(|argument| argument.to_string_lossy())
            .collect();

        for required in [
            "--pull=never",
            "--network=none",
            "--read-only",
            "--cap-drop=ALL",
            "--security-opt=no-new-privileges",
            "--pids-limit",
            "--user",
            "rex.workflow=true",
            "rex.workflow.invocation=invocation-id",
            "type=bind,source=/host/inputs,destination=/work/inputs,readonly",
            "type=bind,source=/host/outputs,destination=/work/outputs",
            "/work/tmp:rw,noexec,nosuid,nodev,mode=1777,size=1024",
        ] {
            assert!(
                arguments.iter().any(|argument| argument == required),
                "missing {required}"
            );
        }
        assert!(!arguments.iter().any(|argument| argument == "--privileged"));
        assert!(
            !arguments
                .iter()
                .any(|argument| argument.contains("/host/workspace"))
        );
    }

    #[test]
    fn docker_mount_paths_reject_mount_option_delimiters() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("source,readonly=false");
        std::fs::create_dir(&directory).unwrap();

        let error = canonical_directory(&directory, "test directory").unwrap_err();
        assert!(error.to_string().contains("contains a comma"));
    }

    #[tokio::test]
    async fn bounded_reader_drains_and_marks_truncated_output() {
        let data = vec![b'x'; 128];
        let output = read_bounded(std::io::Cursor::new(data), 64).await.unwrap();
        assert_eq!(output.len(), 64);
        assert!(output.ends_with(TRUNCATION_MARKER));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn executor_preserves_io_exit_status_and_removes_container() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().unwrap();
        let fake_docker = temporary.path().join("docker");
        std::fs::write(
            &fake_docker,
            "#!/bin/sh\n\
             state=$0.state\n\
             case $1 in\n\
               create)\n\
                 shift\n\
                 while [ $# -gt 0 ]; do\n\
                   if [ \"$1\" = '--mount' ]; then\n\
                     case $2 in *destination=/work/outputs*) printf '%s' \"$2\" > \"$state\";; esac\n\
                     shift 2\n\
                   else shift; fi\n\
                 done\n\
                 printf 'container-id\\n'\n\
                 ;;\n\
               start)\n\
                 mount=$(cat \"$state\")\n\
                 output=${mount#type=bind,source=}\n\
                 output=${output%,destination=/work/outputs}\n\
                 printf 'fake output' > \"$output/output-0000.bin\"\n\
                 printf 'fake diagnostic' >&2\n\
                 cat\n\
                 exit 7\n\
                 ;;\n\
               inspect)\n\
                 printf '{\"Running\":false,\"OOMKilled\":false,\"Dead\":false,\"ExitCode\":7,\"Error\":\"\"}\\n'\n\
                 ;;\n\
               container)\n\
                 rm -f \"$state\"\n\
                 ;;\n\
             esac\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake_docker).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&fake_docker, permissions).unwrap();

        let store = Store::new_in_memory();
        let stdin = store.put(b"fake stdin".to_vec()).await.unwrap();
        let executor = DockerToolExecutor::new(images()).with_docker_binary(&fake_docker);
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
        assert_eq!(store.get(output[0]).await.unwrap(), b"fake output");
        assert!(!fake_docker.with_extension("state").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn executor_timeout_force_removes_the_container() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().unwrap();
        let fake_docker = temporary.path().join("docker");
        std::fs::write(
            &fake_docker,
            "#!/bin/sh\n\
             state=$0.state\n\
             case $1 in\n\
               create) touch \"$state\"; printf 'container-id\\n';;\n\
               start) exec sleep 30;;\n\
               container) rm -f \"$state\";;\n\
             esac\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake_docker).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&fake_docker, permissions).unwrap();

        let executor = DockerToolExecutor::new(images())
            .with_docker_binary(&fake_docker)
            .with_execution_timeout(Duration::from_millis(25));
        let error = executor
            .execute(
                &Store::new_in_memory(),
                ToolExecutionPlan {
                    program: ToolProgram::Ffmpeg,
                    arguments: Vec::new(),
                    inputs: Vec::new(),
                    outputs: Vec::new(),
                    stdin: None,
                },
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("execution timeout"));
        assert!(!fake_docker.with_extension("state").exists());
    }
}
