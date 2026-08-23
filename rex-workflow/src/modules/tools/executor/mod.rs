mod catalog;
mod docker;
mod remote;
mod workspace;

use blake3::Hash;
use rex::storage::Store;
use std::{
    collections::BTreeMap, error::Error, fmt, future::Future, pin::Pin, str::FromStr, sync::Arc,
    time::Duration,
};

pub use docker::{DockerOciJobExecutor, DockerToolExecutor, docker_executor};
pub use remote::{FakeRemoteOciExecutor, FakeRemoteRunner};

const DEFAULT_MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_EXECUTION_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const DEFAULT_TMPFS_BYTES: u64 = 512 * 1024 * 1024;
const DEFAULT_MEMORY_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const DEFAULT_CPU_COUNT: u32 = 4;
const DEFAULT_PID_LIMIT: u32 = 512;

pub type InputId = usize;
pub type OutputId = usize;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PathSlot {
    Input(InputId),
    InputParent(InputId),
    Output(OutputId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolArgument {
    Literal(String),
    Path {
        slot: PathSlot,
        prefix: String,
        suffix: String,
    },
    Joined(Vec<ToolArgument>),
}

impl ToolArgument {
    pub fn literal(value: impl Into<String>) -> Self {
        Self::Literal(value.into())
    }

    pub fn input(id: InputId) -> Self {
        Self::Path {
            slot: PathSlot::Input(id),
            prefix: String::new(),
            suffix: String::new(),
        }
    }

    pub fn input_decorated(
        id: InputId,
        prefix: impl Into<String>,
        suffix: impl Into<String>,
    ) -> Self {
        Self::Path {
            slot: PathSlot::Input(id),
            prefix: prefix.into(),
            suffix: suffix.into(),
        }
    }

    pub fn input_parent_decorated(
        id: InputId,
        prefix: impl Into<String>,
        suffix: impl Into<String>,
    ) -> Self {
        Self::Path {
            slot: PathSlot::InputParent(id),
            prefix: prefix.into(),
            suffix: suffix.into(),
        }
    }

    pub fn output(id: OutputId) -> Self {
        Self::Path {
            slot: PathSlot::Output(id),
            prefix: String::new(),
            suffix: String::new(),
        }
    }

    pub fn output_decorated(id: OutputId, prefix: impl Into<String>) -> Self {
        Self::Path {
            slot: PathSlot::Output(id),
            prefix: prefix.into(),
            suffix: String::new(),
        }
    }

    pub fn output_with_suffix(id: OutputId, suffix: impl Into<String>) -> Self {
        Self::Path {
            slot: PathSlot::Output(id),
            prefix: String::new(),
            suffix: suffix.into(),
        }
    }

    pub fn joined(parts: Vec<ToolArgument>) -> Self {
        Self::Joined(parts)
    }
}

#[derive(Clone, Debug)]
pub struct CasInput {
    pub hash: Hash,
    pub extension: String,
    pub kind: InputKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputKind {
    Blob,
    Tree,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputKind {
    Single,
    Numbered,
    Directory,
    Tree,
}

#[derive(Clone, Debug)]
pub struct ExpectedOutput {
    pub kind: OutputKind,
    pub extension: String,
}

/// An executor-neutral tool invocation expressed only through arguments and files.
///
/// Standard input is always closed. Executors stage `inputs`, resolve symbolic paths in
/// `arguments`, and collect `outputs` after the backend reports that the job has finished.
#[derive(Clone, Debug)]
pub struct ToolExecutionPlan {
    pub program: ToolProgram,
    pub arguments: Vec<ToolArgument>,
    pub inputs: Vec<CasInput>,
    pub outputs: Vec<ExpectedOutput>,
}

/// One headless external program that the workflow host may execute.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolProgram {
    Ffmpeg,
    Ffprobe,
    ImageMagick,
    ImageMagickMogrify,
    ImageMagickIdentify,
    ImageMagickCompare,
    ImageMagickComposite,
    ImageMagickMontage,
    ImageMagickStream,
    Qpdf,
    PdfInfo,
    PdfToText,
    PdfToCairo,
    PdfImages,
    Gnuplot,
    Graphviz,
}

/// A set of programs installed together in one tool runtime image.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ToolBundle {
    Ffmpeg,
    Gnuplot,
    Graphviz,
    ImageMagick,
    Qpdf,
    Poppler,
}

impl ToolProgram {
    /// Return the runtime image bundle containing this program.
    pub fn bundle(self) -> ToolBundle {
        catalog::runtime(self).bundle
    }
}

impl ToolBundle {
    pub const ALL: [Self; 6] = [
        Self::Ffmpeg,
        Self::Gnuplot,
        Self::Graphviz,
        Self::ImageMagick,
        Self::Qpdf,
        Self::Poppler,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Ffmpeg => "ffmpeg",
            Self::Gnuplot => "gnuplot",
            Self::Graphviz => "graphviz",
            Self::ImageMagick => "imagemagick",
            Self::Qpdf => "qpdf",
            Self::Poppler => "poppler",
        }
    }
}

impl fmt::Display for ToolBundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// An OCI target platform in `os/architecture[/variant]` form.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OciPlatform {
    pub os: String,
    pub architecture: String,
    pub variant: Option<String>,
}

