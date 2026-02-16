use std::sync::Arc;

use rex::{Engine, EngineError, GasMeter, Parser, Token, Type};
use rex_proc_macro::Rex;

#[derive(Clone)]
struct HostState {
    user_id: String,
    is_admin: bool,
    roles: Vec<String>,
}

fn current_user_id(state: &HostState) -> Result<String, EngineError> {
    Ok(state.user_id.clone())
}

fn is_admin(state: &HostState) -> Result<bool, EngineError> {
    Ok(state.is_admin)
}

fn have_role(state: &HostState, role: String) -> Result<bool, EngineError> {
    Ok(state.roles.iter().any(|r| r == &role))
}

async fn have_role_async(state: HostState, role: String) -> Result<bool, EngineError> {
    Ok(state.roles.iter().any(|r| r == &role))
}

fn parse(code: &str) -> Arc<rex_ast::expr::Expr> {
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    parser.parse_program(&mut GasMeter::default()).unwrap().expr
}

fn unlimited_gas() -> GasMeter {
    GasMeter::default()
}

#[derive(Clone, Debug, PartialEq, Rex)]
struct EmbedRecord {
    n: i32,
}

#[tokio::test]
async fn injected_functions_can_read_shared_state_fields() {
    let mut engine: Engine<HostState> = Engine::with_prelude(HostState {
        user_id: "u-123".to_string(),
        is_admin: true,
        roles: vec!["admin".to_string(), "editor".to_string()],
    })
    .unwrap();

    engine.export("current_user_id", current_user_id).unwrap();
    engine.export("is_admin", is_admin).unwrap();
    engine.export("have_role", have_role).unwrap();

    let expr = parse("(current_user_id, is_admin, have_role \"admin\", have_role \"viewer\")");
    let mut gas = unlimited_gas();
    let (value, ty) = engine.eval(expr.as_ref(), &mut gas).await.unwrap();
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::con("string", 0),
            Type::con("bool", 0),
            Type::con("bool", 0),
            Type::con("bool", 0),
        ])
    );

    let items = engine.heap().pointer_as_tuple(&value).unwrap();
    assert_eq!(items.len(), 4);
    assert_eq!(engine.heap().pointer_as_string(&items[0]).unwrap(), "u-123");
    assert!(engine.heap().pointer_as_bool(&items[1]).unwrap());
    assert!(engine.heap().pointer_as_bool(&items[2]).unwrap());
    assert!(!engine.heap().pointer_as_bool(&items[3]).unwrap());
}

#[tokio::test]
async fn async_injected_functions_can_read_shared_state_fields() {
    let mut engine: Engine<HostState> = Engine::with_prelude(HostState {
        user_id: "u-456".to_string(),
        is_admin: false,
        roles: vec!["reader".to_string(), "editor".to_string()],
    })
    .unwrap();

    engine
        .export_async("have_role_async", |state: &HostState, role: String| {
            have_role_async(state.clone(), role)
        })
        .unwrap();

    let expr = parse("(have_role_async \"editor\", have_role_async \"admin\")");
    let mut gas = unlimited_gas();
    let (value, ty) = engine.eval(expr.as_ref(), &mut gas).await.unwrap();
    assert_eq!(
        ty,
        Type::tuple(vec![Type::con("bool", 0), Type::con("bool", 0)])
    );

    let items = engine.heap().pointer_as_tuple(&value).unwrap();
    assert_eq!(items.len(), 2);
    assert!(engine.heap().pointer_as_bool(&items[0]).unwrap());
    assert!(!engine.heap().pointer_as_bool(&items[1]).unwrap());
}

