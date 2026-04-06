use std::path::Path;
use std::sync::Arc;

use futures::FutureExt;
use rexlang_ast::expr::{Expr, Program, sym};
use rexlang_engine::{
    CancellationToken, Engine, EngineError, Library, ReplState, Value, assert_pointer_eq,
};
use rexlang_lexer::Token;
use rexlang_parser::Parser;
use rexlang_typesystem::{BuiltinTypeId, Scheme, Type, TypeError, TypeVar};
use rexlang_util::{GasCosts, GasMeter};

fn parse(code: &str) -> Arc<Expr> {
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    parser.parse_program(&mut GasMeter::default()).unwrap().expr
}

fn parse_program(code: &str) -> Program {
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    parser.parse_program(&mut GasMeter::default()).unwrap()
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

fn unlimited_gas() -> GasMeter {
    GasMeter::default()
}

fn inject_globals(
    engine: &mut Engine,
    build: impl FnOnce(&mut Library<()>) -> Result<(), EngineError>,
) {
    let mut library = Library::<()>::global();
    build(&mut library).unwrap();
    engine.inject_library(library).unwrap();
}

#[test]
fn registry_markdown_lists_core_sections() {
    let engine = Engine::with_prelude(()).unwrap();
    let doc = engine.registry_markdown();

    assert!(doc.contains("# Engine Registry"));
    assert!(doc.contains("## Library Index"));
    assert!(doc.contains("## Libraries"));
    assert!(doc.contains("## ADTs"));
    assert!(doc.contains("## Functions and Values"));
    assert!(doc.contains("## Type Classes"));
    assert!(doc.contains("## Native Implementations"));
    assert!(doc.contains("[`virtual:Prelude`](#library-virtual-prelude)"));
    assert!(doc.contains("<a id=\"library-virtual-prelude\"></a>"));
    assert!(doc.contains("### `virtual:Prelude`"));
    assert!(doc.contains("`List`"));
    assert!(doc.contains("`Option`"));
}

#[test]
fn library_add_adt_decls_from_types_collects_nested_unique_adts() {
    let mut engine = Engine::with_prelude(()).unwrap();
    let mut library = Library::new("acme.types");
    let a = Type::var(TypeVar::new(0, Some(sym("a"))));
    let types = vec![
        Type::fun(
            Type::app(Type::user_con("Foo", 1), a.clone()),
            Type::user_con("Bar", 0),
        ),
        Type::app(Type::user_con("Foo", 1), Type::builtin(BuiltinTypeId::I32)),
    ];

    library
        .add_adt_decls_from_types(&mut engine, types)
        .unwrap();

    assert_eq!(library.structured_decls.len(), 2);
    assert!(
        library
            .structured_decls
            .iter()
            .any(|d| matches!(d, rexlang_ast::expr::Decl::Type(td) if td.name == sym("Foo")))
    );
    assert!(
        library
            .structured_decls
            .iter()
            .any(|d| matches!(d, rexlang_ast::expr::Decl::Type(td) if td.name == sym("Bar")))
    );
}

#[test]
fn library_add_adt_decls_from_types_rejects_conflicting_adts() {
    let mut engine = Engine::with_prelude(()).unwrap();
    let mut library = Library::new("acme.types");
    let types = vec![Type::user_con("Thing", 1), Type::user_con("Thing", 2)];

    let err = library
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
    a.add_variant(sym("A"), vec![Type::con("B", 0)]);
    let mut b = engine.adt_decl("B", &[]);
    b.add_variant(sym("B"), vec![Type::con("A", 0)]);

    let mut library = Library::<()>::global();
    let err = library.add_adt_family(vec![a, b]).unwrap_err();
    assert!(matches!(err, EngineError::Custom(_)));
    assert!(err.to_string().contains("cyclic ADT auto-registration"));
}

#[tokio::test]
async fn repl_persists_function_definitions() {
    let mut gas = unlimited_gas();
    let mut engine = Engine::with_prelude(()).unwrap();
    engine.add_default_resolvers();
    let mut state = ReplState::new();
    let mut evaluator = rexlang_engine::Evaluator::new_with_compiler(
        rexlang_engine::RuntimeEnv::new(engine.clone()),
        rexlang_engine::Compiler::new(engine.clone()),
    );

    let program1 = parse_program("fn inc (x: i32) -> i32 = x + 1\ninc 1");
    let (v1, t1) = evaluator
        .eval_repl_program(&program1, &mut state, &mut gas)
        .await
        .unwrap();
    assert_eq!(t1, Type::builtin(BuiltinTypeId::I32));
    assert_pointer_eq!(&engine.heap, v1, engine.heap.alloc_i32(2).unwrap());

    let program2 = parse_program("inc 2");
    let (v2, t2) = evaluator
        .eval_repl_program(&program2, &mut state, &mut gas)
        .await
        .unwrap();
    assert_eq!(t2, Type::builtin(BuiltinTypeId::I32));
    assert_pointer_eq!(&engine.heap, v2, engine.heap.alloc_i32(3).unwrap());
}

#[tokio::test]
async fn repl_persists_import_aliases() {
    let mut gas = unlimited_gas();
    let mut engine = Engine::with_prelude(()).unwrap();
    engine.add_default_resolvers();

    let examples =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../rexlang-cli/examples/libraries_basic");
    engine.add_include_resolver(&examples).unwrap();

    let mut state = ReplState::new();
    let mut evaluator = rexlang_engine::Evaluator::new_with_compiler(
        rexlang_engine::RuntimeEnv::new(engine.clone()),
        rexlang_engine::Compiler::new(engine.clone()),
    );
    let program1 = parse_program("import foo.bar as Bar\n()");
    let (v1, t1) = evaluator
        .eval_repl_program(&program1, &mut state, &mut gas)
        .await
        .unwrap();
    assert_eq!(t1, Type::tuple(vec![]));
    assert_pointer_eq!(&engine.heap, v1, engine.heap.alloc_tuple(vec![]).unwrap());

    let program2 = parse_program("Bar.triple 10");
    let (v2, t2) = evaluator
        .eval_repl_program(&program2, &mut state, &mut gas)
        .await
        .unwrap();
    assert_eq!(t2, Type::builtin(BuiltinTypeId::I32));
    assert_pointer_eq!(&engine.heap, v2, engine.heap.alloc_i32(30).unwrap());
}

#[tokio::test]
async fn repl_persists_imported_values() {
    let mut gas = unlimited_gas();
    let mut engine = Engine::with_prelude(()).unwrap();
    engine.add_default_resolvers();

    let examples =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../rexlang-cli/examples/libraries_basic");
    engine.add_include_resolver(&examples).unwrap();

    let mut state = ReplState::new();
    let mut evaluator = rexlang_engine::Evaluator::new_with_compiler(
        rexlang_engine::RuntimeEnv::new(engine.clone()),
        rexlang_engine::Compiler::new(engine.clone()),
    );
    let program1 = parse_program("import foo.bar (triple as t)\n()");
    let (v1, t1) = evaluator
        .eval_repl_program(&program1, &mut state, &mut gas)
        .await
        .unwrap();
    assert_eq!(t1, Type::tuple(vec![]));
    assert_pointer_eq!(&engine.heap, v1, engine.heap.alloc_tuple(vec![]).unwrap());

    let program2 = parse_program("t 10");
    let (v2, t2) = evaluator
        .eval_repl_program(&program2, &mut state, &mut gas)
        .await
        .unwrap();
    assert_eq!(t2, Type::builtin(BuiltinTypeId::I32));
    assert_pointer_eq!(&engine.heap, v2, engine.heap.alloc_i32(30).unwrap());
}

#[tokio::test]
async fn injected_library_can_define_pub_adt_declarations() {
    let mut gas = unlimited_gas();
    let mut engine = Engine::with_prelude(()).unwrap();
    engine.add_default_resolvers();

    let mut library = Library::new("acme.status");
    library
        .add_raw_declaration("pub type Status = Ready | Failed string")
        .unwrap();
    engine.inject_library(library).unwrap();

    let (value, _ty) = rexlang_engine::Evaluator::new_with_compiler(
        rexlang_engine::RuntimeEnv::new(engine.clone()),
        rexlang_engine::Compiler::new(engine.clone()),
    )
    .eval_snippet(
        r#"
            import acme.status (Failed)
            Failed "boom"
            "#,
        &mut gas,
    )
    .await
    .unwrap();

    let v = engine.heap.get(&value).unwrap();
    match v.as_ref() {
        Value::Adt(tag, args) => {
            assert!(tag.as_ref().ends_with(".Failed"));
            assert_eq!(args.len(), 1);
        }
        _ => panic!("expected ADT value"),
    }
}

#[tokio::test]
async fn eval_can_be_cancelled_while_waiting_on_async_native() {
    let expr = parse("stall");
    let mut engine = Engine::with_prelude(()).unwrap();

    let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
    let scheme = Scheme::new(vec![], vec![], Type::builtin(BuiltinTypeId::I32));
    inject_globals(&mut engine, |library| {
        library.export_native_async_cancellable(
            "stall",
            scheme,
            0,
            move |engine, token: CancellationToken, _, _args| {
                let started_tx = started_tx.clone();
                async move {
                    let _ = started_tx.send(());
                    token.cancelled().await;
                    engine.heap.alloc_i32(0)
                }
                .boxed()
            },
        )
    });

    let token = engine.cancellation_token();
    let canceller = std::thread::spawn(move || {
        let recv = started_rx.recv_timeout(std::time::Duration::from_secs(2));
        assert!(recv.is_ok(), "stall native never started");
        token.cancel();
    });

    let mut gas = unlimited_gas();
    let res = rexlang_engine::Evaluator::new_with_compiler(
        rexlang_engine::RuntimeEnv::new(engine.clone()),
        rexlang_engine::Compiler::new(engine.clone()),
    )
    .eval(expr.as_ref(), &mut gas)
    .await;
    let joined = canceller.join();
    assert!(joined.is_ok(), "cancel thread panicked");
    assert!(matches!(
        res,
        Err(ref err) if matches!(err.as_engine_error(), EngineError::Cancelled)
    ));
}

#[tokio::test]
async fn eval_can_be_cancelled_while_waiting_on_non_cancellable_async_native() {
    let expr = parse("stall");
    let mut engine = Engine::with_prelude(()).unwrap();

    let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
    inject_globals(&mut engine, |library| {
        library.export_async("stall", move |_state: &()| {
            let started_tx = started_tx.clone();
            async move {
                let _ = started_tx.send(());
                futures::future::pending::<Result<i32, EngineError>>().await
            }
        })
    });

    let token = engine.cancellation_token();
    let canceller = std::thread::spawn(move || {
        let recv = started_rx.recv_timeout(std::time::Duration::from_secs(2));
        assert!(recv.is_ok(), "stall native never started");
        token.cancel();
    });

    let mut gas = unlimited_gas();
    let res = rexlang_engine::Evaluator::new_with_compiler(
        rexlang_engine::RuntimeEnv::new(engine.clone()),
        rexlang_engine::Compiler::new(engine.clone()),
    )
    .eval(expr.as_ref(), &mut gas)
    .await;
    let joined = canceller.join();
    assert!(joined.is_ok(), "cancel thread panicked");
    assert!(matches!(
        res,
        Err(ref err) if matches!(err.as_engine_error(), EngineError::Cancelled)
    ));
}

#[tokio::test]
async fn native_per_impl_gas_cost_is_charged() {
    let expr = parse("foo");
    let mut engine = Engine::with_prelude(()).unwrap();
    let scheme = Scheme::new(vec![], vec![], Type::builtin(BuiltinTypeId::I32));
    inject_globals(&mut engine, |library| {
        library.export_native_with_gas_cost("foo", scheme, 0, 50, |engine, _t, _args| {
            engine.heap.alloc_i32(1)
        })
    });

    let mut gas = GasMeter::new(
        Some(10),
        GasCosts {
            eval_node: 1,
            native_call_base: 1,
            native_call_per_arg: 0,
            ..GasCosts::sensible_defaults()
        },
    );
    let err = match rexlang_engine::Evaluator::new_with_compiler(
        rexlang_engine::RuntimeEnv::new(engine.clone()),
        rexlang_engine::Compiler::new(engine.clone()),
    )
    .eval(expr.as_ref(), &mut gas)
    .await
    {
        Ok(_) => panic!("expected out of gas"),
        Err(e) => e,
    };
    assert!(matches!(err.as_engine_error(), EngineError::OutOfGas(..)));
}

#[tokio::test]
async fn export_value_typed_registers_global_value() {
    let expr = parse("answer");
    let mut engine = Engine::with_prelude(()).unwrap();
    inject_globals(&mut engine, |library| {
        library.export_value_typed("answer", Type::builtin(BuiltinTypeId::I32), Value::I32(42))
    });

    let mut gas = unlimited_gas();
    let (value, ty) = rexlang_engine::Evaluator::new_with_compiler(
        rexlang_engine::RuntimeEnv::new(engine.clone()),
        rexlang_engine::Compiler::new(engine.clone()),
    )
    .eval(expr.as_ref(), &mut gas)
    .await
    .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_pointer_eq!(&engine.heap, value, engine.heap.alloc_i32(42).unwrap());
}

#[tokio::test]
async fn async_native_per_impl_gas_cost_is_charged() {
    let expr = parse("foo");
    let mut engine = Engine::with_prelude(()).unwrap();
    let scheme = Scheme::new(vec![], vec![], Type::builtin(BuiltinTypeId::I32));
    inject_globals(&mut engine, |library| {
        library.export_native_async_with_gas_cost("foo", scheme, 0, 50, |engine, _t, _args| {
            async move { engine.heap.alloc_i32(1) }.boxed()
        })
    });

    let mut gas = GasMeter::new(
        Some(10),
        GasCosts {
            eval_node: 1,
            native_call_base: 1,
            native_call_per_arg: 0,
            ..GasCosts::sensible_defaults()
        },
    );
    let err = match rexlang_engine::Evaluator::new_with_compiler(
        rexlang_engine::RuntimeEnv::new(engine.clone()),
        rexlang_engine::Compiler::new(engine.clone()),
    )
    .eval(expr.as_ref(), &mut gas)
    .await
    {
        Ok(_) => panic!("expected out of gas"),
        Err(e) => e,
    };
    assert!(matches!(err.as_engine_error(), EngineError::OutOfGas(..)));
}

#[tokio::test]
async fn cancellable_async_native_per_impl_gas_cost_is_charged() {
    let expr = parse("foo");
    let mut engine = Engine::with_prelude(()).unwrap();
    let scheme = Scheme::new(vec![], vec![], Type::builtin(BuiltinTypeId::I32));
    inject_globals(&mut engine, |library| {
        library.export_native_async_cancellable_with_gas_cost(
            "foo",
            scheme,
            0,
            50,
            |engine, _token: CancellationToken, _t, _args| {
                async move { engine.heap.alloc_i32(1) }.boxed()
            },
        )
    });

    let mut gas = GasMeter::new(
        Some(10),
        GasCosts {
            eval_node: 1,
            native_call_base: 1,
            native_call_per_arg: 0,
            ..GasCosts::sensible_defaults()
        },
    );
    let err = match rexlang_engine::Evaluator::new_with_compiler(
        rexlang_engine::RuntimeEnv::new(engine.clone()),
        rexlang_engine::Compiler::new(engine.clone()),
    )
    .eval(expr.as_ref(), &mut gas)
    .await
    {
        Ok(_) => panic!("expected out of gas"),
        Err(e) => e,
    };
    assert!(matches!(err.as_engine_error(), EngineError::OutOfGas(..)));
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
    let mut library = Library::global();
    library.add_decls(program.decls.clone());
    engine.inject_library(library).unwrap();
    let mut gas = unlimited_gas();
    match rexlang_engine::Evaluator::new_with_compiler(
        rexlang_engine::RuntimeEnv::new(engine.clone()),
        rexlang_engine::Compiler::new(engine.clone()),
    )
    .eval(program.expr.as_ref(), &mut gas)
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