impl OciPlatform {
    pub fn new(
        os: impl Into<String>,
        architecture: impl Into<String>,
        variant: Option<String>,
    ) -> Result<Self, ToolExecutionError> {
        let platform = Self {
            os: os.into(),
            architecture: architecture.into(),
            variant,
        };
        validate_platform_part(&platform.os)?;
        validate_platform_part(&platform.architecture)?;
        if let Some(variant) = &platform.variant {
            validate_platform_part(variant)?;
        }
        Ok(platform)
    }

    /// The native Linux OCI platform used by the local Docker backend.
    pub fn native_linux() -> Self {
        Self::new("linux", oci_architecture(), None)
            .expect("Rust target architecture is a valid OCI platform component")
    }
}

impl fmt::Display for OciPlatform {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.os, self.architecture)?;
        if let Some(variant) = &self.variant {
            write!(formatter, "/{variant}")?;
        }
        Ok(())
    }
}

impl FromStr for OciPlatform {
    type Err = ToolExecutionError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let parts = value.split('/').collect::<Vec<_>>();
        match parts.as_slice() {
            [os, architecture] => Self::new(*os, *architecture, None),
            [os, architecture, variant] => {
                Self::new(*os, *architecture, Some((*variant).to_owned()))
            }
            _ => Err(ToolExecutionError::with_kind(
                ToolExecutionErrorKind::InvalidJob,
                format!("OCI platform `{value}` must use OS/ARCHITECTURE[/VARIANT]"),
            )),
        }
    }
}

fn validate_platform_part(value: &str) -> Result<(), ToolExecutionError> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(ToolExecutionError::with_kind(
            ToolExecutionErrorKind::InvalidJob,
            format!("invalid OCI platform component `{value}`"),
        ));
    }
    Ok(())
}

fn oci_architecture() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        "arm" => "arm",
        "powerpc64" => "ppc64",
        "powerpc64le" => "ppc64le",
        "s390x" => "s390x",
        "riscv64" => "riscv64",
        architecture => architecture,
    }
}

/// A validated immutable OCI digest.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OciDigest(String);

impl OciDigest {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for OciDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for OciDigest {
    type Err = ToolExecutionError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let Some(hex) = value.strip_prefix("sha256:") else {
            return Err(ToolExecutionError::with_kind(
                ToolExecutionErrorKind::InvalidJob,
                "OCI digest must use sha256",
            ));
        };
        if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ToolExecutionError::with_kind(
                ToolExecutionErrorKind::InvalidJob,
                "OCI sha256 digest must contain 64 hexadecimal digits",
            ));
        }
        Ok(Self(format!("sha256:{}", hex.to_ascii_lowercase())))
    }
}

/// One host-approved OCI image for a tool bundle and target platform.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OciImage {
    pub bundle: ToolBundle,
    pub reference: String,
    pub platform: OciPlatform,
}

/// OCI image references for every supported tool bundle.
///
/// Production configuration requires digest-qualified references. Mutable
/// tags are accepted only by `development` for locally built images.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OciToolImages {
    images: BTreeMap<ToolBundle, OciImage>,
    allow_tags: bool,
}

