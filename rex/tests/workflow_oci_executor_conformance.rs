use rex::storage::{EntryKind, Store};
use rex::workflow::executor::{
    CasInput, DockerOciJobExecutor, ExpectedOutput, FakeRemoteOciExecutor, FakeRemoteRunner,
    InputKind, OciExecutorCapabilities, OciImage, OciIsolationPolicy, OciJob, OciJobExecutor,
    OciJobFuture, OciJobLimits, OciPlatform, OutputKind, ToolArgument, ToolExecution,
    ToolExecutionErrorKind,
};
use std::{
    collections::BTreeMap,
    future::pending,
    sync::{Arc, Mutex},
    time::Duration,
};

const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[derive(Default)]
struct RunnerState {
    jobs: Vec<OciJob>,
    active: usize,
    cleanups: usize,
}

#[derive(Clone, Default)]
struct ProtocolRunner {
    state: Arc<Mutex<RunnerState>>,
}

struct ActiveGuard(Arc<Mutex<RunnerState>>);

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        let mut state = self.0.lock().unwrap();
        state.active -= 1;
        state.cleanups += 1;
    }
}

impl FakeRemoteRunner for ProtocolRunner {
    fn run<'a>(&'a self, store: &'a Store, job: OciJob) -> OciJobFuture<'a> {
        Box::pin(async move {
            {
                let mut state = self.state.lock().unwrap();
                state.jobs.push(job.clone());
                state.active += 1;
            }
            let _guard = ActiveGuard(self.state.clone());
            let wait = job.arguments.iter().any(
                |argument| matches!(argument, ToolArgument::Literal(value) if value == "wait"),
            );
            if wait {
                pending::<()>().await;
            }

            let ordinary_failure = job.arguments.iter().any(
                |argument| matches!(argument, ToolArgument::Literal(value) if value == "fail"),
            );
            if ordinary_failure {
                return Ok(ToolExecution {
                    exit_code: Some(7),
                    stdout: Vec::new(),
                    stderr: b"ordinary tool failure".to_vec(),
                    outputs: BTreeMap::new(),
                    provenance: None,
                });
            }

            let fallback_blob = store
                .put(b"remote output".to_vec())
                .await
                .map_err(|error| {
                    rex::workflow::executor::ToolExecutionError::with_kind(
                        ToolExecutionErrorKind::Infrastructure,
                        format!("store fake remote output: {error}"),
                    )
                })?;
            let input_blob = job
                .inputs
                .iter()
                .find(|input| input.kind == InputKind::Blob)
                .map(|input| input.hash)
                .unwrap_or(fallback_blob);
            let tree_blob = store.put(b"nested remote output".to_vec()).await.unwrap();
            let tree = store
                .put_tree(BTreeMap::from([(
                    "nested.txt".to_owned(),
                    (EntryKind::Blob, tree_blob),
                )]))
                .await
                .unwrap();

            let mut outputs: BTreeMap<_, _> = job
                .outputs
                .iter()
                .enumerate()
                .map(|(id, output)| {
                    let hash = match output.kind {
                        OutputKind::Tree => tree,
                        OutputKind::Single | OutputKind::Numbered | OutputKind::Directory => {
                            input_blob
                        }
                    };
                    (id, vec![hash])
                })
                .collect();
            if job.arguments.iter().any(
                |argument| matches!(argument, ToolArgument::Literal(value) if value == "duplicate-single"),
            ) {
                outputs.insert(0, vec![input_blob, fallback_blob]);
            }
            if job.arguments.iter().any(
                |argument| matches!(argument, ToolArgument::Literal(value) if value == "omit-output"),
            ) {
                outputs.remove(&0);
            }
            Ok(ToolExecution {
                exit_code: Some(0),
                stdout: b"remote stdout".to_vec(),
                stderr: Vec::new(),
                outputs,
                provenance: None,
            })
        })
    }
}

fn image(platform: OciPlatform) -> OciImage {
    OciImage {
        name: "fixture".to_owned(),
        reference: format!("registry.example/runtime@sha256:{DIGEST}"),
        platform,
    }
}

