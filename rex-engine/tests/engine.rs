use std::{collections::BTreeMap, sync::Arc};

use rex_ast::{CompilationUnit, Decl, Expr, Symbol};
use rex_engine::{Engine, EngineError, Module, Value, registry_markdown};
use rex_parser::parse as parse_rex;
use rex_typesystem::{
    error::TypeError,
    types::{BuiltinTypeId, Type, TypeVar},
};

fn parse(code: &str) -> Arc<Expr> {
    parse_rex(code).unwrap().body.unwrap()
}

fn parse_program(code: &str) -> CompilationUnit {
    parse_rex(code).unwrap()
}

fn strip_span(mut err: TypeError) -> TypeError {
    while let TypeError::Spanned { error, .. } = err {
        err = *error;
    }
    err
}

fn engine_with_arith() -> Engine {
    Engine::with_prelude(()).unwrap()
}
fn inject_globals(
    engine: &mut Engine,
    build: impl FnOnce(&mut Module<()>) -> Result<(), EngineError>,
) {
    let mut module = Module::<()>::global();
    build(&mut module).unwrap();
    engine.inject_module(module).unwrap();
}

#[test]
fn registry_markdown_lists_core_sections() {
    let engine = Engine::with_prelude(()).unwrap();
    let doc = registry_markdown(&engine);

    assert!(doc.contains("# Engine Registry"));
    assert!(doc.contains("## Module Index"));
    assert!(doc.contains("## Modules"));
    assert!(doc.contains("## ADTs"));
    assert!(doc.contains("## Functions and Values"));
    assert!(doc.contains("## Type Classes"));
    assert!(doc.contains("## Native Implementations"));
    assert!(doc.contains("[`virtual:Prelude`](#module-virtual-prelude)"));
    assert!(doc.contains("<a id=\"module-virtual-prelude\"></a>"));
    assert!(doc.contains("### `virtual:Prelude`"));
    assert!(doc.contains("`List`"));
    assert!(doc.contains("`Option`"));
}

#[tokio::test]
async fn compile_program_rejects_declaration_only_input() {
    let mut compiler = Engine::with_prelude(()).unwrap().into_compiler();
    let program = parse_program("fn id<a> x: a -> a = x;");
    let err = match compiler.compile_program(&program, Default::default()).await {
        Ok(_) => panic!("declaration-only program unexpectedly compiled"),
        Err(err) => err,
    };

    assert!(matches!(err, EngineError::MissingMain));
}

#[tokio::test]
async fn compile_program_uses_explicit_main_signature_and_runtime_inputs() {
    let program = parse_program("fn main x: i32 -> y: i32 -> i32 = x + y;");
    let mut compiler = Engine::with_prelude(()).unwrap().into_compiler();
    let compiled = compiler
        .compile_program(&program, Default::default())
        .await
        .unwrap();

    let signature = compiled.main_signature();
    assert_eq!(signature.inputs().len(), 2);
    assert_eq!(signature.inputs()[0].name, "x");
    assert_eq!(signature.inputs()[0].typ, Type::builtin(BuiltinTypeId::I32));
    assert_eq!(signature.inputs()[1].name, "y");
    assert_eq!(signature.result_type(), &Type::builtin(BuiltinTypeId::I32));
    assert_eq!(compiled.result_type(), &Type::builtin(BuiltinTypeId::I32));

    let evaluator = compiler.into_evaluator();
    let mut inputs = BTreeMap::new();
    inputs.insert("y".to_string(), evaluator.heap().alloc_i32(5).unwrap());
    inputs.insert("x".to_string(), evaluator.heap().alloc_i32(37).unwrap());
    let value = evaluator.run(compiled, inputs).await.unwrap();
    assert_eq!(value.as_i32().unwrap(), 42);
}

#[tokio::test]
async fn compile_program_preserves_function_results_from_main() {
    let program = parse_program("fn main x: i32 -> i32 -> i32 = \\ y -> x + y;");
    let mut compiler = Engine::with_prelude(()).unwrap().into_compiler();
    let compiled = compiler
        .compile_program(&program, Default::default())
        .await
        .unwrap();

    let signature = compiled.main_signature();
    assert_eq!(signature.inputs().len(), 1);
    assert_eq!(signature.inputs()[0].name, "x");
    assert_eq!(signature.inputs()[0].typ, Type::builtin(BuiltinTypeId::I32));
    assert_eq!(
        signature.result_type(),
        &Type::fun(
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32),
        )
    );
}

