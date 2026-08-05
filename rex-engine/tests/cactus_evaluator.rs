use futures::{FutureExt, channel::oneshot};
use rex_engine::{
    AsyncCallExecutor, AsyncCallPolicy, Builder, CompileOptions, EngineError, ExecutionBounds,
    Module, NativeAsyncPermit, NativeFuture, ParallelismController, Value,
};
use rex_parser::parse as parse_rex;
use rex_typesystem::types::{BuiltinTypeId, Scheme, Type};
use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::task::{Context as TaskContext, Poll, Waker};
use std::time::Duration;

async fn eval_value<State>(
    source: &str,
    builder: Builder<State>,
) -> Result<(Value, Type), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let compiler = builder.build_compiler();
    let parsed = parse_rex(source).unwrap();
    let (program, evaluator) = compiler
        .compile_program(&parsed, CompileOptions::for_module("test.main").unwrap())
        .await?;
    let typ = program.result_type().clone();
    let value = evaluator.run(program, Default::default()).await?;
    Ok((value, typ))
}

async fn eval_i32(source: &str, builder: Builder<()>) -> i32 {
    let (value, _typ) = eval_value(source, builder).await.unwrap();
    value.as_i32().unwrap()
}

async fn wait_for_count(count: &AtomicUsize, expected: usize) -> bool {
    for _ in 0..1024 {
        if count.load(Ordering::SeqCst) >= expected {
            return true;
        }
        tokio::task::yield_now().await;
    }
    false
}

struct GateControl {
    started: Arc<AtomicUsize>,
    started_rx: mpsc::Receiver<i32>,
    releases: Vec<oneshot::Sender<()>>,
}

struct GateParts {
    started: Arc<AtomicUsize>,
    started_tx: mpsc::Sender<i32>,
    releases: Arc<Mutex<VecDeque<oneshot::Receiver<()>>>>,
    control: GateControl,
}

struct DropCounter(Arc<AtomicUsize>);

impl Drop for DropCounter {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct DynamicPermitController {
    ready_work_limit: usize,
    capacity: AtomicUsize,
    active: Arc<AtomicUsize>,
    waker: Arc<Mutex<Option<Waker>>>,
}

#[derive(Clone)]
struct CountingCallExecutor {
    spawned: Arc<AtomicUsize>,
}

#[derive(Clone, Copy)]
struct TokioCallExecutor;

impl DynamicPermitController {
    fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            ready_work_limit: 64,
            capacity: AtomicUsize::new(capacity),
            active: Arc::new(AtomicUsize::new(0)),
            waker: Arc::new(Mutex::new(None)),
        })
    }

    fn set_capacity(&self, capacity: usize) {
        self.capacity.store(capacity, Ordering::SeqCst);
        if let Some(waker) = self.waker.lock().unwrap().take() {
            waker.wake();
        }
    }
}

impl ParallelismController for DynamicPermitController {
    fn ready_work_limit(&self) -> usize {
        self.ready_work_limit
    }

    fn poll_acquire_native_async(
        &self,
        cx: &mut TaskContext<'_>,
    ) -> Poll<Result<NativeAsyncPermit, EngineError>> {
        loop {
            let active = self.active.load(Ordering::SeqCst);
            if active >= self.capacity.load(Ordering::SeqCst) {
                *self.waker.lock().unwrap() = Some(cx.waker().clone());
                return Poll::Pending;
            }
            if self
                .active
                .compare_exchange(active, active + 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                let active = Arc::clone(&self.active);
                let waker = Arc::clone(&self.waker);
                return Poll::Ready(Ok(NativeAsyncPermit::new(move || {
                    active.fetch_sub(1, Ordering::SeqCst);
                    if let Some(waker) = waker.lock().unwrap().take() {
                        waker.wake();
                    }
                })));
            }
        }
    }
}

impl AsyncCallExecutor for CountingCallExecutor {
    fn spawn(&self, future: NativeFuture) -> NativeFuture {
        self.spawned.fetch_add(1, Ordering::SeqCst);
        future
    }
}

impl AsyncCallExecutor for TokioCallExecutor {
    fn spawn(&self, future: NativeFuture) -> NativeFuture {
        async move {
            tokio::spawn(future).await.map_err(|error| {
                EngineError::Internal(format!("Tokio host task failed: {error}"))
            })?
        }
        .boxed()
    }
}

fn gate_parts(child_count: usize) -> GateParts {
    let started = Arc::new(AtomicUsize::new(0));
    let (started_tx, started_rx) = mpsc::channel();
    let mut release_txs = Vec::new();
    let mut release_rxs = VecDeque::new();
    for _ in 0..child_count {
        let (tx, rx) = oneshot::channel();
        release_txs.push(tx);
        release_rxs.push_back(rx);
    }

    GateParts {
        started: Arc::clone(&started),
        started_tx,
        releases: Arc::new(Mutex::new(release_rxs)),
        control: GateControl {
            started,
            started_rx,
            releases: release_txs,
        },
    }
}

fn builder_with_gate(child_count: usize) -> (Builder<()>, GateControl) {
    let parts = gate_parts(child_count);
    let mut builder = Builder::with_prelude(()).unwrap();
    let mut module = Module::global();
    module
        .export_async("gate", {
            let started = Arc::clone(&parts.started);
            let started_tx = parts.started_tx.clone();
            let releases = Arc::clone(&parts.releases);
            move |_: &(), value: i32| {
                let started = Arc::clone(&started);
                let started_tx = started_tx.clone();
                let release = releases
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("missing async gate release channel");
                async move {
                    started_tx.send(value).unwrap();
                    started.fetch_add(1, Ordering::SeqCst);
                    release.await.unwrap();
                    Ok(value)
                }
            }
        })
        .unwrap();
    builder.inject_module(module).unwrap();

    (builder, parts.control)
}