fn job(executor: &dyn OciJobExecutor) -> OciJob {
    OciJob {
        image: image(executor.target_platform()),
        command: vec!["fixture".to_owned()],
        arguments: Vec::new(),
        inputs: Vec::new(),
        outputs: Vec::new(),
        limits: OciJobLimits {
            execution_timeout: Duration::from_secs(5),
            max_stdout_bytes: 1024,
            max_stderr_bytes: 1024,
            max_output_bytes: 1024 * 1024,
            temporary_storage_bytes: 16 * 1024 * 1024,
            memory_bytes: 512 * 1024 * 1024,
            cpu_count: 1,
            pid_limit: 64,
        },
        isolation: OciIsolationPolicy::default(),
    }
}

async fn assert_pre_dispatch_conformance(executor: &dyn OciJobExecutor) {
    let store = Store::new_in_memory();

    let mut invalid = job(executor);
    invalid.command.clear();
    assert_eq!(
        executor.execute(&store, invalid).await.unwrap_err().kind(),
        ToolExecutionErrorKind::InvalidJob
    );

    let mut unknown_slot = job(executor);
    unknown_slot.arguments.push(ToolArgument::input(0));
    assert_eq!(
        executor
            .execute(&store, unknown_slot)
            .await
            .unwrap_err()
            .kind(),
        ToolExecutionErrorKind::InvalidJob
    );

    let mut incompatible = job(executor);
    incompatible.image.platform = OciPlatform::new("linux", "incompatible", None).unwrap();
    assert_eq!(
        executor
            .execute(&store, incompatible)
            .await
            .unwrap_err()
            .kind(),
        ToolExecutionErrorKind::Unsupported
    );

    let mut zero_limit = job(executor);
    zero_limit.limits.pid_limit = 0;
    assert_eq!(
        executor
            .execute(&store, zero_limit)
            .await
            .unwrap_err()
            .kind(),
        ToolExecutionErrorKind::InvalidJob
    );
}

fn fake(runner: Arc<ProtocolRunner>) -> FakeRemoteOciExecutor {
    FakeRemoteOciExecutor::new(
        "fake-cloud",
        OciPlatform::new("linux", "amd64", None).unwrap(),
        OciExecutorCapabilities::secure(),
        runner,
    )
    .unwrap()
}

#[tokio::test]
async fn docker_and_remote_backends_share_pre_dispatch_validation() {
    assert_pre_dispatch_conformance(&DockerOciJobExecutor::new()).await;
    assert_pre_dispatch_conformance(&fake(Arc::new(ProtocolRunner::default()))).await;
}

#[tokio::test]
async fn fake_remote_transfers_only_declared_cas_data_and_records_provenance() {
    let runner = Arc::new(ProtocolRunner::default());
    let executor = fake(runner.clone());
    let store = Store::new_in_memory();
    let blob = store.put(b"declared input".to_vec()).await.unwrap();
    let nested = store.put(b"nested input".to_vec()).await.unwrap();
    let tree = store
        .put_tree(BTreeMap::from([(
            "nested.txt".to_owned(),
            (EntryKind::Blob, nested),
        )]))
        .await
        .unwrap();
    let mut execution_job = job(&executor);
    execution_job.inputs = vec![
        CasInput {
            hash: blob,
            extension: "bin".to_owned(),
            kind: InputKind::Blob,
        },
        CasInput {
            hash: tree,
            extension: "tree".to_owned(),
            kind: InputKind::Tree,
        },
    ];
    execution_job.outputs = vec![
        ExpectedOutput {
            kind: OutputKind::Single,
            extension: "bin".to_owned(),
        },
        ExpectedOutput {
            kind: OutputKind::Tree,
            extension: "tree".to_owned(),
        },
    ];

    let execution = executor.execute(&store, execution_job).await.unwrap();
    assert_eq!(
        store.get(execution.outputs[&0][0]).await.unwrap(),
        b"declared input"
    );
    assert!(
        store
            .get_tree(execution.outputs[&1][0])
            .await
            .unwrap()
            .contains_key("nested.txt")
    );
    assert!(executor.remote_store().get(blob).await.is_ok());
    assert!(executor.remote_store().get_tree(tree).await.is_ok());
    let provenance = execution.provenance.unwrap();
    assert_eq!(provenance.executor, "fake-cloud");
    assert_eq!(provenance.inputs, vec![blob, tree]);
    assert_eq!(provenance.outputs, execution.outputs);

    let state = runner.state.lock().unwrap();
    assert_eq!(state.active, 0);
    assert_eq!(state.cleanups, 1);
    assert_eq!(state.jobs[0].isolation, OciIsolationPolicy::default());
}