impl OciToolImages {
    pub fn current_development() -> Self {
        Self::development(
            OciPlatform::native_linux(),
            "rex-tool-ffmpeg:local",
            "rex-tool-gnuplot:local",
            "rex-tool-graphviz:local",
            "rex-tool-imagemagick:local",
            "rex-tool-qpdf:local",
            "rex-tool-poppler:local",
        )
    }

    pub fn new(
        platform: OciPlatform,
        ffmpeg: impl Into<String>,
        gnuplot: impl Into<String>,
        graphviz: impl Into<String>,
        image_magick: impl Into<String>,
        qpdf: impl Into<String>,
        poppler: impl Into<String>,
    ) -> Self {
        Self::configured(
            platform,
            [
                (ToolBundle::Ffmpeg, ffmpeg.into()),
                (ToolBundle::Gnuplot, gnuplot.into()),
                (ToolBundle::Graphviz, graphviz.into()),
                (ToolBundle::ImageMagick, image_magick.into()),
                (ToolBundle::Qpdf, qpdf.into()),
                (ToolBundle::Poppler, poppler.into()),
            ],
            false,
        )
    }

    pub fn development(
        platform: OciPlatform,
        ffmpeg: impl Into<String>,
        gnuplot: impl Into<String>,
        graphviz: impl Into<String>,
        image_magick: impl Into<String>,
        qpdf: impl Into<String>,
        poppler: impl Into<String>,
    ) -> Self {
        Self::configured(
            platform,
            [
                (ToolBundle::Ffmpeg, ffmpeg.into()),
                (ToolBundle::Gnuplot, gnuplot.into()),
                (ToolBundle::Graphviz, graphviz.into()),
                (ToolBundle::ImageMagick, image_magick.into()),
                (ToolBundle::Qpdf, qpdf.into()),
                (ToolBundle::Poppler, poppler.into()),
            ],
            true,
        )
    }

    fn configured(
        platform: OciPlatform,
        images: impl IntoIterator<Item = (ToolBundle, String)>,
        allow_tags: bool,
    ) -> Self {
        Self {
            images: images
                .into_iter()
                .map(|(bundle, reference)| {
                    (
                        bundle,
                        OciImage {
                            bundle,
                            reference,
                            platform: platform.clone(),
                        },
                    )
                })
                .collect(),
            allow_tags,
        }
    }

    pub fn image(&self, bundle: ToolBundle) -> &OciImage {
        self.images
            .get(&bundle)
            .expect("OCI image configuration covers every tool bundle")
    }

    pub fn iter(&self) -> impl Iterator<Item = (ToolBundle, &OciImage)> {
        ToolBundle::ALL
            .into_iter()
            .map(|bundle| (bundle, self.image(bundle)))
    }

    pub fn allows_tags(&self) -> bool {
        self.allow_tags
    }

    pub fn with_image(mut self, bundle: ToolBundle, reference: impl Into<String>) -> Self {
        self.images
            .get_mut(&bundle)
            .expect("OCI image configuration covers every tool bundle")
            .reference = reference.into();
        self
    }

    pub fn with_tags_allowed(mut self, allow_tags: bool) -> Self {
        self.allow_tags = allow_tags;
        self
    }

    pub fn validate(&self) -> Result<(), ToolExecutionError> {
        for (bundle, image) in self.iter() {
            validate_image_reference(bundle, &image.reference, self.allow_tags)?;
        }
        Ok(())
    }
}

fn validate_image_reference(
    bundle: ToolBundle,
    reference: &str,
    allow_tags: bool,
) -> Result<(), ToolExecutionError> {
    if reference.is_empty()
        || reference.starts_with('-')
        || reference
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(ToolExecutionError::with_kind(
            ToolExecutionErrorKind::InvalidJob,
            format!("invalid OCI image reference for {bundle}"),
        ));
    }
    if !allow_tags && image_digest(reference).is_none() {
        return Err(ToolExecutionError::with_kind(
            ToolExecutionErrorKind::InvalidJob,
            format!("OCI image for {bundle} must be digest-qualified"),
        ));
    }
    Ok(())
}

