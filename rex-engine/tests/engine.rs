use std::path::Path;
use std::sync::Arc;

use rex_ast::expr::{Decl, Expr, Program, Symbol};
use rex_engine::{Compiler, Engine, EngineError, Evaluator, Module, ReplState, RuntimeEnv, Value};
use rex_lexer::Token;
use rex_parser::Parser;
use rex_typesystem::{
    error::TypeError,
    types::{BuiltinTypeId, Type, TypeVar},
};

fn parse(code: &str) -> Arc<Expr> {
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    parser.parse_program().unwrap().expr
}

fn parse_program(code: &str) -> Program {
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    parser.parse_program().unwrap()
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
    let doc = engine.registry_markdown();

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

    assert_eq!(module.structured_decls.len(), 2);
    assert!(
        module
            .structured_decls
            .iter()
            .any(|d| matches!(d, Decl::Type(td) if td.name == Symbol::intern("Foo")))
    );
    assert!(
        module
            .structured_decls
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
async fn repl_persists_function_definitions() {
    let mut engine = Engine::with_prelude(()).unwrap();
    engine.add_default_resolvers();
    let mut state = ReplState::new();
    let mut evaluator = Evaluator::new_with_compiler(
        RuntimeEnv::new(engine.clone()),
        Compiler::new(engine.clone()),
    );

    let program1 = parse_program("fn inc (x: i32) -> i32 = x + 1;\ninc 1");
    let (v1, t1) = evaluator
        .eval_repl_program(&program1, &mut state)
        .await
        .unwrap();
    assert_eq!(t1, Type::builtin(BuiltinTypeId::I32));
    assert_eq!(v1.to_rust::<i32>().unwrap(), 2);

    let program2 = parse_program("inc 2");
    let (v2, t2) = evaluator
        .eval_repl_program(&program2, &mut state)
        .await
        .unwrap();
    assert_eq!(t2, Type::builtin(BuiltinTypeId::I32));
    assert_eq!(v2.to_rust::<i32>().unwrap(), 3);
}

#[tokio::test]
async fn repl_persists_import_aliases() {
    let mut engine = Engine::with_prelude(()).unwrap();
    engine.add_default_resolvers();

    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rex-cli/examples/modules_basic");
    engine.add_include_resolver(&examples).unwrap();

    let mut state = ReplState::new();
    let mut evaluator = Evaluator::new_with_compiler(
        RuntimeEnv::new(engine.clone()),
        Compiler::new(engine.clone()),
    );
    let program1 = parse_program("import foo.bar as Bar\n()");
    let (v1, t1) = evaluator
        .eval_repl_program(&program1, &mut state)
        .await
        .unwrap();
    assert_eq!(t1, Type::tuple(vec![]));
    assert!(
        v1.value_eq(&engine.heap.alloc_tuple(vec![]).unwrap())
            .unwrap()
    );

    let program2 = parse_program("Bar.triple 10");
    let (v2, t2) = evaluator
        .eval_repl_program(&program2, &mut state)
        .await
        .unwrap();
    assert_eq!(t2, Type::builtin(BuiltinTypeId::I32));
    assert_eq!(v2.to_rust::<i32>().unwrap(), 30);
}

#[tokio::test]
async fn repl_persists_imported_values() {
    let mut engine = Engine::with_prelude(()).unwrap();
    engine.add_default_resolvers();

    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rex-cli/examples/modules_basic");
    engine.add_include_resolver(&examples).unwrap();

    let mut state = ReplState::new();
    let mut evaluator = Evaluator::new_with_compiler(
        RuntimeEnv::new(engine.clone()),
        Compiler::new(engine.clone()),
    );
    let program1 = parse_program("import foo.bar (triple as t)\n()");
    let (v1, t1) = evaluator
        .eval_repl_program(&program1, &mut state)
        .await
        .unwrap();
    assert_eq!(t1, Type::tuple(vec![]));
    assert!(
        v1.value_eq(&engine.heap.alloc_tuple(vec![]).unwrap())
            .unwrap()
    );

    let program2 = parse_program("t 10");
    let (v2, t2) = evaluator
        .eval_repl_program(&program2, &mut state)
        .await
        .unwrap();
    assert_eq!(t2, Type::builtin(BuiltinTypeId::I32));
    assert_eq!(v2.to_rust::<i32>().unwrap(), 30);
}

#[tokio::test]
async fn injected_module_can_define_pub_adt_declarations() {
    let mut engine = Engine::with_prelude(()).unwrap();
    engine.add_default_resolvers();

    let mut module = Module::new("acme.status");
    module
        .add_raw_declaration("pub type Status = Ready | Failed string")
        .unwrap();
    engine.inject_module(module).unwrap();

    let (value, _ty) = Evaluator::new_with_compiler(
        RuntimeEnv::new(engine.clone()),
        Compiler::new(engine.clone()),
    )
    .eval_snippet(
        r#"
            import acme.status (Failed)
            Failed "boom"
            "#,
    )
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
    let (value, ty) = Evaluator::new_with_compiler(
        RuntimeEnv::new(engine.clone()),
        Compiler::new(engine.clone()),
    )
    .eval(expr.as_ref())
    .await
    .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_eq!(value.to_rust::<i32>().unwrap(), 42);
}

#[tokio::test]
async fn record_update_requires_known_variant_for_sum_types() {
    let program = parse_program(
        r#"
        type Foo = Bar { x: i32 } | Baz { x: i32 }
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
    match Evaluator::new_with_compiler(
        RuntimeEnv::new(engine.clone()),
        Compiler::new(engine.clone()),
    )
    .eval(program.expr.as_ref())
    .await
    {
        Err(err) => {
            let EngineError::Type(err) = err.into_engine_error() else {
                panic!("expected type error");
            };
            let err = strip_span(err);
            assert!(matches!(err, TypeError::FieldNotKnown { .. }));
        }
        _ => panic!("expected type error"),
    }
}