#[tokio::test]
async fn remote_boundary_distinguishes_failures_limits_and_missing_inputs() {
    let executor = fake(Arc::new(ProtocolRunner::default()));
    let store = Store::new_in_memory();

    let mut failure = job(&executor);
    failure.arguments.push(ToolArgument::literal("fail"));
    assert_eq!(
        executor.execute(&store, failure).await.unwrap().exit_code,
        Some(7)
    );

    let mut oversized = job(&executor);
    oversized.outputs.push(ExpectedOutput {
        kind: OutputKind::Single,
        extension: "bin".to_owned(),
    });
    oversized.limits.max_output_bytes = 3;
    assert_eq!(
        executor
            .execute(&store, oversized)
            .await
            .unwrap_err()
            .kind(),
        ToolExecutionErrorKind::ResultProtocol
    );

    let mut missing = job(&executor);
    missing.inputs.push(CasInput {
        hash: blake3::hash(b"missing"),
        extension: "bin".to_owned(),
        kind: InputKind::Blob,
    });
    assert_eq!(
        executor.execute(&store, missing).await.unwrap_err().kind(),
        ToolExecutionErrorKind::Infrastructure
    );
}

#[tokio::test]
async fn remote_boundary_rejects_malformed_output_shapes() {
    let executor = fake(Arc::new(ProtocolRunner::default()));
    let store = Store::new_in_memory();

    for marker in ["duplicate-single", "omit-output"] {
        let mut malformed = job(&executor);
        malformed.outputs.push(ExpectedOutput {
            kind: OutputKind::Single,
            extension: "bin".to_owned(),
        });
        malformed.arguments.push(ToolArgument::literal(marker));
        assert_eq!(
            executor
                .execute(&store, malformed)
                .await
                .unwrap_err()
                .kind(),
            ToolExecutionErrorKind::ResultProtocol
        );
    }
}

#[tokio::test]
async fn remote_boundary_rejects_mutable_images_and_unsupported_policy() {
    let runner = Arc::new(ProtocolRunner::default());
    let executor = fake(runner.clone());
    let store = Store::new_in_memory();
    let mut mutable = job(&executor);
    mutable.image.reference = "registry.example/runtime:latest".to_owned();
    assert_eq!(
        executor.execute(&store, mutable).await.unwrap_err().kind(),
        ToolExecutionErrorKind::InvalidJob
    );

    let incapable = FakeRemoteOciExecutor::new(
        "incapable-cloud",
        OciPlatform::new("linux", "amd64", None).unwrap(),
        OciExecutorCapabilities::default(),
        runner,
    )
    .unwrap();
    assert_eq!(
        incapable
            .execute(&store, job(&incapable))
            .await
            .unwrap_err()
            .kind(),
        ToolExecutionErrorKind::Unsupported
    );
}

#[tokio::test]
async fn remote_timeout_and_cancellation_release_service_work() {
    let runner = Arc::new(ProtocolRunner::default());
    let executor = Arc::new(fake(runner.clone()));
    let store = Store::new_in_memory();
    let mut timed = job(executor.as_ref());
    timed.arguments.push(ToolArgument::literal("wait"));
    timed.limits.execution_timeout = Duration::from_millis(20);
    assert_eq!(
        executor.execute(&store, timed).await.unwrap_err().kind(),
        ToolExecutionErrorKind::Timeout
    );
    assert_eq!(runner.state.lock().unwrap().active, 0);

    let mut cancelled = job(executor.as_ref());
    cancelled.arguments.push(ToolArgument::literal("wait"));
    cancelled.limits.execution_timeout = Duration::from_secs(30);
    let task_executor = executor.clone();
    let task_store = store.clone();
    let task = tokio::spawn(async move { task_executor.execute(&task_store, cancelled).await });
    for _ in 0..100 {
        if runner.state.lock().unwrap().active == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(runner.state.lock().unwrap().active, 1);
    task.abort();
    let _ = task.await;
    assert_eq!(runner.state.lock().unwrap().active, 0);
    assert_eq!(runner.state.lock().unwrap().cleanups, 2);
}