#[tokio::test]
async fn compile_program_treats_final_expression_as_zero_input_main() {
    let program = parse_program("1 + 2");
    let mut compiler = Engine::with_prelude(()).unwrap().into_compiler();
    let compiled = compiler
        .compile_program(&program, Default::default())
        .await
        .unwrap();

    assert!(compiled.main_signature().inputs().is_empty());
    assert_eq!(compiled.result_type(), &Type::builtin(BuiltinTypeId::I32));
    let value = compiler
        .into_evaluator()
        .run(compiled, Default::default())
        .await
        .unwrap();
    assert_eq!(value.as_i32().unwrap(), 3);
}

#[tokio::test]
async fn compile_program_rejects_main_plus_final_expression() {
    let program = parse_program("fn main x: i32 -> i32 = x;\n2");
    let mut compiler = Engine::with_prelude(()).unwrap().into_compiler();
    let err = match compiler.compile_program(&program, Default::default()).await {
        Ok(_) => panic!("main plus final expression unexpectedly compiled"),
        Err(err) => err,
    };

    assert!(matches!(err, EngineError::MainWithFinalExpression));
}

#[tokio::test]
async fn evaluator_rejects_missing_or_extra_main_inputs() {
    let program = parse_program("fn main x: i32 -> i32 = x;");
    let mut compiler = Engine::with_prelude(()).unwrap().into_compiler();
    let compiled = compiler
        .compile_program(&program, Default::default())
        .await
        .unwrap();
    let err = compiler
        .into_evaluator()
        .run(compiled, Default::default())
        .await
        .unwrap_err();

    assert!(matches!(
        err,
        EngineError::MainInputMismatch { missing, extra }
            if missing == vec!["x".to_string()] && extra.is_empty()
    ));

    let mut compiler = Engine::with_prelude(()).unwrap().into_compiler();
    let compiled = compiler
        .compile_program(&program, Default::default())
        .await
        .unwrap();
    let evaluator = compiler.into_evaluator();
    let mut inputs = BTreeMap::new();
    inputs.insert("x".to_string(), evaluator.heap().alloc_i32(1).unwrap());
    inputs.insert("y".to_string(), evaluator.heap().alloc_i32(2).unwrap());
    let err = evaluator.run(compiled, inputs).await.unwrap_err();

    assert!(matches!(
        err,
        EngineError::MainInputMismatch { missing, extra }
            if missing.is_empty() && extra == vec!["y".to_string()]
    ));
}

#[test]
fn module_add_adt_decls_from_types_collects_nested_unique_adts() {
    let mut engine = Engine::with_prelude(()).unwrap();
    let mut module = Module::new("acme.types");
    let a = Type::var(TypeVar::new(0, Some(Symbol::intern("a"))));
    let types = vec![
        Type::fun(
            Type::app(Type::user_con("Foo", 1), a.clone()),
            Type::user_con("Bar", 0),
        ),
        Type::app(Type::user_con("Foo", 1), Type::builtin(BuiltinTypeId::I32)),
    ];

    module.add_adt_decls_from_types(&mut engine, types).unwrap();

    assert_eq!(module.decls.len(), 2);
    assert!(
        module
            .decls
            .iter()
            .any(|d| matches!(d, Decl::Type(td) if td.name == Symbol::intern("Foo")))
    );
    assert!(
        module
            .decls
            .iter()
            .any(|d| matches!(d, Decl::Type(td) if td.name == Symbol::intern("Bar")))
    );
}

#[test]
fn module_add_adt_decls_from_types_rejects_conflicting_adts() {
    let mut engine = Engine::with_prelude(()).unwrap();
    let mut module = Module::new("acme.types");
    let types = vec![Type::user_con("Thing", 1), Type::user_con("Thing", 2)];

    let err = module
        .add_adt_decls_from_types(&mut engine, types)
        .unwrap_err();

    assert!(matches!(err, EngineError::Custom(_)));
    assert!(
        err.to_string()
            .contains("conflicting ADT definitions discovered in input types")
    );
}

