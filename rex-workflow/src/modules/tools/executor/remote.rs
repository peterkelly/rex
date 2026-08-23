//! In-memory remote boundary used to validate cloud-ready OCI execution.

use super::{
    InputKind, OciDigest, OciExecutorCapabilities, OciJob, OciJobExecutor, OciJobFuture,
    OciPlatform, OutputKind, ToolExecutionError, ToolExecutionErrorKind, ToolExecutionProvenance,
    validate_oci_job,
};
use blake3::Hash;
use futures::future::BoxFuture;
use rex::storage::{EntryKind, Store};
use std::{collections::BTreeMap, fmt, str::FromStr, sync::Arc};

/// Service-side behavior for the fake remote target.
pub trait FakeRemoteRunner: Send + Sync {
    fn run<'a>(&'a self, remote_store: &'a Store, job: OciJob) -> OciJobFuture<'a>;
}

/// A protocol test double with a CAS distinct from the caller's store.
///
/// This is not a production cloud executor. It models input transfer, remote
/// execution, declared-result transfer, limits, and provenance without bind
/// mounts or developer-machine paths.
#[derive(Clone)]
pub struct FakeRemoteOciExecutor {
    executor_id: Arc<str>,
    platform: OciPlatform,
    capabilities: OciExecutorCapabilities,
    remote_store: Store,
    runner: Arc<dyn FakeRemoteRunner>,
}

impl FakeRemoteOciExecutor {
    pub fn new(
        executor_id: impl Into<String>,
        platform: OciPlatform,
        capabilities: OciExecutorCapabilities,
        runner: Arc<dyn FakeRemoteRunner>,
    ) -> Result<Self, ToolExecutionError> {
        let executor_id = executor_id.into();
        if executor_id.is_empty()
            || executor_id.len() > 255
            || executor_id
                .bytes()
                .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
        {
            return Err(ToolExecutionError::with_kind(
                ToolExecutionErrorKind::InvalidJob,
                "invalid fake remote executor identity",
            ));
        }
        Ok(Self {
            executor_id: Arc::from(executor_id),
            platform,
            capabilities,
            remote_store: Store::new_in_memory(),
            runner,
        })
    }

    pub fn remote_store(&self) -> &Store {
        &self.remote_store
    }
}

impl fmt::Debug for FakeRemoteOciExecutor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FakeRemoteOciExecutor")
            .field("executor_id", &self.executor_id)
            .field("platform", &self.platform)
            .field("capabilities", &self.capabilities)
            .finish_non_exhaustive()
    }
}

impl OciJobExecutor for FakeRemoteOciExecutor {
    fn executor_id(&self) -> &str {
        &self.executor_id
    }

    fn target_platform(&self) -> OciPlatform {
        self.platform.clone()
    }

    fn capabilities(&self) -> OciExecutorCapabilities {
        self.capabilities.clone()
    }

    fn execute<'a>(&'a self, store: &'a Store, job: OciJob) -> OciJobFuture<'a> {
        Box::pin(async move {
            validate_oci_job(&job, &self.platform, &self.capabilities)?;
            let image_digest = manifest_digest(&job.image.reference)?;
            for input in &job.inputs {
                transfer_object(
                    store,
                    &self.remote_store,
                    input.hash,
                    match input.kind {
                        InputKind::Blob => EntryKind::Blob,
                        InputKind::Tree => EntryKind::Tree,
                    },
                    ToolExecutionErrorKind::Infrastructure,
                )
                .await?;
            }

            let mut execution = tokio::time::timeout(
                job.limits.execution_timeout,
                self.runner.run(&self.remote_store, job.clone()),
            )
            .await
            .map_err(|_| {
                ToolExecutionError::with_kind(
                    ToolExecutionErrorKind::Timeout,
                    "remote OCI job exceeded its execution timeout",
                )
            })??;
            if execution.stdout.len() > job.limits.max_stdout_bytes
                || execution.stderr.len() > job.limits.max_stderr_bytes
            {
                return Err(ToolExecutionError::with_kind(
                    ToolExecutionErrorKind::ResultProtocol,
                    "remote result exceeded its stdout or stderr limit",
                ));
            }
            if execution.outputs.len() != job.outputs.len()
                || execution.outputs.keys().any(|id| *id >= job.outputs.len())
            {
                return Err(ToolExecutionError::with_kind(
                    ToolExecutionErrorKind::ResultProtocol,
                    "remote result does not contain exactly the declared output slots",
                ));
            }

            let mut output_bytes = 0_u64;
            for (id, hashes) in &execution.outputs {
                let expected = &job.outputs[*id];
                match expected.kind {
                    OutputKind::Single if hashes.len() > 1 => {
                        return Err(result_error(
                            "remote result returned multiple objects for a single output",
                        ));
                    }
                    OutputKind::Tree if hashes.len() != 1 => {
                        return Err(result_error(
                            "remote result must return exactly one tree object for a tree output",
                        ));
                    }
                    _ => {}
                }
                let kind = match expected.kind {
                    OutputKind::Tree => EntryKind::Tree,
                    OutputKind::Single | OutputKind::Numbered | OutputKind::Directory => {
                        EntryKind::Blob
                    }
                };
                for hash in hashes {
                    output_bytes = output_bytes
                        .checked_add(remote_object_size(&self.remote_store, *hash, kind).await?)
                        .ok_or_else(|| result_error("remote result size overflowed"))?;
                    if output_bytes > job.limits.max_output_bytes {
                        return Err(result_error("remote result exceeded its output size limit"));
                    }
                    transfer_object(
                        &self.remote_store,
                        store,
                        *hash,
                        kind,
                        ToolExecutionErrorKind::ResultProtocol,
                    )
                    .await?;
                }
            }
            execution.provenance = Some(ToolExecutionProvenance {
                executor: self.executor_id.to_string(),
                platform: self.platform.clone(),
                image_digest,
                inputs: job.inputs.iter().map(|input| input.hash).collect(),
                outputs: execution.outputs.clone(),
            });
            Ok(execution)
        })
    }
}