fn builder_with_even_gate(child_count: usize) -> (Builder<()>, GateControl) {
    let parts = gate_parts(child_count);
    let mut builder = Builder::with_prelude(()).unwrap();
    let mut module = Module::global();
    module
        .export_async("gate_even", {
            let started = Arc::clone(&parts.started);
            let started_tx = parts.started_tx.clone();
            let releases = Arc::clone(&parts.releases);
            move |_: &(), value: i32| {
                let started = Arc::clone(&started);
                let started_tx = started_tx.clone();
                let release = releases
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("missing async gate release channel");
                async move {
                    started_tx.send(value).unwrap();
                    started.fetch_add(1, Ordering::SeqCst);
                    release.await.unwrap();
                    Ok(value % 2 == 0)
                }
            }
        })
        .unwrap();
    builder.inject_module(module).unwrap();

    (builder, parts.control)
}

fn builder_with_gate_and_even_gate(
    gate_count: usize,
    even_gate_count: usize,
) -> (Builder<()>, GateControl, GateControl) {
    let value_parts = gate_parts(gate_count);
    let even_parts = gate_parts(even_gate_count);
    let mut builder = Builder::with_prelude(()).unwrap();
    let mut module = Module::global();
    module
        .export_async("gate", {
            let started = Arc::clone(&value_parts.started);
            let started_tx = value_parts.started_tx.clone();
            let releases = Arc::clone(&value_parts.releases);
            move |_: &(), value: i32| {
                let started = Arc::clone(&started);
                let started_tx = started_tx.clone();
                let release = releases
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("missing async gate release channel");
                async move {
                    started_tx.send(value).unwrap();
                    started.fetch_add(1, Ordering::SeqCst);
                    release.await.unwrap();
                    Ok(value)
                }
            }
        })
        .unwrap();
    module
        .export_async("gate_even", {
            let started = Arc::clone(&even_parts.started);
            let started_tx = even_parts.started_tx.clone();
            let releases = Arc::clone(&even_parts.releases);
            move |_: &(), value: i32| {
                let started = Arc::clone(&started);
                let started_tx = started_tx.clone();
                let release = releases
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("missing async gate release channel");
                async move {
                    started_tx.send(value).unwrap();
                    started.fetch_add(1, Ordering::SeqCst);
                    release.await.unwrap();
                    Ok(value % 2 == 0)
                }
            }
        })
        .unwrap();
    builder.inject_module(module).unwrap();

    (builder, value_parts.control, even_parts.control)
}

async fn eval_with_gates(source: &str, gate_count: usize) -> (Value, Type, Vec<i32>) {
    let (builder, gate) = builder_with_gate(gate_count);
    eval_with_gate_control(source, builder, gate, gate_count).await
}

async fn eval_with_even_gates(source: &str, gate_count: usize) -> (Value, Type, Vec<i32>) {
    let (builder, gate) = builder_with_even_gate(gate_count);
    eval_with_gate_control(source, builder, gate, gate_count).await
}

async fn eval_with_gate_control(
    source: &str,
    builder: Builder<()>,
    gate: GateControl,
    gate_count: usize,
) -> (Value, Type, Vec<i32>) {
    let source = source.to_string();
    let eval_task = tokio::spawn(async move { eval_value(&source, builder).await });
    assert!(
        wait_for_count(&gate.started, gate_count).await,
        "evaluation did not start all gated async children"
    );

    let mut started_values = Vec::with_capacity(gate_count);
    for _ in 0..gate_count {
        started_values.push(gate.started_rx.recv().unwrap());
    }
    started_values.sort();

    for release in gate.releases {
        release.send(()).unwrap();
    }
    for _ in 0..1024 {
        if eval_task.is_finished() {
            break;
        }
        tokio::task::yield_now().await;
    }
    if !eval_task.is_finished() {
        eval_task.abort();
        panic!("timed out waiting for gated evaluation");
    }

    let (value, ty) = eval_task
        .await
        .expect("gated evaluation task panicked")
        .expect("gated evaluation failed");
    (value, ty, started_values)
}

async fn eval_gated_i32(source: &str, gate_count: usize) -> (i32, Vec<i32>) {
    let (value, _ty, started_values) = eval_with_gates(source, gate_count).await;
    (value.as_i32().unwrap(), started_values)
}

fn handle_as_i32_list(value: &Value) -> Vec<i32> {
    value
        .as_list()
        .unwrap()
        .iter()
        .map(|item| item.as_i32().unwrap())
        .collect()
}

fn extreme_stress_builder() -> Builder<()> {
    let mut builder = Builder::with_prelude(()).unwrap();
    builder.set_extreme_gc_stress(true);
    builder
}

#[tokio::test]
async fn evaluator_handles_literals_sequences_and_records() {
    let result = eval_i32(
        r#"
        type Foo = Bar { x: i32, y: i32, z: i32 };
        type Sum = A { x: i32 } | B { x: i32 };

        let
            foo: Foo = Bar { x = 1, y = 2, z = 3 },
            tuple = (1, 2, 3),
            list = [1, 2, 3],
            foo2 = { foo with { x = 6 } },
            sum: Sum = A { x = 1 },
            sum2 = match sum with {
                case A {x} -> { sum with { x = x + 1 } };
                case B {x} -> { sum with { x = x + 2 } };
            }
        in
            foo2.x + (match sum2 with { case A {x} -> x; case B {x} -> x; })
        "#,
        Builder::with_prelude(()).unwrap(),
    )
    .await;
    assert_eq!(result, 8);
}

#[tokio::test]
async fn tuple_evaluation_starts_all_async_children() {
    let (value, ty, started_values) = eval_with_gates("(gate 1, gate 2)", 2).await;
    assert_eq!(started_values, vec![1, 2]);

    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32),
        ])
    );
    let values = value.as_tuple().unwrap();
    assert_eq!(values[0].as_i32().unwrap(), 1);
    assert_eq!(values[1].as_i32().unwrap(), 2);
}

#[tokio::test]
async fn list_evaluation_starts_all_async_children() {
    let (result, started_values) = eval_gated_i32("sum [gate 1, gate 2]", 2).await;
    assert_eq!(started_values, vec![1, 2]);
    assert_eq!(result, 3);
}

#[tokio::test]
async fn sequence_map_starts_all_async_callbacks() {
    let (result, started_values) = eval_gated_i32("sum (map gate [1, 2])", 2).await;
    assert_eq!(started_values, vec![1, 2]);
    assert_eq!(result, 3);
}