#[tokio::test]
async fn overloaded_exports_types_and_values() {
    let mut engine: Engine<()> = Engine::with_prelude(()).unwrap();

    EmbedRecord::inject_rex(&mut engine).unwrap();

    engine
        .export("over1", |_state: &(), x: i32| Ok(x + 1))
        .unwrap();
    engine
        .export("over1", |_state: &(), x: bool| {
            Ok(if x {
                "bool:true".to_string()
            } else {
                "bool:false".to_string()
            })
        })
        .unwrap();
    engine
        .export("over1", |_state: &(), rec: EmbedRecord| Ok(rec.n > 10))
        .unwrap();

    engine
        .export("over3", |_state: &(), a: i32, b: i32, c: i32| Ok(a + b + c))
        .unwrap();
    engine
        .export("over3", |_state: &(), a: String, b: String, c: String| {
            Ok(a.len() < b.len() + c.len())
        })
        .unwrap();
    engine
        .export(
            "over3",
            |_state: &(), a: EmbedRecord, b: EmbedRecord, c: EmbedRecord| {
                Ok(format!("records:{}:{}:{}", a.n, b.n, c.n))
            },
        )
        .unwrap();

    let expr = r#"
    (
        over1 41,
        over1 true,
        over1 (EmbedRecord { n = 9 }),
        over3 1 2 3,
        over3 "a" "bb" "ccc",
        over3 (EmbedRecord { n = 1 }) (EmbedRecord { n = 2 }) (EmbedRecord { n = 3 })
    )
    "#;

    let (_, inferred) = engine
        .infer_snippet(expr, &mut GasMeter::default())
        .unwrap();
    let expected = Type::tuple(vec![
        Type::con("i32", 0),
        Type::con("string", 0),
        Type::con("bool", 0),
        Type::con("i32", 0),
        Type::con("bool", 0),
        Type::con("string", 0),
    ]);
    assert_eq!(inferred, expected);

    let value = engine
        .eval(parse(expr).as_ref(), &mut GasMeter::default())
        .await;
    assert!(value.is_ok(), "evaluation failed: {value:?}");
    let (value, ty) = value.unwrap();
    assert_eq!(ty, expected);

    let items = engine.heap().pointer_as_tuple(&value).unwrap();
    assert_eq!(items.len(), 6);
    assert_eq!(engine.heap().pointer_as_i32(&items[0]).unwrap(), 42);
    assert_eq!(
        engine.heap().pointer_as_string(&items[1]).unwrap(),
        "bool:true"
    );
    assert!(!engine.heap().pointer_as_bool(&items[2]).unwrap());
    assert_eq!(engine.heap().pointer_as_i32(&items[3]).unwrap(), 6);
    assert!(engine.heap().pointer_as_bool(&items[4]).unwrap());
    assert_eq!(
        engine.heap().pointer_as_string(&items[5]).unwrap(),
        "records:1:2:3"
    );
}

#[tokio::test]
async fn overloaded_async_exports_types_and_values() {
    let mut engine: Engine<()> = Engine::with_prelude(()).unwrap();
    EmbedRecord::inject_rex(&mut engine).unwrap();

    engine
        .export_async("a1", |_state: &(), x: i32| async move { Ok(x + 1) })
        .unwrap();
    engine
        .export_async("a1", |_state: &(), x: bool| async move {
            Ok(if x {
                "bool:true".to_string()
            } else {
                "bool:false".to_string()
            })
        })
        .unwrap();
    engine
        .export_async("a1", |_state: &(), rec: EmbedRecord| async move {
            Ok(rec.n > 10)
        })
        .unwrap();

    engine
        .export_async("a3", |_state: &(), a: i32, b: i32, c: i32| async move {
            Ok(a + b + c)
        })
        .unwrap();
    engine
        .export_async(
            "a3",
            |_state: &(), a: String, b: String, c: String| async move {
                Ok(a.len() < b.len() + c.len())
            },
        )
        .unwrap();
    engine
        .export_async(
            "a3",
            |_state: &(), a: EmbedRecord, b: EmbedRecord, c: EmbedRecord| async move {
                Ok(format!("records:{}:{}:{}", a.n, b.n, c.n))
            },
        )
        .unwrap();

    let expr = r#"
    (
        a1 41,
        a1 true,
        a1 (EmbedRecord { n = 9 }),
        a3 1 2 3,
        a3 "a" "bb" "ccc",
        a3 (EmbedRecord { n = 1 }) (EmbedRecord { n = 2 }) (EmbedRecord { n = 3 })
    )
    "#;

    let (_, inferred) = engine
        .infer_snippet(expr, &mut GasMeter::default())
        .unwrap();
    let expected = Type::tuple(vec![
        Type::con("i32", 0),
        Type::con("string", 0),
        Type::con("bool", 0),
        Type::con("i32", 0),
        Type::con("bool", 0),
        Type::con("string", 0),
    ]);
    assert_eq!(inferred, expected);

    let value = engine
        .eval(parse(expr).as_ref(), &mut GasMeter::default())
        .await;
    assert!(value.is_ok(), "evaluation failed: {value:?}");
    let (value, ty) = value.unwrap();
    assert_eq!(ty, expected);

    let items = engine.heap().pointer_as_tuple(&value).unwrap();
    assert_eq!(items.len(), 6);
    assert_eq!(engine.heap().pointer_as_i32(&items[0]).unwrap(), 42);
    assert_eq!(
        engine.heap().pointer_as_string(&items[1]).unwrap(),
        "bool:true"
    );
    assert!(!engine.heap().pointer_as_bool(&items[2]).unwrap());
    assert_eq!(engine.heap().pointer_as_i32(&items[3]).unwrap(), 6);
    assert!(engine.heap().pointer_as_bool(&items[4]).unwrap());
    assert_eq!(
        engine.heap().pointer_as_string(&items[5]).unwrap(),
        "records:1:2:3"
    );
}
