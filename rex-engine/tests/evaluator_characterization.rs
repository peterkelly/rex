use rex_ast::Symbol;
use rex_engine::{Builder, EngineError, Handle};
use rex_parser::parse as parse_rex;
use rex_typesystem::types::{BuiltinTypeId, Type, TypeKind};

async fn run_snippet(builder: Builder, source: &str) -> Result<(Handle, Type), EngineError> {
    let compiler = builder.build_compiler();
    let parsed = parse_rex(source).unwrap();
    let (program, evaluator) = compiler
        .compile_program(&parsed, Default::default())
        .await?;
    let typ = program.result_type().clone();
    let value = evaluator.run(program, Default::default()).await?;
    Ok((value, typ))
}

#[tokio::test]
async fn owning_evaluator_resources_can_be_kept_after_run() {
    let compiler = Builder::with_prelude(()).unwrap().build_compiler();
    let parsed = parse_rex("(7 is i32)").unwrap();
    let (program, evaluator) = compiler
        .compile_program(&parsed, Default::default())
        .await
        .unwrap();
    let type_system = evaluator.type_system();
    let heap = evaluator.heap().clone();

    let value = evaluator.run(program, Default::default()).await.unwrap();

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
    let builder = Builder::with_prelude(()).unwrap();
    let (value, ty) = run_snippet(
        builder,
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