#[tokio::test]
async fn sequence_filter_starts_all_async_callbacks() {
    let (value, ty, started_values) =
        eval_with_even_gates("filter gate_even [1, 2, 3, 4]", 4).await;
    assert_eq!(started_values, vec![1, 2, 3, 4]);
    assert_eq!(ty, Type::list(Type::builtin(BuiltinTypeId::I32)));
    assert_eq!(handle_as_i32_list(&value), vec![2, 4]);
}

#[tokio::test]
async fn sequence_filter_map_starts_all_async_callbacks() {
    let (value, ty, started_values) = eval_with_even_gates(
        r#"
        filter_map
            (\x -> if gate_even x then Some (x * 10) else None)
            [1, 2, 3, 4]
        "#,
        4,
    )
    .await;
    assert_eq!(started_values, vec![1, 2, 3, 4]);
    assert_eq!(ty, Type::list(Type::builtin(BuiltinTypeId::I32)));
    assert_eq!(handle_as_i32_list(&value), vec![20, 40]);
}

#[tokio::test]
async fn sequence_flat_map_starts_all_async_callbacks() {
    let (value, ty, started_values) = eval_with_gates(
        r#"
        bind
            (\x -> let y = gate x in [y, y + 100])
            [1, 2, 3]
        "#,
        3,
    )
    .await;
    assert_eq!(started_values, vec![1, 2, 3]);
    assert_eq!(ty, Type::list(Type::builtin(BuiltinTypeId::I32)));
    assert_eq!(handle_as_i32_list(&value), vec![1, 101, 2, 102, 3, 103]);
}

#[tokio::test]
async fn dict_value_map_starts_all_async_callbacks() {
    let (result, started_values) = eval_gated_i32(
        r#"
        let
            mapped = map gate (({ a = 1, b = 2, c = 3 }) is Dict i32)
        in
            match mapped with {
                case {a, b, c} -> a + b + c;
            }
        "#,
        3,
    )
    .await;
    assert_eq!(started_values, vec![1, 2, 3]);
    assert_eq!(result, 6);
}

#[tokio::test]
async fn sequence_map_preserves_order_when_callbacks_complete_out_of_order() {
    let (builder, mut gate) = builder_with_gate(3);
    let eval_task = tokio::spawn(async move { eval_value("map gate [1, 2, 3]", builder).await });

    assert!(
        wait_for_count(&gate.started, 3).await,
        "evaluation did not start all map callbacks"
    );
    let mut started_values = Vec::new();
    for _ in 0..3 {
        started_values.push(gate.started_rx.recv().unwrap());
    }
    started_values.sort();
    assert_eq!(started_values, vec![1, 2, 3]);

    gate.releases.remove(2).send(()).unwrap();
    gate.releases.remove(0).send(()).unwrap();
    gate.releases.remove(0).send(()).unwrap();

    let (value, ty) = eval_task
        .await
        .expect("gated map evaluation task panicked")
        .expect("gated map evaluation failed");
    assert_eq!(ty, Type::list(Type::builtin(BuiltinTypeId::I32)));
    assert_eq!(handle_as_i32_list(&value), vec![1, 2, 3]);
}

#[tokio::test]
async fn sequence_filter_preserves_order_when_callbacks_complete_out_of_order() {
    let (builder, mut gate) = builder_with_even_gate(4);
    let eval_task =
        tokio::spawn(async move { eval_value("filter gate_even [1, 2, 3, 4]", builder).await });

    assert!(
        wait_for_count(&gate.started, 4).await,
        "evaluation did not start all filter callbacks"
    );
    let mut started_values = Vec::new();
    for _ in 0..4 {
        started_values.push(gate.started_rx.recv().unwrap());
    }
    started_values.sort();
    assert_eq!(started_values, vec![1, 2, 3, 4]);

    gate.releases.remove(3).send(()).unwrap();
    gate.releases.remove(1).send(()).unwrap();
    gate.releases.remove(1).send(()).unwrap();
    gate.releases.remove(0).send(()).unwrap();

    let (value, ty) = eval_task
        .await
        .expect("gated filter evaluation task panicked")
        .expect("gated filter evaluation failed");
    assert_eq!(ty, Type::list(Type::builtin(BuiltinTypeId::I32)));
    assert_eq!(handle_as_i32_list(&value), vec![2, 4]);
}

#[tokio::test]
async fn sequence_filter_map_preserves_order_when_callbacks_complete_out_of_order() {
    let (builder, mut gate) = builder_with_even_gate(4);
    let eval_task = tokio::spawn(async move {
        eval_value(
            r#"
            filter_map
                (\x -> if gate_even x then Some (x * 10) else None)
                [1, 2, 3, 4]
            "#,
            builder,
        )
        .await
    });

    assert!(
        wait_for_count(&gate.started, 4).await,
        "evaluation did not start all filter_map callbacks"
    );
    let mut started_values = Vec::new();
    for _ in 0..4 {
        started_values.push(gate.started_rx.recv().unwrap());
    }
    started_values.sort();
    assert_eq!(started_values, vec![1, 2, 3, 4]);

    gate.releases.remove(3).send(()).unwrap();
    gate.releases.remove(1).send(()).unwrap();
    gate.releases.remove(1).send(()).unwrap();
    gate.releases.remove(0).send(()).unwrap();

    let (value, ty) = eval_task
        .await
        .expect("gated filter_map evaluation task panicked")
        .expect("gated filter_map evaluation failed");
    assert_eq!(ty, Type::list(Type::builtin(BuiltinTypeId::I32)));
    assert_eq!(handle_as_i32_list(&value), vec![20, 40]);
}

#[tokio::test]
async fn sequence_flat_map_preserves_order_when_callbacks_complete_out_of_order() {
    let (builder, mut gate) = builder_with_gate(3);
    let eval_task = tokio::spawn(async move {
        eval_value(
            r#"
            bind
                (\x -> let y = gate x in [y, y + 100])
                [1, 2, 3]
            "#,
            builder,
        )
        .await
    });

    assert!(
        wait_for_count(&gate.started, 3).await,
        "evaluation did not start all flat_map callbacks"
    );
    let mut started_values = Vec::new();
    for _ in 0..3 {
        started_values.push(gate.started_rx.recv().unwrap());
    }
    started_values.sort();
    assert_eq!(started_values, vec![1, 2, 3]);

    gate.releases.remove(2).send(()).unwrap();
    gate.releases.remove(0).send(()).unwrap();
    gate.releases.remove(0).send(()).unwrap();

    let (value, ty) = eval_task
        .await
        .expect("gated flat_map evaluation task panicked")
        .expect("gated flat_map evaluation failed");
    assert_eq!(ty, Type::list(Type::builtin(BuiltinTypeId::I32)));
    assert_eq!(handle_as_i32_list(&value), vec![1, 101, 2, 102, 3, 103]);
}

