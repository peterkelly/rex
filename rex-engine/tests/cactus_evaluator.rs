use rex_engine::{Compiler, Engine, EngineError, Evaluator, FromRex, Handle, RuntimeEnv};
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
