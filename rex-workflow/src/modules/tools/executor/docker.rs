use super::{
    OciDigest, OciExecutorCapabilities, OciIsolationPolicy, OciJob, OciJobExecutor, OciJobFuture,
    OciJobLimits, OciPlatform, OciToolExecutor, OciToolImages, ToolExecution, ToolExecutionError,
    ToolExecutionErrorKind, ToolExecutionPlan, ToolExecutionProvenance, ToolExecutor, ToolFuture,
    validate_oci_job, workspace::ToolWorkspace,
};
use rex::storage::Store;
use serde::Deserialize;
use std::{
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
    process::{Command as StdCommand, ExitStatus, Stdio},
    str::FromStr,
    sync::Arc,
    thread,
    time::Duration,
};
use tokio::{process::Command, time::timeout};
use uuid::Uuid;

const CONTAINER_WORKSPACE: &str = "/work";
const CONTAINER_CONTROL: &str = "/work/control";
const CONTAINER_INPUTS: &str = "/work/inputs";
const CONTAINER_OUTPUTS: &str = "/work/outputs";
const CONTAINER_RESULTS: &str = "/work/results";
const CONTAINER_TMP: &str = "/work/tmp";

/// Executes tool plans in one-shot Docker containers on the local machine.
#[derive(Clone)]
pub struct DockerToolExecutor {
    executor: OciToolExecutor,
    backend: Arc<DockerOciJobExecutor>,
}

/// Docker-specific implementation of the provider-neutral OCI job contract.
#[derive(Clone, Debug)]
pub struct DockerOciJobExecutor {
    docker_binary: PathBuf,
    platform: OciPlatform,
}

impl DockerToolExecutor {
    pub fn new(images: OciToolImages) -> Self {
        let backend = Arc::new(DockerOciJobExecutor::new());
        Self {
            executor: OciToolExecutor::new(images, backend.clone()),
            backend,
        }
    }

    /// Override the Docker CLI binary used to manage containers.
    pub fn with_docker_binary(mut self, binary: impl Into<PathBuf>) -> Self {
        let backend = Arc::new(self.backend.as_ref().clone().with_docker_binary(binary));
        self.executor = OciToolExecutor::new(self.executor.images.clone(), backend.clone())
            .with_job_limits(self.executor.limits.clone())
            .with_isolation_policy(self.executor.isolation.clone());
        self.backend = backend;
        self
    }

    pub fn with_execution_timeout(mut self, execution_timeout: Duration) -> Self {
        self.executor.limits.execution_timeout = execution_timeout;
        self
    }

    pub fn with_max_output_bytes(mut self, max_output_bytes: usize) -> Self {
        self.executor.limits.max_stdout_bytes = max_output_bytes;
        self.executor.limits.max_stderr_bytes = max_output_bytes;
        self
    }

    pub fn with_job_limits(mut self, limits: OciJobLimits) -> Self {
        self.executor.limits = limits;
        self
    }

    pub fn with_isolation_policy(mut self, isolation: OciIsolationPolicy) -> Self {
        self.executor.isolation = isolation;
        self
    }

    pub fn oci_backend(&self) -> &DockerOciJobExecutor {
        &self.backend
    }
}

impl DockerOciJobExecutor {
    pub fn new() -> Self {
        Self {
            docker_binary: PathBuf::from("docker"),
            platform: OciPlatform::native_linux(),
        }
    }

    pub fn with_docker_binary(mut self, binary: impl Into<PathBuf>) -> Self {
        self.docker_binary = binary.into();
        self
    }
}

impl Default for DockerOciJobExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolExecutor for DockerToolExecutor {
    fn execute<'a>(&'a self, store: &'a Store, plan: ToolExecutionPlan) -> ToolFuture<'a> {
        self.executor.execute(store, plan)
    }
}

impl OciJobExecutor for DockerOciJobExecutor {
    fn executor_id(&self) -> &str {
        "docker"
    }

    fn target_platform(&self) -> OciPlatform {
        self.platform.clone()
    }

    fn capabilities(&self) -> OciExecutorCapabilities {
        OciExecutorCapabilities::secure()
    }

    fn execute<'a>(&'a self, store: &'a Store, job: OciJob) -> OciJobFuture<'a> {
        Box::pin(execute_docker_job(self, store, job))
    }
}

pub fn docker_executor(images: OciToolImages) -> Arc<dyn ToolExecutor> {
    Arc::new(DockerToolExecutor::new(images))
}