#[tokio::test]
async fn dict_value_map_preserves_keys_when_callbacks_complete_out_of_order() {
    let (builder, mut gate) = builder_with_gate(3);
    let eval_task = tokio::spawn(async move {
        eval_value(
            r#"
            let
                mapped = map gate (({ a = 1, b = 2, c = 3 }) is Dict i32)
            in
                match mapped with {
                    case {a, b, c} -> a * 100 + b * 10 + c;
                }
            "#,
            builder,
        )
        .await
    });

    assert!(
        wait_for_count(&gate.started, 3).await,
        "evaluation did not start all dict_map callbacks"
    );
    let mut started_values = Vec::new();
    for _ in 0..3 {
        started_values.push(gate.started_rx.recv().unwrap());
    }
    started_values.sort();
    assert_eq!(started_values, vec![1, 2, 3]);

    gate.releases.remove(2).send(()).unwrap();
    gate.releases.remove(0).send(()).unwrap();
    gate.releases.remove(0).send(()).unwrap();

    let (value, ty) = eval_task
        .await
        .expect("gated dict_map evaluation task panicked")
        .expect("gated dict_map evaluation failed");
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_eq!(value.as_i32().unwrap(), 123);
}

#[tokio::test]
async fn dict_entry_map_applies_collision_results_in_input_key_order() {
    let (builder, mut gate) = builder_with_gate(3);
    let eval_task = tokio::spawn(async move {
        eval_value(
            r#"
            let
                mapped =
                    dict_map
                        (\entry ->
                            match entry with {
                                case (key, value) -> ("same", gate value);
                            })
                        (({ c = 3, a = 1, b = 2 }) is Dict i32)
            in
                match mapped with {
                    case {same} -> same;
                }
            "#,
            builder,
        )
        .await
    });

    assert!(
        wait_for_count(&gate.started, 3).await,
        "evaluation did not start all dict_map callbacks"
    );
    let mut started_values = Vec::new();
    for _ in 0..3 {
        started_values.push(gate.started_rx.recv().unwrap());
    }
    started_values.sort();
    assert_eq!(started_values, vec![1, 2, 3]);

    // Completion order is c, a, b. Collision resolution must nevertheless
    // follow original key order a, b, c, making c's value the final winner.
    gate.releases.remove(2).send(()).unwrap();
    gate.releases.remove(0).send(()).unwrap();
    gate.releases.remove(0).send(()).unwrap();

    let (value, ty) = eval_task
        .await
        .expect("gated dict_map evaluation task panicked")
        .expect("gated dict_map evaluation failed");
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_eq!(value.as_i32().unwrap(), 3);
}

#[tokio::test]
async fn dict_entry_filter_starts_all_callbacks_and_preserves_entries() {
    let (value, ty, started_values) = eval_with_even_gates(
        r#"
        let
            filtered =
                dict_filter
                    (\entry ->
                        match entry with {
                            case (key, value) -> gate_even value;
                        })
                    (({ a = 1, b = 2, c = 3, d = 4 }) is Dict i32)
        in
            match filtered with {
                case {b, d} ->
                    if length filtered == (2 is u64) then 200 + b * 10 + d else 0;
            }
        "#,
        4,
    )
    .await;
    assert_eq!(started_values, vec![1, 2, 3, 4]);
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_eq!(value.as_i32().unwrap(), 224);
}

#[tokio::test]
async fn dict_value_filter_starts_all_async_callbacks() {
    let (value, ty, started_values) = eval_with_even_gates(
        r#"
        let
            filtered = filter gate_even
                (({ a = 1, b = 2, c = 3, d = 4 }) is Dict i32)
        in
            match filtered with {
                case {b, d} ->
                    if length filtered == (2 is u64) then 200 + b * 10 + d else 0;
            }
        "#,
        4,
    )
    .await;
    assert_eq!(started_values, vec![1, 2, 3, 4]);
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_eq!(value.as_i32().unwrap(), 224);
}

#[tokio::test]
async fn sibling_map_and_filter_fan_out_their_callbacks() {
    let (builder, gate, even_gate) = builder_with_gate_and_even_gate(2, 2);
    let eval_task = tokio::spawn(async move {
        eval_value(
            r#"
            let
                xs = [1, 2]
            in
                (sum (map gate xs), length (filter gate_even xs))
            "#,
            builder,
        )
        .await
    });

    assert!(
        wait_for_count(&gate.started, 2).await,
        "evaluation did not start all map callbacks"
    );
    assert!(
        wait_for_count(&even_gate.started, 2).await,
        "evaluation did not start all filter callbacks"
    );

    for release in gate.releases {
        release.send(()).unwrap();
    }
    for release in even_gate.releases {
        release.send(()).unwrap();
    }

    let (value, ty) = eval_task
        .await
        .expect("gated map/filter evaluation task panicked")
        .expect("gated map/filter evaluation failed");
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::U64),
        ])
    );
    let values = value.as_tuple().unwrap();
    assert_eq!(values[0].as_i32().unwrap(), 3);
    assert_eq!(values[1].as_u64().unwrap(), 1);
}

#[tokio::test]
async fn dict_evaluation_starts_all_async_children() {
    let (result, started_values) = eval_gated_i32(
        r#"
        match { a = gate 1, b = gate 2 } with {
            case {a, b} -> a + b;
        }
        "#,
        2,
    )
    .await;
    assert_eq!(started_values, vec![1, 2]);
    assert_eq!(result, 3);
}

