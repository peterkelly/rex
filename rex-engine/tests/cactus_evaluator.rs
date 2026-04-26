use std::sync::{Arc, Mutex};

use futures::FutureExt;
use rex_engine::{
    Compiler, Engine, EngineError, Evaluator, EvaluatorRef, FromPointer, Module, NativeFuture,
    Pointer, RuntimeEnv, Value, apply_with_context,
};
use rex_typesystem::types::{BuiltinTypeId, Scheme, Type};
use rex_util::GasMeter;

fn inject_globals<State>(
    engine: &mut Engine<State>,
    build: impl FnOnce(&mut Module<State>) -> Result<(), EngineError>,
) where
    State: Clone + Send + Sync + 'static,
{
    let mut module = Module::global();
    build(&mut module).unwrap();
    engine.inject_module(module).unwrap();
}

fn engine_export_native_async<State, F>(
    engine: &mut Engine<State>,
    name: impl Into<String>,
    scheme: Scheme,
    arity: usize,
    handler: F,
) -> Result<(), EngineError>
where
    State: Clone + Send + Sync + 'static,
    F: Fn(EvaluatorRef<State>, Type, Vec<Pointer>) -> NativeFuture + Send + Sync + 'static,
{
    let mut module = Module::global();
    module.export_native_async(name, scheme, arity, handler)?;
    engine.inject_module(module)
}

async fn eval_value<State>(source: &str, engine: Engine<State>) -> (Engine<State>, Pointer, Type)
where
    State: Clone + Send + Sync + 'static,
{
    let runtime = RuntimeEnv::new(engine.clone());
    let compiler = Compiler::new(engine.clone());
    let mut evaluator = Evaluator::new_with_compiler(runtime, compiler);
    let mut gas = GasMeter::default();
    let (value, typ) = evaluator.eval_snippet(source, &mut gas).await.unwrap();
    (engine, value, typ)
}

async fn eval_i32(source: &str, engine: Engine<()>) -> i32 {
    let (engine, value, _typ) = eval_value(source, engine).await;
    i32::from_pointer(&engine.heap, &value).unwrap()
}

async fn eval_bool(source: &str, engine: Engine<()>) -> bool {
    let (engine, value, _typ) = eval_value(source, engine).await;
    engine.heap.pointer_as_bool(&value).unwrap()
}

async fn eval_public_bool<State>(source: &str, engine: Engine<State>) -> bool
where
    State: Clone + Send + Sync + 'static,
{
    let (engine, value, typ) = eval_value(source, engine).await;
    assert_eq!(typ, Type::builtin(BuiltinTypeId::Bool));
    engine.heap.pointer_as_bool(&value).unwrap()
}

async fn eval_public_string<State>(source: &str, engine: Engine<State>) -> String
where
    State: Clone + Send + Sync + 'static,
{
    let (engine, value, typ) = eval_value(source, engine).await;
    assert_eq!(typ, Type::builtin(BuiltinTypeId::String));
    String::from_pointer(&engine.heap, &value).unwrap()
}

fn ignore_log(_: &str) {}

#[derive(Clone, Default)]
struct ParentProbeState {
    outer_parent: Arc<Mutex<Option<Pointer>>>,
}

fn engine_with_context_marker() -> Engine<()> {
    let mut engine = Engine::with_prelude(()).unwrap();
    let bool_ty = Type::builtin(BuiltinTypeId::Bool);
    let i32_ty = Type::builtin(BuiltinTypeId::I32);
    inject_globals(&mut engine, |module| {
        module.export_native(
            "context_marker0",
            Scheme::new(vec![], vec![], bool_ty.clone()),
            0,
            |engine, _, _| engine.heap.alloc_bool(engine.context.parent.is_some()),
        )?;
        module.export_native(
            "context_marker",
            Scheme::new(vec![], vec![], Type::fun(i32_ty.clone(), bool_ty.clone())),
            1,
            |engine, _, _| engine.heap.alloc_bool(engine.context.parent.is_some()),
        )
    });

    let func_ty = Type::fun(i32_ty.clone(), bool_ty.clone());
    let call_once_ty = Type::fun(func_ty.clone(), Type::fun(i32_ty.clone(), bool_ty));
    engine_export_native_async(
        &mut engine,
        "call_once",
        Scheme::new(vec![], vec![], call_once_ty),
        2,
        move |engine, _, args| {
            let func_ty = func_ty.clone();
            let i32_ty = i32_ty.clone();
            async move {
                let mut gas = GasMeter::default();
                apply_with_context(
                    &engine,
                    args[0],
                    args[1],
                    Some(&func_ty),
                    Some(&i32_ty),
                    &mut gas,
                )
                .await
            }
            .boxed()
        },
    )
    .unwrap();
    engine
}