#[test]
fn inject_adt_family_rejects_cycles() {
    let mut engine = Engine::with_prelude(()).unwrap();
    let mut a = engine.adt_decl("A", &[]);
    a.add_variant(Symbol::intern("A"), vec![Type::con("B", 0)]);
    let mut b = engine.adt_decl("B", &[]);
    b.add_variant(Symbol::intern("B"), vec![Type::con("A", 0)]);

    let mut module = Module::<()>::global();
    let err = module.add_adt_family(vec![a, b]).unwrap_err();
    assert!(matches!(err, EngineError::Custom(_)));
    assert!(err.to_string().contains("cyclic ADT auto-registration"));
}

#[tokio::test]
async fn injected_module_can_define_pub_adt_declarations() {
    let mut engine = Engine::with_prelude(()).unwrap();

    let mut module = Module::new("acme.status");
    let mut status = engine.adt_decl("Status", &[]);
    status.add_variant(Symbol::intern("Ready"), vec![]);
    status.add_variant(
        Symbol::intern("Failed"),
        vec![Type::builtin(BuiltinTypeId::String)],
    );
    module.add_adt_decl(status).unwrap();
    engine.inject_module(module).unwrap();

    let mut compiler = engine.into_compiler();
    let parsed = parse_program(
        r#"
            import acme.status (Failed);
            Failed "boom"
            "#,
    );
    let program = compiler
        .compile_program(&parsed, Default::default())
        .await
        .unwrap();
    let value = compiler
        .into_evaluator()
        .run(program, Default::default())
        .await
        .unwrap();

    match value.value().unwrap() {
        Value::Adt(tag, args) => {
            assert_eq!(tag.as_ref(), "Failed");
            assert_eq!(args.len(), 1);
        }
        _ => panic!("expected ADT value"),
    }
}

#[tokio::test]
async fn export_value_registers_global_value() {
    let expr = parse("answer");
    let mut engine = Engine::with_prelude(()).unwrap();
    inject_globals(&mut engine, |module| module.export_value("answer", 42i32));
    let mut compiler = engine.into_compiler();
    let body_program = CompilationUnit {
        decls: Vec::new(),
        body: Some(expr),
    };
    let compiled = compiler
        .compile_program(&body_program, Default::default())
        .await
        .unwrap();
    let ty = compiled.result_type().clone();
    let value = compiler
        .into_evaluator()
        .run(compiled, Default::default())
        .await
        .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_eq!(value.to_rust::<i32>().unwrap(), 42);
}

#[tokio::test]
async fn record_update_requires_known_variant_for_sum_types() {
    let program = parse_program(
        r#"
        type Foo = Bar { x: i32 } | Baz { x: i32 };
        let
          f = \ (foo : Foo) -> { foo with { x = 2 } }
        in
          f (Bar { x = 1 })
        "#,
    );
    let mut engine = engine_with_arith();
    let mut module = Module::global();
    module.add_decls(program.decls.clone());
    engine.inject_module(module).unwrap();
    let mut compiler = engine.into_compiler();
    let body_program = CompilationUnit {
        decls: Vec::new(),
        body: program.body.clone(),
    };
    match compiler
        .compile_program(&body_program, Default::default())
        .await
    {
        Err(err) => {
            let EngineError::Type(err) = err else {
                panic!("expected type error");
            };
            let err = strip_span(err);
            assert!(matches!(err, TypeError::FieldNotKnown { .. }));
        }
        Ok(compiled) => {
            let result = compiler
                .into_evaluator()
                .run(compiled, Default::default())
                .await;
            match result {
                Err(err) => {
                    let EngineError::Type(err) = err else {
                        panic!("expected type error");
                    };
                    let err = strip_span(err);
                    assert!(matches!(err, TypeError::FieldNotKnown { .. }));
                }
                Ok(_) => panic!("expected type error"),
            }
        }
    }
}
