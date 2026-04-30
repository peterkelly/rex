use rex_engine::{Compiler, Engine, EngineError, Evaluator, FromRex, Handle, Module, RuntimeEnv};
use rex_typesystem::types::Type;

async fn eval_value<State>(
    source: &str,
    engine: Engine<State>,
) -> Result<(Handle, Type), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let runtime = RuntimeEnv::new(engine.clone());
    let compiler = Compiler::new(engine);
    let mut evaluator = Evaluator::new_with_compiler(runtime, compiler);
    evaluator
        .eval_snippet(source)
        .await
        .map_err(|err| err.into_engine_error())
}

async fn eval_i32(source: &str, engine: Engine<()>) -> i32 {
    let (value, _typ) = eval_value(source, engine).await.unwrap();
    i32::from_rex(&value).unwrap()
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
        type Foo = Bar { x: i32, y: i32, z: i32 }
        type Sum = A { x: i32 } | B { x: i32 }

        let
            foo: Foo = Bar { x = 1, y = 2, z = 3 },
            tuple = (1, 2, 3),
            list = [1, 2, 3],
            foo2 = { foo with { x = 6 } },
            sum: Sum = A { x = 1 },
            sum2 = match sum
                when A {x} -> { sum with { x = x + 1 } }
                when B {x} -> { sum with { x = x + 2 } }
        in
            foo2.x + (match sum2 when A {x} -> x when B {x} -> x)
        "#,
        Engine::with_prelude(()).unwrap(),
    )
    .await;
    assert_eq!(result, 8);
}

#[tokio::test]
async fn gc_every_alloc_handles_broad_evaluator_paths() {
    let result = eval_i32(
        r#"
        type Point = Point { x: i32, y: i32 }
        type Choice = Left { item: i32 } | Right { item: i32 }

        class Score a where
            score : a -> i32

        instance Score Point where
            score = \p -> p.x + p.y

        let rec sum_list = \xs ->
            match xs
                when Empty -> 0
                when Cons h t -> h + sum_list t
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
            lefts = match unzipped when (left, right) -> left,
            arr = to_array mapped,
            arr2 = map (\x -> x * 2) arr,
            flat: List i32 = bind (\x -> [x, x + 1]) [1, 2, 3],
            point: Point = Point { x = 10, y = 5 },
            point2: Point = { point with { y = 7 } },
            choice: Choice = Right { item = score point2 },
            chosen = match choice
                when Left {item} -> item
                when Right {item} -> item + 1,
            dict_val = match { foo = chosen, bar = foldl (\acc x -> acc + x) 0 evens }
                when {foo, bar} -> foo + bar,
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
                match pair when (left, right) -> acc + left + right
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
async fn evaluator_handles_control_flow_typeclasses_and_recursion() {
    let result = eval_i32(
        r#"
        class Pick a where
            pick : a -> a

        instance Pick i32 where
            pick = \x -> x

        let rec fact = \n ->
            if n == 0 then 1 else n * fact (n - 1)
        in
            match (Some (pick 4))
                when Some x -> fact x
                when None -> 0
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