fn result_error(message: impl Into<String>) -> ToolExecutionError {
    ToolExecutionError::with_kind(ToolExecutionErrorKind::ResultProtocol, message)
}

async fn remote_object_size(
    store: &Store,
    hash: Hash,
    kind: EntryKind,
) -> Result<u64, ToolExecutionError> {
    let mut size = store
        .size(hash)
        .await
        .map_err(|error| result_error(format!("inspect remote output {hash}: {error}")))?;
    if kind == EntryKind::Tree {
        let entries = store
            .get_tree(hash)
            .await
            .map_err(|error| result_error(format!("inspect remote output tree {hash}: {error}")))?;
        for entry in entries.values() {
            size = size
                .checked_add(entry.size)
                .ok_or_else(|| result_error("remote output tree size overflowed"))?;
        }
    }
    Ok(size)
}

fn manifest_digest(reference: &str) -> Result<OciDigest, ToolExecutionError> {
    let digest = reference
        .rsplit_once('@')
        .map(|(_, digest)| digest)
        .unwrap_or(reference);
    OciDigest::from_str(digest).map_err(|_| {
        ToolExecutionError::with_kind(
            ToolExecutionErrorKind::InvalidJob,
            "remote execution requires a digest-qualified OCI image manifest",
        )
    })
}

fn transfer_object<'a>(
    source: &'a Store,
    destination: &'a Store,
    hash: Hash,
    kind: EntryKind,
    error_kind: ToolExecutionErrorKind,
) -> BoxFuture<'a, Result<(), ToolExecutionError>> {
    Box::pin(async move {
        let transferred = match kind {
            EntryKind::Blob => {
                let bytes = source.get(hash).await.map_err(|error| {
                    ToolExecutionError::with_kind(
                        error_kind,
                        format!("transfer CAS blob {hash}: {error}"),
                    )
                })?;
                destination.put(bytes).await.map_err(|error| {
                    ToolExecutionError::with_kind(
                        error_kind,
                        format!("store transferred CAS blob {hash}: {error}"),
                    )
                })?
            }
            EntryKind::Tree => {
                let entries = source.get_tree(hash).await.map_err(|error| {
                    ToolExecutionError::with_kind(
                        error_kind,
                        format!("transfer CAS tree {hash}: {error}"),
                    )
                })?;
                let mut creations = BTreeMap::new();
                for (name, entry) in entries {
                    transfer_object(source, destination, entry.hash, entry.kind, error_kind)
                        .await?;
                    creations.insert(name, (entry.kind, entry.hash));
                }
                destination.put_tree(creations).await.map_err(|error| {
                    ToolExecutionError::with_kind(
                        error_kind,
                        format!("store transferred CAS tree {hash}: {error}"),
                    )
                })?
            }
        };
        if transferred != hash {
            return Err(ToolExecutionError::with_kind(
                error_kind,
                format!("CAS transfer changed object identity from {hash} to {transferred}"),
            ));
        }
        Ok(())
    })
}