fn image_digest(reference: &str) -> Option<OciDigest> {
    let (_, digest) = reference.rsplit_once('@')?;
    digest.parse().ok()
}

/// Resource and result bounds that every OCI backend must enforce.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OciJobLimits {
    pub execution_timeout: Duration,
    pub max_stdout_bytes: usize,
    pub max_stderr_bytes: usize,
    pub max_output_bytes: u64,
    pub temporary_storage_bytes: u64,
    pub memory_bytes: u64,
    pub cpu_count: u32,
    pub pid_limit: u32,
}

impl Default for OciJobLimits {
    fn default() -> Self {
        Self {
            execution_timeout: DEFAULT_EXECUTION_TIMEOUT,
            max_stdout_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            max_stderr_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            max_output_bytes: 1024 * 1024 * 1024,
            temporary_storage_bytes: DEFAULT_TMPFS_BYTES,
            memory_bytes: DEFAULT_MEMORY_BYTES,
            cpu_count: DEFAULT_CPU_COUNT,
            pid_limit: DEFAULT_PID_LIMIT,
        }
    }
}

/// Isolation guarantees requested by the trusted workflow host.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OciIsolationPolicy {
    pub network_disabled: bool,
    pub read_only_root: bool,
    pub read_only_inputs: bool,
    pub run_as_non_root: bool,
    pub drop_all_capabilities: bool,
    pub no_new_privileges: bool,
    pub no_devices: bool,
    pub no_secrets: bool,
    pub no_additional_mounts: bool,
}

impl Default for OciIsolationPolicy {
    fn default() -> Self {
        Self {
            network_disabled: true,
            read_only_root: true,
            read_only_inputs: true,
            run_as_non_root: true,
            drop_all_capabilities: true,
            no_new_privileges: true,
            no_devices: true,
            no_secrets: true,
            no_additional_mounts: true,
        }
    }
}

/// Controls an OCI execution target can enforce.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OciExecutorCapabilities {
    pub network_disabled: bool,
    pub read_only_root: bool,
    pub read_only_inputs: bool,
    pub run_as_non_root: bool,
    pub drop_all_capabilities: bool,
    pub no_new_privileges: bool,
    pub no_devices: bool,
    pub no_secrets: bool,
    pub no_additional_mounts: bool,
    pub execution_timeout: bool,
    pub stream_limits: bool,
    pub output_size_limit: bool,
    pub temporary_storage_limit: bool,
    pub memory_limit: bool,
    pub cpu_limit: bool,
    pub pid_limit: bool,
}

impl OciExecutorCapabilities {
    pub fn secure() -> Self {
        Self {
            network_disabled: true,
            read_only_root: true,
            read_only_inputs: true,
            run_as_non_root: true,
            drop_all_capabilities: true,
            no_new_privileges: true,
            no_devices: true,
            no_secrets: true,
            no_additional_mounts: true,
            execution_timeout: true,
            stream_limits: true,
            output_size_limit: true,
            temporary_storage_limit: true,
            memory_limit: true,
            cpu_limit: true,
            pid_limit: true,
        }
    }
}

/// Executor-neutral OCI work expressed only through logical CAS slots.
#[derive(Clone, Debug)]
pub struct OciJob {
    pub image: OciImage,
    pub command: Vec<String>,
    pub arguments: Vec<ToolArgument>,
    pub inputs: Vec<CasInput>,
    pub outputs: Vec<ExpectedOutput>,
    pub limits: OciJobLimits,
    pub isolation: OciIsolationPolicy,
}

/// A completed tool result reconstructed from backend-produced result and output files.
#[derive(Clone, Debug)]
pub struct ToolExecution {
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub outputs: BTreeMap<OutputId, Vec<Hash>>,
    pub provenance: Option<ToolExecutionProvenance>,
}

/// Immutable facts identifying an OCI execution and its CAS boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolExecutionProvenance {
    pub executor: String,
    pub platform: OciPlatform,
    pub image_digest: OciDigest,
    pub inputs: Vec<Hash>,
    pub outputs: BTreeMap<OutputId, Vec<Hash>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolExecutionErrorKind {
    InvalidJob,
    Unsupported,
    Infrastructure,
    Timeout,
    ResultProtocol,
}

