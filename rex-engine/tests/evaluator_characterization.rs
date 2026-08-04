use rex_ast::Symbol;
use rex_engine::{Builder, CompileOptions, Context, EngineError, Module, Value};
use rex_parser::parse as parse_rex;
use rex_typesystem::types::{BuiltinTypeId, Scheme, Type, TypeKind};

async fn run_snippet(builder: Builder, source: &str) -> Result<(Value, Type), EngineError> {
    let compiler = builder.build_compiler();
    let parsed = parse_rex(source).unwrap();
    let (program, evaluator) = compiler
        .compile_program(&parsed, CompileOptions::for_module("test.main").unwrap())
        .await?;
    let typ = program.result_type().clone();
    let value = evaluator.run(program, Default::default()).await?;
    Ok((value, typ))
}

fn i32_type() -> Type {
    Type::builtin(BuiltinTypeId::I32)
}

fn inject_global_module(
    builder: &mut Builder,
    build: impl FnOnce(&mut Module<()>) -> Result<(), EngineError>,
) {
    let mut module = Module::global();
    build(&mut module).unwrap();
    builder.inject_module(module).unwrap();
}

#[tokio::test]
async fn owning_evaluator_resources_can_be_kept_after_run() {
    let compiler = Builder::with_prelude(()).unwrap().build_compiler();
    let parsed = parse_rex("(7 is i32)").unwrap();
    let (program, evaluator) = compiler
        .compile_program(&parsed, CompileOptions::for_module("test.main").unwrap())
        .await
        .unwrap();
    let type_system = evaluator.type_system();
    let value = evaluator.run(program, Default::default()).await.unwrap();

    assert_eq!(value.to_rust::<i32>().unwrap(), 7);
    assert!(
        type_system.adts.contains_key(&Symbol::intern("Option")),
        "the type system handle should remain usable after run consumes the evaluator"
    );
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

#[tokio::test]
async fn synchronous_native_callback_returns_owned_scalar() {
    let mut builder = Builder::with_prelude(()).unwrap();
    inject_global_module(&mut builder, |module| {
        module.export_native(
            "allocate_answer",
            Scheme::new(vec![], vec![], i32_type()),
            0,
            |_ctx: Context<()>, _, args| {
                assert!(args.is_empty());
                Ok(Value::I32(42))
            },
        )
    });
    builder.set_extreme_gc_stress(true);

    let (value, typ) = run_snippet(builder, "allocate_answer").await.unwrap();

    assert_eq!(typ, i32_type());
    assert_eq!(value.as_i32().unwrap(), 42);
}

#[tokio::test]
async fn synchronous_native_callback_owns_and_returns_argument() {
    let mut builder = Builder::with_prelude(()).unwrap();
    inject_global_module(&mut builder, |module| {
        module.export_native(
            "allocate_then_return",
            Scheme::new(vec![], vec![], Type::fun(i32_type(), i32_type())),
            1,
            |_ctx: Context<()>, _, args| {
                let argument = args
                    .into_iter()
                    .next()
                    .ok_or_else(|| EngineError::Internal("missing argument".into()))?;
                Ok(argument)
            },
        )
    });
    builder.set_extreme_gc_stress(true);

    let (value, typ) = run_snippet(builder, "allocate_then_return 73")
        .await
        .unwrap();

    assert_eq!(typ, i32_type());
    assert_eq!(value.as_i32().unwrap(), 73);
}

#[tokio::test]
async fn synchronous_native_callback_can_build_owned_composite_result() {
    let mut builder = Builder::with_prelude(()).unwrap();
    inject_global_module(&mut builder, |module| {
        let result_type = Type::tuple(vec![i32_type(), Type::list(i32_type())]);
        module.export_native(
            "allocate_row",
            Scheme::new(vec![], vec![], Type::fun(i32_type(), result_type)),
            1,
            |_ctx: Context<()>, _, args| {
                let start = args
                    .first()
                    .ok_or_else(|| EngineError::Internal("missing argument".into()))?
                    .as_i32()?;
                let values = (start..start + 4).map(Value::I32).collect();
                Ok(Value::Tuple(vec![Value::I32(start), Value::List(values)]))
            },
        )
    });
    builder.set_extreme_gc_stress(true);

    let (value, typ) = run_snippet(builder, "allocate_row 10").await.unwrap();

    assert_eq!(typ, Type::tuple(vec![i32_type(), Type::list(i32_type())]));
    let values = value.as_tuple().unwrap();
    assert_eq!(values[0].as_i32().unwrap(), 10);
    assert_eq!(
        values[1]
            .as_list()
            .unwrap()
            .iter()
            .map(|value| value.as_i32().unwrap())
            .collect::<Vec<_>>(),
        vec![10, 11, 12, 13]
    );
}

#[tokio::test]
async fn synchronous_native_callback_rejects_wrong_typed_result() {
    let mut builder = Builder::with_prelude(()).unwrap();
    inject_global_module(&mut builder, |module| {
        module.export_native(
            "foreign_value",
            Scheme::new(vec![], vec![], i32_type()),
            0,
            move |_ctx: Context<()>, _, _args| Ok(Value::Bool(true)),
        )
    });

    let error = run_snippet(builder, "foreign_value").await.unwrap_err();
    assert!(
        error.to_string().contains("expected i32"),
        "unexpected error: {error}"
    );
}
