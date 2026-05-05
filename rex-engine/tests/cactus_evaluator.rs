use futures::{FutureExt, channel::oneshot};
use rex_engine::{
    AsyncCallExecutor, AsyncCallPolicy, Engine, EngineError, ExecutionBounds, FromRex, Handle,
    Module, NativeFuture,
};
use rex_typesystem::types::{BuiltinTypeId, Scheme, Type};
use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};

async fn eval_value<State>(
    source: &str,
    engine: Engine<State>,
) -> Result<(Handle, Type), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let mut evaluator = engine.into_evaluator();
    evaluator
        .eval_snippet(source)
        .await
        .map_err(|err| err.into_engine_error())
}

async fn eval_i32(source: &str, engine: Engine<()>) -> i32 {
    let (value, _typ) = eval_value(source, engine).await.unwrap();
    i32::from_rex(&value).unwrap()
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

#[derive(Clone)]
struct CountingCallExecutor {
    spawned: Arc<AtomicUsize>,
}

impl AsyncCallExecutor for CountingCallExecutor {
    fn spawn(&self, future: NativeFuture) -> NativeFuture {
        self.spawned.fetch_add(1, Ordering::SeqCst);
        future
    }
}

fn engine_with_gate(child_count: usize) -> (Engine<()>, GateControl) {
    let started = Arc::new(AtomicUsize::new(0));
    let (started_tx, started_rx) = mpsc::channel();
    let mut release_txs = Vec::new();
    let mut release_rxs = VecDeque::new();
    for _ in 0..child_count {
        let (tx, rx) = oneshot::channel();
        release_txs.push(tx);
        release_rxs.push_back(rx);
    }

    let releases = Arc::new(Mutex::new(release_rxs));
    let mut engine = Engine::with_prelude(()).unwrap();
    let mut module = Module::global();
    module
        .export_async("gate", {
            let started = Arc::clone(&started);
            let started_tx = started_tx.clone();
            let releases = Arc::clone(&releases);
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
    engine.inject_module(module).unwrap();

    (
        engine,
        GateControl {
            started,
            started_rx,
            releases: release_txs,
        },
    )
}

async fn eval_with_gates(source: &str, gate_count: usize) -> (Handle, Type, Vec<i32>) {
    let (engine, gate) = engine_with_gate(gate_count);
    let source = source.to_string();
    let eval_task = tokio::spawn(async move { eval_value(&source, engine).await });
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
    (i32::from_rex(&value).unwrap(), started_values)
}

fn engine_collecting_on_every_alloc() -> Engine<()> {
    let engine = Engine::with_prelude(()).unwrap();
    engine.heap.set_collect_on_every_alloc(true).unwrap();
    engine
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
                when A {x} -> { sum with { x = x + 1 } };
                when B {x} -> { sum with { x = x + 2 } };
            }
        in
            foo2.x + (match sum2 with { when A {x} -> x; when B {x} -> x; })
        "#,
        Engine::with_prelude(()).unwrap(),
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
    assert_eq!(i32::from_rex(&values[0]).unwrap(), 1);
    assert_eq!(i32::from_rex(&values[1]).unwrap(), 2);
}

#[tokio::test]
async fn list_evaluation_starts_all_async_children() {
    let (result, started_values) = eval_gated_i32("sum [gate 1, gate 2]", 2).await;
    assert_eq!(started_values, vec![1, 2]);
    assert_eq!(result, 3);
}

#[tokio::test]
async fn dict_evaluation_starts_all_async_children() {
    let (result, started_values) = eval_gated_i32(
        r#"
        match { a = gate 1, b = gate 2 } with {
            when {a, b} -> a + b;
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
    let mut engine = Engine::with_prelude(()).unwrap();
    engine.set_async_call_policy(AsyncCallPolicy::executor(CountingCallExecutor {
        spawned: Arc::clone(&spawned),
    }));

    let mut module = Module::global();
    module
        .export_async("bump", |_: &(), value: i32| async move { Ok(value + 1) })
        .unwrap();
    engine.inject_module(module).unwrap();

    let result = eval_i32("bump 1 + bump 2", engine).await;
    assert_eq!(result, 5);
    assert_eq!(spawned.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn async_call_policy_accepts_executor_closures() {
    let spawned = Arc::new(AtomicUsize::new(0));
    let mut engine = Engine::with_prelude(()).unwrap();
    engine.set_async_call_policy(AsyncCallPolicy::executor_fn({
        let spawned = Arc::clone(&spawned);
        move |future| {
            spawned.fetch_add(1, Ordering::SeqCst);
            future
        }
    }));
    assert!(engine.async_call_policy().is_executor());

    let mut module = Module::global();
    module
        .export_async("bump", |_: &(), value: i32| async move { Ok(value + 1) })
        .unwrap();
    engine.inject_module(module).unwrap();

    let result = eval_i32("bump 10 + bump 20", engine).await;
    assert_eq!(result, 32);
    assert_eq!(spawned.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn small_ready_work_bound_still_completes_fanout() {
    let mut engine = Engine::with_prelude(()).unwrap();
    engine.set_execution_bounds(ExecutionBounds::new(1, 64));

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
        engine,
    )
    .await;
    assert_eq!(result, 272);
}

#[tokio::test]
async fn pending_async_bound_delays_admitting_host_calls() {
    let (mut engine, mut gate) = engine_with_gate(2);
    engine.set_execution_bounds(ExecutionBounds::new(64, 1));

    let eval_task = tokio::spawn(async move { eval_i32("sum [gate 1, gate 2]", engine).await });

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

    let mut engine = Engine::with_prelude(()).unwrap();
    engine.set_execution_bounds(ExecutionBounds::new(64, 1));
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
    engine.inject_module(module).unwrap();

    let eval_task =
        tokio::spawn(async move { eval_i32("sum [gate_call 1, gate_call 2]", engine).await });

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

#[tokio::test]
async fn gc_every_alloc_handles_broad_evaluator_paths() {
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
                when Empty -> 0;
                when Cons h t -> h + sum_list t;
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
            lefts = match unzipped with { when (left, right) -> left; },
            arr = to_array mapped,
            arr2 = map (\x -> x * 2) arr,
            flat: List i32 = bind (\x -> [x, x + 1]) [1, 2, 3],
            point: Point = Point { x = 10, y = 5 },
            point2: Point = { point with { y = 7 } },
            choice: Choice = Right { item = score point2 },
            chosen = match choice with {
                when Left {item} -> item;
                when Right {item} -> item + 1;
            },
            dict_val = match { foo = chosen, bar = foldl (\acc x -> acc + x) 0 evens } with {
                when {foo, bar} -> foo + bar;
            },
            mixed = ("gc", 1.5, true),
            tuple_score = ((1 is i32), (2 is i32), (3 is i32)).2
        in
            dict_val
                + sum_list flat
                + get (0 is i32) arr2
                + (if arr == arr then 1 else 0)
                + tuple_score
        "#,
        engine_collecting_on_every_alloc(),
    )
    .await;
    assert_eq!(result, 313);
}

#[tokio::test]
async fn gc_every_alloc_handles_host_callbacks_and_conversions() {
    let mut engine = Engine::with_prelude(()).unwrap();
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
    engine.inject_module(module).unwrap();
    engine.heap.set_collect_on_every_alloc(true).unwrap();

    let result = eval_i32(
        r#"
        let
            arr = pack (bump_async 4) (triple 3),
            xs = to_list arr,
            ys = map (\x -> x + 1) xs,
            zipped = zip xs ys,
            folded = foldl (\acc pair ->
                match pair with { when (left, right) -> acc + left + right; }
            ) 0 zipped
        in
            folded + sum arr
        "#,
        engine,
    )
    .await;
    assert_eq!(result, 87);
}

#[tokio::test]
async fn gc_every_alloc_handles_native_returning_nested_data() {
    let mut engine = Engine::with_prelude(()).unwrap();
    let mut module = Module::global();
    let i32_ty = Type::builtin(BuiltinTypeId::I32);
    let row_ty = Type::tuple(vec![i32_ty.clone(), Type::array(i32_ty.clone())]);
    let scheme = Scheme::new(
        vec![],
        vec![],
        Type::fun(i32_ty.clone(), Type::array(row_ty)),
    );
    module
        .export_native("make_nested", scheme, 1, |engine, _, args| {
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
                let mut values = Vec::new();
                for offset in 0..4 {
                    let item = engine.heap().alloc_i32(i + offset)?;
                    let label = engine.heap().alloc_string(format!("{i}:{offset}"))?;
                    let _ = engine.heap().alloc_tuple(vec![item.clone(), label])?;
                    values.push(item);
                }
                let array = engine.heap().alloc_array(values)?;
                let base = engine.heap().alloc_i32(i)?;
                let row = engine.heap().alloc_tuple(vec![base, array])?;
                for noise in 0..4 {
                    let value = engine.heap().alloc_i32(i * 100 + noise)?;
                    let _ = engine.heap().alloc_tuple(vec![row.clone(), value])?;
                }
                rows.push(row);
            }

            engine.heap().alloc_array(rows)
        })
        .unwrap();
    engine.inject_module(module).unwrap();
    engine.heap.set_collect_on_every_alloc(true).unwrap();

    let result = eval_i32(
        r#"
        let
            rows = make_nested 16,
            row_score = \row ->
                match row with { when (base, xs) -> base + sum xs; }
        in
            sum (map row_score rows)
        "#,
        engine,
    )
    .await;
    assert_eq!(result, 776);
}

#[tokio::test]
async fn gc_every_alloc_handles_self_referential_data() {
    let result = eval_i32(
        r#"
        let rec
            xs = Cons 1 xs
        in
            match xs with {
                when Cons h t ->
                    (match t with {
                        when Cons h2 _ -> h + h2;
                        when Empty -> 0;
                    });
                when Empty -> 0;
            }
        "#,
        engine_collecting_on_every_alloc(),
    )
    .await;
    assert_eq!(result, 2);
}

#[tokio::test]
async fn gc_every_alloc_handles_captured_closure_envs() {
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
        engine_collecting_on_every_alloc(),
    )
    .await;
    assert_eq!(result, 466);
}

#[tokio::test]
async fn gc_every_alloc_handles_typeclass_cached_values() {
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
        engine_collecting_on_every_alloc(),
    )
    .await;
    assert_eq!(result, 89);
}

#[tokio::test]
async fn gc_every_alloc_handles_async_native_handles_across_awaits() {
    let mut engine = Engine::with_prelude(()).unwrap();
    let mut module = Module::global();
    let array_i32 = Type::array(Type::builtin(BuiltinTypeId::I32));
    let i32_ty = Type::builtin(BuiltinTypeId::I32);
    let scheme = Scheme::new(vec![], vec![], Type::fun(array_i32, i32_ty));
    module
        .export_native_async(
            "async_sum_after_alloc",
            scheme,
            1,
            |engine, _, args: Vec<Handle>| {
                async move {
                    let retained = args.first().cloned().ok_or_else(|| {
                        EngineError::Internal("missing async_sum_after_alloc argument".into())
                    })?;
                    tokio::task::yield_now().await;
                    for value in 0..64 {
                        let value = engine.heap().alloc_i32(value)?;
                        let tuple = engine.heap().alloc_tuple(vec![value.clone(), value])?;
                        let _ = engine.heap().alloc_array(vec![tuple])?;
                    }
                    let mut sum = 0;
                    for value in retained.as_array()? {
                        sum += i32::from_rex(&value)?;
                    }
                    engine.heap().alloc_i32(sum)
                }
                .boxed()
            },
        )
        .unwrap();
    engine.inject_module(module).unwrap();
    engine.heap.set_collect_on_every_alloc(true).unwrap();

    let result = eval_i32(
        r#"
        async_sum_after_alloc (to_array [
            1, 2, 3, 4, 5, 6, 7, 8
        ])
        "#,
        engine,
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
                when Some x -> fact x;
                when None -> 0;
            }
        "#,
        Engine::with_prelude(()).unwrap(),
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
        Engine::with_prelude(()).unwrap(),
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
        Engine::with_prelude(()).unwrap(),
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
        Engine::with_prelude(()).unwrap(),
    )
    .await;
    assert_eq!(result, 84);
}
