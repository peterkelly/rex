use rex_engine::{Compiler, Engine, EngineError, Evaluator, Handle, RuntimeEnv};
use rex_typesystem::types::{BuiltinTypeId, Type, TypeKind};

async fn eval_snippet(engine: &mut Engine, source: &str) -> Result<(Handle, Type), EngineError> {
    Evaluator::new_with_compiler(
        RuntimeEnv::new(engine.clone()),
        Compiler::new(engine.clone()),
    )
    .eval_snippet(source)
    .await
    .map_err(|err| err.into_engine_error())
}

#[tokio::test]
async fn baseline_control_flow_typeclass_and_recursion_paths_still_evaluate() {
    let mut engine = Engine::with_prelude(()).unwrap();
    let (value, ty) = eval_snippet(
        &mut engine,
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
        matches!(ty.as_ref(), TypeKind::Con(con) if con.builtin_id == Some(BuiltinTypeId::I32))
            || matches!(ty.as_ref(), TypeKind::Var(_)),
        "expected i32-compatible result type, got {ty}"
    );
    assert_eq!(value.to_rust::<i32>().unwrap(), 24);
}