#[derive(Debug)]
pub struct ToolExecutionError {
    kind: ToolExecutionErrorKind,
    message: String,
}

impl ToolExecutionError {
    pub(super) fn new(message: impl Into<String>) -> Self {
        Self::with_kind(ToolExecutionErrorKind::Infrastructure, message)
    }

    pub fn with_kind(kind: ToolExecutionErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> ToolExecutionErrorKind {
        self.kind
    }
}

impl fmt::Display for ToolExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for ToolExecutionError {}

pub type ToolFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ToolExecution, ToolExecutionError>> + Send + 'a>>;

/// Execute tool plans without using tool process streams or process status as result data.
///
/// Implementations may use backend-specific APIs for dispatch and lifecycle monitoring, but tool
/// stdout, stderr, and exit code must be recorded as files and retrieved after completion.
pub trait ToolExecutor: Send + Sync {
    fn execute<'a>(&'a self, store: &'a Store, plan: ToolExecutionPlan) -> ToolFuture<'a>;
}

pub type OciJobFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ToolExecution, ToolExecutionError>> + Send + 'a>>;

/// Executes a logical OCI job without exposing backend-specific staging.
pub trait OciJobExecutor: Send + Sync {
    fn executor_id(&self) -> &str;
    fn target_platform(&self) -> OciPlatform;
    fn capabilities(&self) -> OciExecutorCapabilities;
    fn execute<'a>(&'a self, store: &'a Store, job: OciJob) -> OciJobFuture<'a>;
}

/// Generic adapter from semantic tool plans to host-approved OCI jobs.
#[derive(Clone)]
pub struct OciToolExecutor {
    images: OciToolImages,
    backend: Arc<dyn OciJobExecutor>,
    limits: OciJobLimits,
    isolation: OciIsolationPolicy,
}

impl OciToolExecutor {
    pub fn new(images: OciToolImages, backend: Arc<dyn OciJobExecutor>) -> Self {
        Self {
            images,
            backend,
            limits: OciJobLimits::default(),
            isolation: OciIsolationPolicy::default(),
        }
    }