#[tokio::test]
async fn record_update_starts_all_async_update_children() {
    let (result, started_values) = eval_gated_i32(
        r#"
        type Box = Box { a: i32, b: i32 };

        let
            base: Box = Box { a = 10, b = 20 },
            updated: Box = { base with { a = gate 1, b = gate 2 } }
        in
            updated.a + updated.b
        "#,
        2,
    )
    .await;
    assert_eq!(started_values, vec![1, 2]);
    assert_eq!(result, 3);
}

#[tokio::test]
async fn application_evaluation_starts_all_async_arguments() {
    let (result, started_values) = eval_gated_i32(
        r#"
        let combine = \a -> \b -> a + b
        in combine (gate 1) (gate 2)
        "#,
        2,
    )
    .await;
    assert_eq!(started_values, vec![1, 2]);
    assert_eq!(result, 3);
}

#[tokio::test]
async fn async_call_policy_wraps_host_calls() {
    let spawned = Arc::new(AtomicUsize::new(0));
    let mut builder = Builder::with_prelude(()).unwrap();
    builder.set_async_call_policy(AsyncCallPolicy::executor(CountingCallExecutor {
        spawned: Arc::clone(&spawned),
    }));

    let mut module = Module::global();
    module
        .export_async("bump", |_: &(), value: i32| async move { Ok(value + 1) })
        .unwrap();
    builder.inject_module(module).unwrap();

    let result = eval_i32("bump 1 + bump 2", builder).await;
    assert_eq!(result, 5);
    assert_eq!(spawned.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn async_call_policy_accepts_executor_closures() {
    let spawned = Arc::new(AtomicUsize::new(0));
    let mut builder = Builder::with_prelude(()).unwrap();
    builder.set_async_call_policy(AsyncCallPolicy::executor_fn({
        let spawned = Arc::clone(&spawned);
        move |future| {
            spawned.fetch_add(1, Ordering::SeqCst);
            future
        }
    }));
    assert!(builder.async_call_policy().is_executor());

    let mut module = Module::global();
    module
        .export_async("bump", |_: &(), value: i32| async move { Ok(value + 1) })
        .unwrap();
    builder.inject_module(module).unwrap();

    let result = eval_i32("bump 10 + bump 20", builder).await;
    assert_eq!(result, 32);
    assert_eq!(spawned.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn sync_calls_bypass_async_admission_and_executor() {
    let invoked = Arc::new(AtomicUsize::new(0));
    let spawned = Arc::new(AtomicUsize::new(0));
    let mut builder = Builder::with_prelude(()).unwrap();
    builder.set_parallelism_controller(DynamicPermitController::new(0));
    builder.set_async_call_policy(AsyncCallPolicy::executor(CountingCallExecutor {
        spawned: Arc::clone(&spawned),
    }));

    let mut module = Module::global();
    module
        .export("bump", {
            let invoked = Arc::clone(&invoked);
            move |_: &(), value: i32| {
                invoked.fetch_add(1, Ordering::SeqCst);
                Ok(value + 1)
            }
        })
        .unwrap();
    builder.inject_module(module).unwrap();

    let value = tokio::time::timeout(Duration::from_secs(10), eval_i32("bump 41", builder))
        .await
        .expect("sync evaluation waited for an async native permit");
    assert_eq!(invoked.load(Ordering::SeqCst), 1);
    assert_eq!(value, 42);
    assert_eq!(spawned.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn small_ready_work_bound_still_completes_fanout() {
    let mut builder = Builder::with_prelude(()).unwrap();
    builder.set_execution_bounds(ExecutionBounds::new(1, 64));

    let result = eval_i32(
        r#"
        let
            xs: List i32 = [
                1, 2, 3, 4, 5, 6, 7, 8,
                9, 10, 11, 12, 13, 14, 15, 16
            ],
            doubled = map (\x -> x * 2) xs
        in
            sum doubled
        "#,
        builder,
    )
    .await;
    assert_eq!(result, 272);
}

#[tokio::test]
async fn pending_async_bound_delays_admitting_host_calls() {
    let (mut builder, mut gate) = builder_with_gate(2);
    builder.set_execution_bounds(ExecutionBounds::new(64, 1));

    let eval_task = tokio::spawn(async move { eval_i32("sum [gate 1, gate 2]", builder).await });

    assert!(
        wait_for_count(&gate.started, 1).await,
        "evaluation did not start the first gated async call"
    );
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        gate.started.load(Ordering::SeqCst),
        1,
        "second async call started before the pending async bound opened"
    );

    let first = gate.started_rx.recv().unwrap();
    gate.releases.remove(0).send(()).unwrap();

    assert!(
        wait_for_count(&gate.started, 2).await,
        "evaluation did not start the second gated async call"
    );
    let second = gate.started_rx.recv().unwrap();
    gate.releases.remove(0).send(()).unwrap();

    let result = eval_task.await.expect("gated evaluation task panicked");
    let mut started = vec![first, second];
    started.sort();
    assert_eq!(started, vec![1, 2]);
    assert_eq!(result, 3);
}

#[tokio::test]
async fn dynamic_native_async_permits_can_increase_during_evaluation() {
    let (mut builder, gate) = builder_with_gate(3);
    let controller = DynamicPermitController::new(1);
    builder.set_parallelism_controller(controller.clone());

    let eval_task =
        tokio::spawn(async move { eval_i32("sum [gate 1, gate 2, gate 3]", builder).await });

    assert!(
        wait_for_count(&gate.started, 1).await,
        "evaluation did not start the first admitted async call"
    );
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        gate.started.load(Ordering::SeqCst),
        1,
        "dynamic controller admitted more async calls before capacity increased"
    );

    controller.set_capacity(3);
    assert!(
        wait_for_count(&gate.started, 3).await,
        "evaluation did not admit deferred async calls after capacity increased"
    );

    let mut started = Vec::new();
    for _ in 0..3 {
        started.push(gate.started_rx.recv().unwrap());
    }
    started.sort();
    assert_eq!(started, vec![1, 2, 3]);

    for release in gate.releases {
        release.send(()).unwrap();
    }
    let result = eval_task.await.expect("gated evaluation task panicked");
    assert_eq!(result, 6);
}

#[tokio::test]
async fn dynamic_native_async_permits_delay_host_callback_invocation() {
    let invoked = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(AtomicUsize::new(0));
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let release = Arc::new(Mutex::new(Some(release_rx)));

    let mut builder = Builder::with_prelude(()).unwrap();
    let controller = DynamicPermitController::new(0);
    builder.set_parallelism_controller(controller.clone());
    let mut module = Module::global();
    module
        .export_async("gate_call", {
            let invoked = Arc::clone(&invoked);
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            move |_: &(), value: i32| {
                invoked.fetch_add(1, Ordering::SeqCst);
                let started = Arc::clone(&started);
                let started_tx = started_tx.clone();
                let release = release
                    .lock()
                    .unwrap()
                    .take()
                    .expect("missing async gate release channel");
                async move {
                    started_tx.send(value).unwrap();
                    started.fetch_add(1, Ordering::SeqCst);
                    release.await.unwrap();
                    Ok(value)
                }
            }
        })
        .unwrap();
    builder.inject_module(module).unwrap();

    let eval_task = tokio::spawn(async move { eval_i32("gate_call 1", builder).await });

    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        invoked.load(Ordering::SeqCst),
        0,
        "host callback was invoked before a native async permit was available"
    );

    controller.set_capacity(1);
    assert!(
        wait_for_count(&invoked, 1).await,
        "evaluation did not invoke host callback after a permit became available"
    );
    assert!(
        wait_for_count(&started, 1).await,
        "admitted host future did not start"
    );
    assert_eq!(started_rx.recv().unwrap(), 1);
    release_tx.send(()).unwrap();
    let result = eval_task.await.expect("gated evaluation task panicked");
    assert_eq!(result, 1);
}

#[tokio::test]
async fn cancelling_evaluation_drops_pending_owned_host_future() {
    let started = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let (_release_tx, release_rx) = oneshot::channel::<()>();
    let release = Arc::new(Mutex::new(Some(release_rx)));

    let mut builder = Builder::with_prelude(()).unwrap();
    let controller = DynamicPermitController::new(1);
    builder.set_parallelism_controller(controller.clone());
    let mut module = Module::global();
    module
        .export_async("wait_for_release", {
            let started = Arc::clone(&started);
            let dropped = Arc::clone(&dropped);
            let release = Arc::clone(&release);
            move |_: &(), value: i32| {
                let started = Arc::clone(&started);
                let guard = DropCounter(Arc::clone(&dropped));
                let release = release
                    .lock()
                    .unwrap()
                    .take()
                    .expect("missing cancellation gate");
                async move {
                    let _guard = guard;
                    started.fetch_add(1, Ordering::SeqCst);
                    release.await.unwrap();
                    Ok(value)
                }
            }
        })
        .unwrap();
    builder.inject_module(module).unwrap();

    let eval_task = tokio::spawn(async move { eval_i32("wait_for_release 1", builder).await });
    assert!(
        wait_for_count(&started, 1).await,
        "pending host future was not polled"
    );

    eval_task.abort();
    assert!(eval_task.await.unwrap_err().is_cancelled());
    assert!(
        wait_for_count(&dropped, 1).await,
        "cancelling evaluation did not drop the host future"
    );
    assert_eq!(
        controller.active.load(Ordering::SeqCst),
        0,
        "cancelling evaluation did not release its async permit"
    );
}

#[tokio::test]
async fn pending_async_bound_delays_invoking_host_callbacks() {
    let invoked = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(AtomicUsize::new(0));
    let (started_tx, started_rx) = mpsc::channel();
    let mut release_txs = Vec::new();
    let mut release_rxs = VecDeque::new();
    for _ in 0..2 {
        let (tx, rx) = oneshot::channel();
        release_txs.push(tx);
        release_rxs.push_back(rx);
    }
    let releases = Arc::new(Mutex::new(release_rxs));

    let mut builder = Builder::with_prelude(()).unwrap();
    builder.set_execution_bounds(ExecutionBounds::new(64, 1));
    let mut module = Module::global();
    module
        .export_async("gate_call", {
            let invoked = Arc::clone(&invoked);
            let started = Arc::clone(&started);
            let started_tx = started_tx.clone();
            let releases = Arc::clone(&releases);
            move |_: &(), value: i32| {
                invoked.fetch_add(1, Ordering::SeqCst);
                let started = Arc::clone(&started);
                let started_tx = started_tx.clone();
                let release = releases
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("missing async gate release channel");
                async move {
                    started_tx.send(value).unwrap();
                    started.fetch_add(1, Ordering::SeqCst);
                    release.await.unwrap();
                    Ok(value)
                }
            }
        })
        .unwrap();
    builder.inject_module(module).unwrap();

    let eval_task =
        tokio::spawn(async move { eval_i32("sum [gate_call 1, gate_call 2]", builder).await });

    assert!(
        wait_for_count(&invoked, 1).await,
        "evaluation did not invoke the first gated async call"
    );
    assert!(
        wait_for_count(&started, 1).await,
        "evaluation did not start the first gated async call"
    );
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        invoked.load(Ordering::SeqCst),
        1,
        "second host callback was invoked before the pending async bound opened"
    );
    assert_eq!(
        started.load(Ordering::SeqCst),
        1,
        "second async future started before the pending async bound opened"
    );

    let first = started_rx.recv().unwrap();
    release_txs.remove(0).send(()).unwrap();

    assert!(
        wait_for_count(&invoked, 2).await,
        "evaluation did not invoke the second gated async call"
    );
    assert!(
        wait_for_count(&started, 2).await,
        "evaluation did not start the second gated async call"
    );
    let second = started_rx.recv().unwrap();
    release_txs.remove(0).send(()).unwrap();

    let result = eval_task.await.expect("gated evaluation task panicked");
    let mut started_values = vec![first, second];
    started_values.sort();
    assert_eq!(started_values, vec![1, 2]);
    assert_eq!(result, 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_host_values_survive_repeated_collections() {
    const HOST_CALLS: usize = 4;
    const STRESS_ROUNDS: usize = 8;

    for round in 0..STRESS_ROUNDS {
        let GateParts {
            started,
            started_tx,
            releases,
            control,
        } = gate_parts(HOST_CALLS);
        let invoked = Arc::new(AtomicUsize::new(0));
        let mut builder = Builder::with_prelude(()).unwrap();
        builder.set_async_call_policy(AsyncCallPolicy::executor(TokioCallExecutor));

        let mut module = Module::global();
        let list_i32 = Type::list(Type::builtin(BuiltinTypeId::I32));
        let scheme = Scheme::new(vec![], vec![], Type::fun(list_i32.clone(), list_i32));
        module
            .export_native_async("stress_copy", scheme, 1, {
                let invoked = Arc::clone(&invoked);
                let started = Arc::clone(&started);
                let releases = Arc::clone(&releases);
                move |_ctx, _, args: Vec<Value>| {
                    let call_id = invoked.fetch_add(1, Ordering::SeqCst);
                    let retained = args.first().cloned();
                    let started = Arc::clone(&started);
                    let started_tx = started_tx.clone();
                    let release = releases
                        .lock()
                        .unwrap()
                        .pop_front()
                        .expect("missing stress release channel");
                    async move {
                        started_tx.send(call_id as i32).unwrap();
                        started.fetch_add(1, Ordering::SeqCst);
                        release.await.unwrap();

                        let retained = retained.ok_or_else(|| {
                            EngineError::Internal("missing stress_copy argument".into())
                        })?;
                        tokio::task::yield_now().await;
                        Ok(retained)
                    }
                    .boxed()
                }
            })
            .unwrap();
        builder.inject_module(module).unwrap();
        builder.set_extreme_gc_stress(true);

        let eval_task = tokio::spawn(async move {
            eval_i32(
                r#"
                sum (map sum [
                    stress_copy [1, 2, 3, 4, 5, 6, 7, 8],
                    stress_copy [1, 2, 3, 4, 5, 6, 7, 8],
                    stress_copy [1, 2, 3, 4, 5, 6, 7, 8],
                    stress_copy [1, 2, 3, 4, 5, 6, 7, 8],
                    map (\x -> x * 2) [
                        1, 2, 3, 4, 5, 6, 7, 8,
                        9, 10, 11, 12, 13, 14, 15, 16
                    ]
                ])
                "#,
                builder,
            )
            .await
        });

        let mut call_ids = (0..HOST_CALLS)
            .map(|_| {
                control
                    .started_rx
                    .recv_timeout(Duration::from_secs(10))
                    .expect("missing stress task start notification")
            })
            .collect::<Vec<_>>();
        call_ids.sort();
        assert_eq!(call_ids, vec![0, 1, 2, 3]);
        assert_eq!(control.started.load(Ordering::SeqCst), HOST_CALLS);
        assert!(
            !eval_task.is_finished(),
            "evaluation completed while host allocation tasks were gated"
        );

        for release in control.releases {
            release.send(()).unwrap();
        }
        let result = tokio::time::timeout(Duration::from_secs(30), eval_task)
            .await
            .unwrap_or_else(|_| panic!("round {round} timed out"))
            .expect("stress evaluation task panicked");
        assert_eq!(result, 416);
        assert_eq!(invoked.load(Ordering::SeqCst), HOST_CALLS);
    }
}

#[tokio::test]
async fn extreme_stress_handles_broad_evaluator_paths() {
    let result = eval_i32(
        r#"
        type Point = Point { x: i32, y: i32 };
        type Choice = Left { item: i32 } | Right { item: i32 };

        class Score a where {
            score : a -> i32;
        }

        instance Score Point where {
            score = \p -> p.x + p.y;
        }

        let rec sum_list = \xs ->
            match xs with {
                case Empty -> 0;
                case Cons h t -> h + sum_list t;
            }
        in
        let
            nums: List i32 = [
                1, 2, 3, 4, 5, 6, 7, 8,
                9, 10, 11, 12, 13, 14, 15, 16,
                17, 18, 19, 20, 21, 22, 23, 24,
                25, 26, 27, 28, 29, 30, 31, 32
            ],
            mapped = map (\x -> x + 1) nums,
            evens = filter (\x -> x % 2 == 0) mapped,
            pairs = zip nums mapped,
            unzipped = unzip pairs,
            lefts = match unzipped with { case (left, right) -> left; },
            arr = mapped,
            arr2 = map (\x -> x * 2) arr,
            flat: List i32 = bind (\x -> [x, x + 1]) [1, 2, 3],
            point: Point = Point { x = 10, y = 5 },
            point2: Point = { point with { y = 7 } },
            choice: Choice = Right { item = score point2 },
            chosen = match choice with {
                case Left {item} -> item;
                case Right {item} -> item + 1;
            },
            dict_val = match { foo = chosen, bar = foldl (\acc x -> acc + x) 0 evens } with {
                case {foo, bar} -> foo + bar;
            },
            mixed = ("gc", 1.5, true),
            tuple_score = ((1 is i32), (2 is i32), (3 is i32)).2
        in
            dict_val
                + sum_list flat
                + get (0 is u64) arr2
                + (if arr == arr then 1 else 0)
                + tuple_score
        "#,
        extreme_stress_builder(),
    )
    .await;
    assert_eq!(result, 313);
}

#[tokio::test]
async fn extreme_stress_handles_host_callbacks_and_conversions() {
    let mut builder = Builder::with_prelude(()).unwrap();
    let mut module = Module::global();
    module
        .export("triple", |_: &(), value: i32| Ok(value * 3))
        .unwrap();
    module
        .export("pack", |_: &(), left: i32, right: i32| {
            Ok(vec![left, right, left + right])
        })
        .unwrap();
    module
        .export_async(
            "bump_async",
            |_: &(), value: i32| async move { Ok(value + 1) },
        )
        .unwrap();
    builder.inject_module(module).unwrap();
    builder.set_extreme_gc_stress(true);

    let result = eval_i32(
        r#"
        let
            xs = pack (bump_async 4) (triple 3),
            ys = map (\x -> x + 1) xs,
            zipped = zip xs ys,
            folded = foldl (\acc pair ->
                match pair with { case (left, right) -> acc + left + right; }
            ) 0 zipped
        in
            folded + sum xs
        "#,
        builder,
    )
    .await;
    assert_eq!(result, 87);
}

#[tokio::test]
async fn extreme_stress_imports_native_owned_nested_data() {
    let mut builder = Builder::with_prelude(()).unwrap();
    let mut module = Module::global();
    let i32_ty = Type::builtin(BuiltinTypeId::I32);
    let row_ty = Type::tuple(vec![i32_ty.clone(), Type::list(i32_ty.clone())]);
    let scheme = Scheme::new(
        vec![],
        vec![],
        Type::fun(i32_ty.clone(), Type::list(row_ty)),
    );
    module
        .export_native("make_nested", scheme, 1, |_ctx, _, args| {
            let count = args
                .first()
                .ok_or_else(|| EngineError::Internal("missing make_nested argument".into()))?
                .as_i32()?;
            if count < 0 {
                return Err(EngineError::Internal(
                    "make_nested count must be non-negative".into(),
                ));
            }

            let mut rows = Vec::new();
            for i in 1..=count {
                let values = (0..4).map(|offset| Value::I32(i + offset)).collect();
                rows.push(Value::Tuple(vec![Value::I32(i), Value::List(values)]));
            }
            Ok(Value::List(rows))
        })
        .unwrap();
    builder.inject_module(module).unwrap();
    builder.set_extreme_gc_stress(true);

    let result = eval_i32(
        r#"
        let
            rows = make_nested 16,
            row_score = \row ->
                match row with { case (base, xs) -> base + sum xs; }
        in
            sum (map row_score rows)
        "#,
        builder,
    )
    .await;
    assert_eq!(result, 776);
}

#[tokio::test]
async fn extreme_stress_handles_captured_closure_envs() {
    let result = eval_i32(
        r#"
        let
            xs: List i32 = [
                1, 2, 3, 4, 5, 6, 7, 8,
                9, 10, 11, 12, 13, 14, 15, 16
            ],
            make_total = \offset ->
                let
                    local = map (\x -> x + offset) xs
                in
                    \extra -> foldl (\acc x -> acc + x) extra local,
            f = make_total 3,
            noise = map (\x -> x * 2) xs
        in
            f 10 + sum noise
        "#,
        extreme_stress_builder(),
    )
    .await;
    assert_eq!(result, 466);
}

#[tokio::test]
async fn extreme_stress_handles_repeated_typeclass_resolution() {
    let result = eval_i32(
        r#"
        type Box = Box { value: i32 };

        class Score a where {
            score : a -> i32;
        }

        instance Score Box where {
            score = \box -> box.value + 1;
        }

        instance Score i32 where {
            score = \x -> x * 2;
        }

        let
            boxes: List Box = [
                Box { value = 1 },
                Box { value = 2 },
                Box { value = 3 }
            ],
            box_scores: List i32 = map score boxes,
            int_scores: List i32 = map score [(10 is i32), (20 is i32)],
            reused_box = score (Box { value = 9 }),
            reused_int = score (5 is i32)
        in
            sum box_scores + sum int_scores + reused_box + reused_int
        "#,
        extreme_stress_builder(),
    )
    .await;
    assert_eq!(result, 89);
}

#[tokio::test]
async fn extreme_stress_handles_async_native_values_across_awaits() {
    let mut builder = Builder::with_prelude(()).unwrap();
    let mut module = Module::global();
    let list_i32 = Type::list(Type::builtin(BuiltinTypeId::I32));
    let i32_ty = Type::builtin(BuiltinTypeId::I32);
    let scheme = Scheme::new(vec![], vec![], Type::fun(list_i32, i32_ty));
    module
        .export_native_async(
            "async_sum_after_alloc",
            scheme,
            1,
            |_ctx, _, args: Vec<Value>| {
                async move {
                    let retained = args.first().cloned().ok_or_else(|| {
                        EngineError::Internal("missing async_sum_after_alloc argument".into())
                    })?;
                    tokio::task::yield_now().await;
                    let mut sum = 0;
                    for value in retained.as_list()? {
                        sum += value.as_i32()?;
                    }
                    Ok(Value::I32(sum))
                }
                .boxed()
            },
        )
        .unwrap();
    builder.inject_module(module).unwrap();
    builder.set_extreme_gc_stress(true);

    let result = eval_i32(
        r#"
        async_sum_after_alloc [
            1, 2, 3, 4, 5, 6, 7, 8
        ]
        "#,
        builder,
    )
    .await;
    assert_eq!(result, 36);
}

#[tokio::test]
async fn evaluator_handles_control_flow_typeclasses_and_recursion() {
    let result = eval_i32(
        r#"
        class Pick a where {
            pick : a -> a;
        }

        instance Pick i32 where {
            pick = \x -> x;
        }

        let rec fact = \n ->
            if n == 0 then 1 else n * fact (n - 1)
        in
            match (Some (pick 4)) with {
                case Some x -> fact x;
                case None -> 0;
            }
        "#,
        Builder::with_prelude(()).unwrap(),
    )
    .await;
    assert_eq!(result, 24);
}

#[tokio::test]
async fn evaluator_handles_prelude_collection_callbacks() {
    let result = eval_i32(
        r#"
        let
            xs = [1, 2, 3],
            ys = map (\x -> x + 1) xs,
            zs = filter (\x -> x == 2) xs,
            total = foldl (\acc x -> acc + x) 0 ys
        in
            total
        "#,
        Builder::with_prelude(()).unwrap(),
    )
    .await;
    assert_eq!(result, 9);
}

#[tokio::test]
async fn evaluator_handles_higher_order_closures() {
    let result = eval_i32(
        r#"
        let
            apply_twice = \f x -> f (f x),
            compose = \f g x -> f (g x),
            a = apply_twice (\n -> n + 1) 1,
            b = compose (\n -> n + 1) (\n -> n * 2) 3
        in
            a + b
        "#,
        Builder::with_prelude(()).unwrap(),
    )
    .await;
    assert_eq!(result, 10);
}

#[tokio::test]
async fn evaluator_handles_partial_and_multi_arg_closures() {
    let result = eval_i32(
        r#"
        let
            add = \x y -> x + y,
            choose = \flag left right -> if flag then left else right,
            inc = add 1,
            picked = choose false (inc 10) (add 20 22)
        in
            (inc 41) + picked
        "#,
        Builder::with_prelude(()).unwrap(),
    )
    .await;
    assert_eq!(result, 84);
}
