use rex_ast::expr::Symbol;
use rex_engine::{Engine, EngineError, Handle};
use rex_typesystem::types::{BuiltinTypeId, Type, TypeKind};

async fn eval_snippet(engine: Engine, source: &str) -> Result<(Handle, Type), EngineError> {
    engine
        .into_evaluator()
        .eval_snippet(source)
        .await
        .map_err(|err| err.into_engine_error())
}

#[tokio::test]
async fn owning_evaluator_resources_can_be_kept_after_run() {
    let mut compiler = Engine::with_prelude(()).unwrap().into_compiler();
    let program = compiler.compile_snippet("(7 is i32)").unwrap();
    let evaluator = compiler.into_evaluator();
    let type_system = evaluator.type_system();
    let heap = evaluator.heap().clone();

    let value = evaluator.run(program).await.unwrap();

    assert_eq!(value.to_rust::<i32>().unwrap(), 7);
    assert!(
        type_system.adts.contains_key(&Symbol::intern("Option")),
        "the type system handle should remain usable after run consumes the evaluator"
    );
    let extra = heap.alloc_i32(8).unwrap();
    assert_eq!(extra.to_rust::<i32>().unwrap(), 8);
}

#[tokio::test]
async fn baseline_control_flow_typeclass_and_recursion_paths_still_evaluate() {
    let engine = Engine::with_prelude(()).unwrap();
    let (value, ty) = eval_snippet(
        engine,
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
    )
    .await
    .unwrap();

    assert!(
        matches!(ty.as_ref(), TypeKind::Con(con) if con.is_builtin(BuiltinTypeId::I32))
            || matches!(ty.as_ref(), TypeKind::Var(_)),
        "expected i32-compatible result type, got {ty}"
    );
    assert_eq!(value.to_rust::<i32>().unwrap(), 24);
}