fn engine_with_context_marker_and_log_module() -> Engine<()> {
    let mut engine = engine_with_context_marker();
    engine.add_default_resolvers();
    let mut module = Module::new("host.log");
    module
        .export_tracing_log_function("debug", ignore_log)
        .unwrap();
    engine.inject_module(module).unwrap();
    engine
}

fn engine_with_parent_probe() -> Engine<ParentProbeState> {
    let mut engine = Engine::with_prelude(ParentProbeState::default()).unwrap();
    let bool_ty = Type::builtin(BuiltinTypeId::Bool);
    let i32_ty = Type::builtin(BuiltinTypeId::I32);
    let func_ty = Type::fun(i32_ty.clone(), bool_ty.clone());
    let call_once_ty = Type::fun(func_ty.clone(), Type::fun(i32_ty.clone(), bool_ty.clone()));

    engine_export_native_async(
        &mut engine,
        "call_once_record_parent",
        Scheme::new(vec![], vec![], call_once_ty),
        2,
        {
            let func_ty = func_ty.clone();
            let i32_ty = i32_ty.clone();
            move |engine, _, args| {
                let func_ty = func_ty.clone();
                let i32_ty = i32_ty.clone();
                async move {
                    {
                        let mut outer_parent = engine.state.outer_parent.lock().unwrap();
                        *outer_parent = engine.context.parent;
                    }

                    let mut gas = GasMeter::default();
                    apply_with_context(
                        &engine,
                        args[0],
                        args[1],
                        Some(&func_ty),
                        Some(&i32_ty),
                        &mut gas,
                    )
                    .await
                }
                .boxed()
            }
        },
    )
    .unwrap();

    inject_globals(&mut engine, |module| {
        module.export_native(
            "inner_parent_descends_from_outer",
            Scheme::new(vec![], vec![], bool_ty),
            0,
            |engine: EvaluatorRef<ParentProbeState>, _, _| {
                let outer_parent = *engine.state.outer_parent.lock().unwrap();
                let mut current = engine.context.parent;
                let mut found = false;

                while let Some(ptr) = current {
                    if Some(ptr) == outer_parent {
                        found = true;
                        break;
                    }

                    let frame = engine.heap.pointer_as_frame(&ptr)?;
                    let parent = *frame.parent();
                    match engine.heap.get(&parent)?.as_ref() {
                        Value::Frame(_) => current = Some(parent),
                        Value::U64(0) => break,
                        other => {
                            return Err(EngineError::Internal(format!(
                                "unexpected frame parent value {}",
                                other.value_type_name()
                            )));
                        }
                    }
                }

                engine.heap.alloc_bool(found)
            },
        )
    });

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
async fn evaluator_handles_control_flow_and_typeclasses() {
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
async fn native_callbacks_receive_cactus_context() {
    assert!(eval_bool("call_once context_marker 1", engine_with_context_marker()).await);
}

#[tokio::test]
async fn public_evaluator_uses_cactus_context() {
    assert!(eval_public_bool("context_marker0", engine_with_context_marker()).await);
}

#[tokio::test]
async fn module_log_callback_receives_cactus_context() {
    let rendered = eval_public_string(
        r#"
        import host.log (debug)

        type Context = Context { value: i32 }

        instance Show Context where
            show =
                if context_marker0
                then (\_ -> "context")
                else (\_ -> "missing")

        debug (Context { value = 1 })
        "#,
        engine_with_context_marker_and_log_module(),
    )
    .await;

    assert_eq!(rendered, "context");
}

#[tokio::test]
async fn native_callback_closure_runs_under_caller_frame() {
    assert!(
        eval_public_bool(
            "call_once_record_parent (\\_ -> inner_parent_descends_from_outer) 1",
            engine_with_parent_probe(),
        )
        .await
    );
}

#[tokio::test]
async fn class_method_resolution_receives_cactus_context() {
    assert!(
        eval_bool(
            r#"
            class Context a where
                marker : a -> bool

            instance Context i32 where
                marker =
                    if context_marker0
                    then (\x -> true)
                    else (\x -> false)

            marker 1
            "#,
            engine_with_context_marker(),
        )
        .await
    );
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
