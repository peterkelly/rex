use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use futures::FutureExt;
use rex_engine::{
    Compiler, Engine, EngineError, Evaluator, Module, Pointer, RuntimeEnv, assert_pointer_eq,
};
use rex_typesystem::types::{BuiltinTypeId, Scheme, Type, TypeKind};
use rex_util::{GasCosts, GasMeter};

fn unlimited_gas() -> GasMeter {
    GasMeter::default()
}

fn inject_globals(
    engine: &mut Engine,
    build: impl FnOnce(&mut Module<()>) -> Result<(), EngineError>,
) {
    let mut module = Module::global();
    build(&mut module).unwrap();
    engine.inject_module(module).unwrap();
}

async fn eval_snippet(engine: &mut Engine, source: &str) -> Result<(Pointer, Type), EngineError> {
    let mut gas = unlimited_gas();
    Evaluator::new_with_compiler(
        RuntimeEnv::new(engine.clone()),
        Compiler::new(engine.clone()),
    )
    .eval_snippet(source, &mut gas)
    .await
    .map_err(|err| err.into_engine_error())
}

async fn run_compiled_snippet_with_eval_gas(
    engine: &mut Engine,
    source: &str,
    eval_gas: &mut GasMeter,
) -> Result<Pointer, EngineError> {
    let mut compile_gas = unlimited_gas();
    let mut compiler = Compiler::new(engine.clone());
    let program = compiler.compile_snippet(source, &mut compile_gas).unwrap();
    Evaluator::new(RuntimeEnv::new(engine.clone()))
        .run(&program, eval_gas)
        .await
        .map_err(|err| err.into_engine_error())
}

#[tokio::test]
async fn prelude_map_callbacks_share_outer_gas_meter() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut engine = Engine::with_prelude(()).unwrap();
    let scheme = Scheme::new(
        vec![],
        vec![],
        Type::fun(
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32),
        ),
    );
    inject_globals(&mut engine, |module| {
        module.export_native_async_with_gas_cost("expensive", scheme, 1, 10_000, {
            let calls = Arc::clone(&calls);
            move |engine, _typ, args| {
                let calls = Arc::clone(&calls);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    let value = engine.heap.pointer_as_i32(&args[0])?;
                    engine.heap.alloc_i32(value)
                }
                .boxed()
            }
        })
    });

    let mut eval_gas = GasMeter::new(
        Some(1_000),
        GasCosts {
            eval_node: 1,
            eval_app_step: 1,
            native_call_base: 1,
            native_call_per_arg: 0,
            ..GasCosts::sensible_defaults()
        },
    );
    let err =
        run_compiled_snippet_with_eval_gas(&mut engine, "map expensive [1, 2]", &mut eval_gas)
            .await
            .unwrap_err();

    assert!(
        matches!(err, EngineError::OutOfGas(_)),
        "expected callback gas to be charged to the outer meter, got {err:?}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn baseline_control_flow_typeclass_and_recursion_paths_still_evaluate() {
    let mut engine = Engine::with_prelude(()).unwrap();
    let (value, ty) = eval_snippet(
        &mut engine,
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
    )
    .await
    .unwrap();

    assert!(
        matches!(ty.as_ref(), TypeKind::Con(con) if con.builtin_id == Some(BuiltinTypeId::I32))
            || matches!(ty.as_ref(), TypeKind::Var(_)),
        "expected i32-compatible result type, got {ty}"
    );
    assert_pointer_eq!(&engine.heap, value, engine.heap.alloc_i32(24).unwrap());
}