async fn execute_docker_job(
    executor: &DockerOciJobExecutor,
    store: &Store,
    job: OciJob,
) -> Result<ToolExecution, ToolExecutionError> {
    validate_oci_job(&job, &executor.platform, &executor.capabilities())?;
    let workspace = ToolWorkspace::prepare(store, &job.inputs, &job.outputs).await?;
    let arguments = workspace.render_arguments(&job.arguments, Path::new(CONTAINER_WORKSPACE))?;
    let (executable, prefix_arguments) = job.command.split_first().expect("validated OCI command");
    let wrapper_arguments = workspace.wrapper_arguments(
        Path::new(CONTAINER_WORKSPACE),
        executable,
        prefix_arguments,
        &arguments,
    );
    let host_control = canonical_directory(&workspace.control_dir(), "Docker control directory")?;
    let host_inputs = canonical_directory(&workspace.input_dir(), "Docker input directory")?;
    let host_outputs = canonical_directory(&workspace.output_dir(), "Docker output directory")?;
    let host_results = canonical_directory(&workspace.result_dir(), "Docker result directory")?;
    let invocation_id = Uuid::new_v4().to_string();
    let container_name = format!("rex-tool-{invocation_id}");
    let create_arguments = create_arguments(
        &host_control,
        &host_inputs,
        &host_outputs,
        &host_results,
        &container_name,
        &invocation_id,
        &job.image.reference,
        &wrapper_arguments,
        &job.limits,
    );

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

    let start_arguments = [
        OsString::from("start"),
        OsString::from("--attach"),
        OsString::from(&container_name),
    ];

    let mut command = Command::new(&executor.docker_binary);
    command
        .args(&start_arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|error| {
        ToolExecutionError::new(format!(
            "start Docker CLI `{}`: {error}",
            executor.docker_binary.display()
        ))
    })?;

    match timeout(job.limits.execution_timeout, child.wait()).await {
        Ok(result) => result
            .map_err(|error| ToolExecutionError::new(format!("wait for Docker tool: {error}")))?,
        Err(_) => {
            let _ = child.kill().await;
            let cleanup_result = cleanup.remove().await;
            return Err(match cleanup_result {
                Ok(()) => ToolExecutionError::with_kind(
                    ToolExecutionErrorKind::Timeout,
                    format!(
                        "Docker tool exceeded the {:?} execution timeout",
                        job.limits.execution_timeout
                    ),
                ),
                Err(error) => ToolExecutionError::with_kind(
                    ToolExecutionErrorKind::Timeout,
                    format!(
                        "Docker tool exceeded the {:?} execution timeout; cleanup also failed: {error}",
                        job.limits.execution_timeout
                    ),
                ),
            });
        }
    };

    let state_result = inspect_state(&executor.docker_binary, &container_name).await;
    let digest_result = match super::image_digest(&job.image.reference) {
        Some(digest) => Ok(digest),
        None => inspect_image_digest(&executor.docker_binary, &container_name).await,
    };
    let cleanup_result = cleanup.remove().await;
    let state = state_result?;
    let image_digest = digest_result?;
    cleanup_result?;

    validate_completed_state(&state)?;
    let result = workspace.read_result(job.limits.max_stdout_bytes, job.limits.max_stderr_bytes)?;
    let outputs = workspace
        .import_outputs(store, &job.outputs, job.limits.max_output_bytes)
        .await?;
    Ok(ToolExecution {
        exit_code: Some(result.exit_code),
        stdout: result.stdout,
        stderr: result.stderr,
        provenance: Some(ToolExecutionProvenance {
            executor: executor.executor_id().to_owned(),
            platform: executor.target_platform(),
            image_digest,
            inputs: job.inputs.iter().map(|input| input.hash).collect(),
            outputs: outputs.clone(),
        }),
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

#[allow(clippy::too_many_arguments)]
fn create_arguments(
    host_control: &Path,
    host_inputs: &Path,
    host_outputs: &Path,
    host_results: &Path,
    container_name: &str,
    invocation_id: &str,
    image: &str,
    wrapper_arguments: &[OsString],
    limits: &OciJobLimits,
) -> Vec<OsString> {
    let control_mount = bind_mount(host_control, CONTAINER_CONTROL, true);
    let input_mount = bind_mount(host_inputs, CONTAINER_INPUTS, true);
    let output_mount = bind_mount(host_outputs, CONTAINER_OUTPUTS, false);
    let result_mount = bind_mount(host_results, CONTAINER_RESULTS, false);
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
        OsString::from(limits.pid_limit.to_string()),
        OsString::from("--memory"),
        OsString::from(limits.memory_bytes.to_string()),
        OsString::from("--cpus"),
        OsString::from(limits.cpu_count.to_string()),
        OsString::from("--user"),
        OsString::from(user),
        OsString::from("--label"),
        OsString::from("rex.workflow=true"),
        OsString::from("--label"),
        OsString::from(format!("rex.workflow.invocation={invocation_id}")),
        OsString::from("--mount"),
        control_mount,
        OsString::from("--mount"),
        input_mount,
        OsString::from("--mount"),
        output_mount,
        OsString::from("--mount"),
        result_mount,
        OsString::from("--tmpfs"),
        OsString::from(format!(
            "{CONTAINER_TMP}:rw,noexec,nosuid,nodev,mode=1777,size={}",
            limits.temporary_storage_bytes
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
        OsString::from("/bin/sh"),
        OsString::from(image),
    ];
    docker_arguments.extend(wrapper_arguments.iter().cloned());
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
    let uid = nix::unistd::geteuid().as_raw();
    let gid = nix::unistd::getegid().as_raw();
    if uid == 0 || gid == 0 {
        "65532:65532".to_owned()
    } else {
        format!("{uid}:{gid}")
    }
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

async fn inspect_image_digest(
    docker_binary: &Path,
    container_name: &str,
) -> Result<OciDigest, ToolExecutionError> {
    let output = docker_output(
        docker_binary,
        [
            OsStr::new("inspect"),
            OsStr::new("--format={{.Image}}"),
            OsStr::new(container_name),
        ],
        "inspect Docker tool image",
    )
    .await?;
    if !output.status.success() {
        return Err(docker_failure(
            "inspect Docker tool image",
            output.status,
            &output.stderr,
        ));
    }
    OciDigest::from_str(String::from_utf8_lossy(&output.stdout).trim()).map_err(|_| {
        ToolExecutionError::with_kind(
            ToolExecutionErrorKind::ResultProtocol,
            "Docker returned an invalid immutable image digest",
        )
    })
}

fn validate_completed_state(state: &ContainerState) -> Result<(), ToolExecutionError> {
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

        let _ = spawn_container_cleanup(self.docker_binary.clone(), self.container_name.clone());
    }
}

fn spawn_container_cleanup(
    docker_binary: PathBuf,
    container_name: String,
) -> io::Result<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("rex-docker-cleanup".to_owned())
        .spawn(move || {
            // Spawn the Docker CLI inside this thread so every cleanup process
            // that starts has an owner which waits for and reaps it. Dropping
            // the JoinHandle detaches only the Rust thread, not the child.
            let _ = StdCommand::new(docker_binary)
                .args(["container", "rm", "--force", &container_name])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::tools::executor::{
        ExpectedOutput, OutputKind, ToolBundle, ToolProgram, catalog,
    };

    const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn images() -> OciToolImages {
        OciToolImages::new(
            OciPlatform::native_linux(),
            format!("example/rex-ffmpeg@sha256:{DIGEST}"),
            format!("example/rex-gnuplot@sha256:{DIGEST}"),
            format!("example/rex-graphviz@sha256:{DIGEST}"),
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
        let images = OciToolImages::new(
            OciPlatform::native_linux(),
            "ffmpeg:latest",
            "gnuplot:latest",
            "graphviz:latest",
            "magick:latest",
            "qpdf:latest",
            "poppler:latest",
        );
        assert!(images.validate().is_err());
        let images = OciToolImages::development(
            OciPlatform::native_linux(),
            "ffmpeg:latest",
            "gnuplot:latest",
            "graphviz:latest",
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
            assert!(
                super::super::validate_image_reference(ToolBundle::Ffmpeg, image, true).is_err()
            );
        }
    }

    #[test]
    fn docker_create_is_offline_hardened_and_separates_mounts() {
        let runtime = catalog::runtime(ToolProgram::ImageMagickIdentify);
        let wrapper_arguments = [
            OsString::from("/work/control/invoke.sh"),
            OsString::from("/work/results/stdout"),
            OsString::from("/work/results/stderr"),
            OsString::from("/work/results/exit-code"),
            OsString::from(runtime.executable),
            OsString::from("identify"),
            OsString::from("/work/inputs/input.png"),
        ];
        let arguments = create_arguments(
            Path::new("/host/control"),
            Path::new("/host/inputs"),
            Path::new("/host/outputs"),
            Path::new("/host/results"),
            "rex-tool-test",
            "invocation-id",
            &images().image(runtime.bundle).reference,
            &wrapper_arguments,
            &OciJobLimits {
                temporary_storage_bytes: 1024,
                pid_limit: 12,
                ..OciJobLimits::default()
            },
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
            "12",
            "--memory",
            "4294967296",
            "--cpus",
            "4",
            "--user",
            "rex.workflow=true",
            "rex.workflow.invocation=invocation-id",
            "type=bind,source=/host/control,destination=/work/control,readonly",
            "type=bind,source=/host/inputs,destination=/work/inputs,readonly",
            "type=bind,source=/host/outputs,destination=/work/outputs",
            "type=bind,source=/host/results,destination=/work/results",
            "/work/tmp:rw,noexec,nosuid,nodev,mode=1777,size=1024",
            "/bin/sh",
            "/work/control/invoke.sh",
            "/work/results/stdout",
            "/work/results/stderr",
            "/work/results/exit-code",
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

    #[cfg(unix)]
    #[test]
    fn cleanup_reaper_waits_for_docker_process() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().unwrap();
        let fake_docker = temporary.path().join("docker");
        std::fs::write(
            &fake_docker,
            "#!/bin/sh\n\
             [ \"$1\" = container ] || exit 2\n\
             [ \"$2\" = rm ] || exit 3\n\
             [ \"$3\" = --force ] || exit 4\n\
             [ \"$4\" = rex-tool-test ] || exit 5\n\
             sleep 0.1\n\
             printf reaped > \"$0.reaped\"\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake_docker).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&fake_docker, permissions).unwrap();

        let reaper =
            spawn_container_cleanup(fake_docker.clone(), "rex-tool-test".to_owned()).unwrap();
        reaper.join().unwrap();

        assert_eq!(
            std::fs::read(fake_docker.with_extension("reaped")).unwrap(),
            b"reaped"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn executor_reads_file_result_and_ignores_transport_io_and_status() {
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
                     case $2 in\n\
                       *destination=/work/outputs*) printf '%s' \"$2\" > \"$state.outputs\";;\n\
                       *destination=/work/results*) printf '%s' \"$2\" > \"$state.results\";;\n\
                     esac\n\
                     shift 2\n\
                   else shift; fi\n\
                 done\n\
                 printf 'container-id\\n'\n\
                 ;;\n\
               start)\n\
                 output=$(cat \"$state.outputs\")\n\
                 output=${output#type=bind,source=}\n\
                 output=${output%,destination=/work/outputs}\n\
                 results=$(cat \"$state.results\")\n\
                 results=${results#type=bind,source=}\n\
                 results=${results%,destination=/work/results}\n\
                 printf 'fake output' > \"$output/output-0000.bin\"\n\
                 printf 'recorded stdout' > \"$results/stdout\"\n\
                 printf 'recorded stderr' > \"$results/stderr\"\n\
                 printf '7\\n' > \"$results/exit-code\"\n\
                 printf 'ignored transport stdout'\n\
                 printf 'ignored transport stderr' >&2\n\
                 exit 99\n\
                 ;;\n\
               inspect)\n\
                 printf '{\"Running\":false,\"OOMKilled\":false,\"Dead\":false,\"ExitCode\":99,\"Error\":\"\"}\\n'\n\
                 ;;\n\
               container)\n\
                 rm -f \"$state.outputs\" \"$state.results\"\n\
                 ;;\n\
             esac\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake_docker).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&fake_docker, permissions).unwrap();
        let store = Store::new_in_memory();
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
                },
            )
            .await
            .unwrap();

        assert_eq!(execution.exit_code, Some(7));
        assert_eq!(execution.stdout, b"recorded stdout");
        assert_eq!(execution.stderr, b"recorded stderr");
        let output = execution.outputs.get(&0).unwrap();
        assert_eq!(store.get(output[0]).await.unwrap(), b"fake output");
        assert!(!fake_docker.with_extension("state.outputs").exists());
        assert!(!fake_docker.with_extension("state.results").exists());
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
                },
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("execution timeout"));
        assert!(!fake_docker.with_extension("state").exists());
    }
}