    pub fn with_job_limits(mut self, limits: OciJobLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn with_isolation_policy(mut self, isolation: OciIsolationPolicy) -> Self {
        self.isolation = isolation;
        self
    }

    pub fn backend(&self) -> &Arc<dyn OciJobExecutor> {
        &self.backend
    }
}

impl ToolExecutor for OciToolExecutor {
    fn execute<'a>(&'a self, store: &'a Store, plan: ToolExecutionPlan) -> ToolFuture<'a> {
        Box::pin(async move {
            self.images.validate()?;
            let runtime = catalog::runtime(plan.program);
            let image = self.images.image(runtime.bundle).clone();
            let mut command = vec![runtime.executable.to_owned()];
            command.extend(
                runtime
                    .prefix_arguments
                    .iter()
                    .map(|value| (*value).to_owned()),
            );
            let job = OciJob {
                image,
                command,
                arguments: plan.arguments,
                inputs: plan.inputs,
                outputs: plan.outputs,
                limits: self.limits.clone(),
                isolation: self.isolation.clone(),
            };
            let expected_inputs = job
                .inputs
                .iter()
                .map(|input| input.hash)
                .collect::<Vec<_>>();
            let expected_image_digest = image_digest(&job.image.reference);
            let execution = self.backend.execute(store, job).await?;
            let provenance = execution.provenance.as_ref().ok_or_else(|| {
                ToolExecutionError::with_kind(
                    ToolExecutionErrorKind::ResultProtocol,
                    "OCI executor omitted execution provenance",
                )
            })?;
            if provenance.executor != self.backend.executor_id()
                || provenance.platform != self.backend.target_platform()
                || provenance.inputs != expected_inputs
                || provenance.outputs != execution.outputs
                || expected_image_digest
                    .as_ref()
                    .is_some_and(|digest| digest != &provenance.image_digest)
            {
                return Err(ToolExecutionError::with_kind(
                    ToolExecutionErrorKind::ResultProtocol,
                    "OCI executor returned inconsistent execution provenance",
                ));
            }
            Ok(execution)
        })
    }
}

/// Validate job structure, target compatibility, and requested guarantees.
pub fn validate_oci_job(
    job: &OciJob,
    target_platform: &OciPlatform,
    capabilities: &OciExecutorCapabilities,
) -> Result<(), ToolExecutionError> {
    let invalid =
        |message| ToolExecutionError::with_kind(ToolExecutionErrorKind::InvalidJob, message);
    validate_image_reference(job.image.bundle, &job.image.reference, true)?;
    if &job.image.platform != target_platform {
        return Err(ToolExecutionError::with_kind(
            ToolExecutionErrorKind::Unsupported,
            format!(
                "OCI image platform {} is incompatible with executor platform {target_platform}",
                job.image.platform
            ),
        ));
    }
    if job.command.is_empty()
        || job
            .command
            .iter()
            .any(|part| part.is_empty() || part.contains('\0'))
    {
        return Err(invalid(
            "OCI job command must contain only non-empty arguments",
        ));
    }
    if job
        .arguments
        .iter()
        .any(|argument| !valid_job_argument(argument, job.inputs.len(), job.outputs.len()))
    {
        return Err(invalid(
            "OCI job argument contains a NUL byte or an unknown logical path slot",
        ));
    }
    if job.limits.execution_timeout.is_zero()
        || job.limits.max_stdout_bytes == 0
        || job.limits.max_stderr_bytes == 0
        || job.limits.max_output_bytes == 0
        || job.limits.temporary_storage_bytes == 0
        || job.limits.memory_bytes == 0
        || job.limits.cpu_count == 0
        || job.limits.pid_limit == 0
    {
        return Err(invalid("OCI job limits must be greater than zero"));
    }

    let required = [
        (
            job.isolation.network_disabled,
            capabilities.network_disabled,
            "network isolation",
        ),
        (
            job.isolation.read_only_root,
            capabilities.read_only_root,
            "a read-only root filesystem",
        ),
        (
            job.isolation.read_only_inputs,
            capabilities.read_only_inputs,
            "read-only inputs",
        ),
        (
            job.isolation.run_as_non_root,
            capabilities.run_as_non_root,
            "non-root execution",
        ),
        (
            job.isolation.drop_all_capabilities,
            capabilities.drop_all_capabilities,
            "dropping all Linux capabilities",
        ),
        (
            job.isolation.no_new_privileges,
            capabilities.no_new_privileges,
            "no-new-privileges behavior",
        ),
        (
            job.isolation.no_devices,
            capabilities.no_devices,
            "device exclusion",
        ),
        (
            job.isolation.no_secrets,
            capabilities.no_secrets,
            "secret exclusion",
        ),
        (
            job.isolation.no_additional_mounts,
            capabilities.no_additional_mounts,
            "additional-mount exclusion",
        ),
        (true, capabilities.execution_timeout, "an execution timeout"),
        (true, capabilities.stream_limits, "stdout and stderr limits"),
        (true, capabilities.output_size_limit, "an output-size limit"),
        (
            true,
            capabilities.temporary_storage_limit,
            "a temporary-storage limit",
        ),
        (true, capabilities.memory_limit, "a memory limit"),
        (true, capabilities.cpu_limit, "a CPU limit"),
        (true, capabilities.pid_limit, "a PID limit"),
    ];
    if let Some((_, _, name)) = required
        .into_iter()
        .find(|(requested, supported, _)| *requested && !*supported)
    {
        return Err(ToolExecutionError::with_kind(
            ToolExecutionErrorKind::Unsupported,
            format!("OCI executor cannot enforce {name}"),
        ));
    }
    Ok(())
}

fn valid_job_argument(argument: &ToolArgument, input_count: usize, output_count: usize) -> bool {
    match argument {
        ToolArgument::Literal(value) => !value.contains('\0'),
        ToolArgument::Path {
            slot,
            prefix,
            suffix,
        } => {
            !prefix.contains('\0')
                && !suffix.contains('\0')
                && match slot {
                    PathSlot::Input(id) | PathSlot::InputParent(id) => *id < input_count,
                    PathSlot::Output(id) => *id < output_count,
                }
        }
        ToolArgument::Joined(parts) => parts
            .iter()
            .all(|part| valid_job_argument(part, input_count, output_count)),
    }
}
