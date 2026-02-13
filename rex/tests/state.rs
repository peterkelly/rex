use std::sync::Arc;

use rex::{Engine, GasCosts, GasMeter, Parser, Token};

#[derive(Clone)]
struct HostState {
    user_id: String,
    is_admin: bool,
    roles: Vec<String>,
}

fn current_user_id(state: &HostState) -> String {
    state.user_id.clone()
}

fn is_admin(state: &HostState) -> bool {
    state.is_admin
}

fn have_role(state: &HostState, role: String) -> bool {
    state.roles.iter().any(|r| r == &role)
}

async fn have_role_async(state: HostState, role: String) -> bool {
    state.roles.iter().any(|r| r == &role)
}

fn parse(code: &str) -> Arc<rex_ast::expr::Expr> {
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    parser.parse_program().unwrap().expr
}

fn unlimited_gas() -> GasMeter {
    GasMeter::unlimited(GasCosts::sensible_defaults())
}

#[tokio::test]
async fn injected_functions_can_read_shared_state_fields() {
    let mut engine: Engine<HostState> = Engine::with_prelude(HostState {
        user_id: "u-123".to_string(),
        is_admin: true,
        roles: vec!["admin".to_string(), "editor".to_string()],
    })
    .unwrap();

    engine
        .inject_fn0("current_user_id", current_user_id)
        .unwrap();
    engine.inject_fn0("is_admin", is_admin).unwrap();
    engine.inject_fn1("have_role", have_role).unwrap();

    let expr = parse("(current_user_id, is_admin, have_role \"admin\", have_role \"viewer\")");
    let mut gas = unlimited_gas();
    let value = engine.eval(expr.as_ref(), &mut gas).await.unwrap();

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
        .inject_async_fn1("have_role_async", |state: &HostState, role: String| {
            have_role_async(state.clone(), role)
        })
        .unwrap();

    let expr = parse("(have_role_async \"editor\", have_role_async \"admin\")");
    let mut gas = unlimited_gas();
    let value = engine.eval(expr.as_ref(), &mut gas).await.unwrap();

    let items = engine.heap().pointer_as_tuple(&value).unwrap();
    assert_eq!(items.len(), 2);
    assert!(engine.heap().pointer_as_bool(&items[0]).unwrap());
    assert!(!engine.heap().pointer_as_bool(&items[1]).unwrap());
}
