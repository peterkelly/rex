use std::sync::Arc;

use rexlang_core::{
    BuiltinTypeId, Engine, EngineError, FromPointer, GasMeter, IntoPointer, Library, Parser,
    Pointer, Rex, RexDefault, Token, Type, TypeError, TypeKind, Value,
};
use rexlang_engine::virtual_export_name;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Rex)]
enum Side {
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq, Rex)]
enum Correctness {
    Right,
    Wrong,
}

#[derive(Clone, Debug, PartialEq, Rex)]
struct Label {
    text: String,
    side: Side,
}

fn render_label(label: Label) -> String {
    match label.side {
        Side::Left => format!("{:<12}", label.text),
        Side::Right => format!("{:>12}", label.text),
    }
}

#[tokio::test]
async fn library_render_label_with_library_scoped_adts_left_and_right() {
    let mut engine: Engine<()> = Engine::with_prelude(()).unwrap();
    engine.add_default_resolvers();

    let mut library = Library::new("sample");
    library.inject_rex_adt::<Side>(&mut engine).unwrap();
    library.inject_rex_adt::<Correctness>(&mut engine).unwrap();
    library.inject_rex_adt::<Label>(&mut engine).unwrap();
    library
        .export("render_label", |_: &(), label: Label| {
            Ok::<String, EngineError>(render_label(label))
        })
        .unwrap();
    engine.inject_library(library).unwrap();

    let mut gas = unlimited_gas();
    let (value, ty) = engine
        .eval_snippet(
            r#"
            import sample (Label, Left, Right, Wrong, render_label)
            import sample as Sample
            (
                render_label (Label { text = "left", side = Left }),
                render_label (Label { text = "right", side = (Right is Sample.Side) }),
                (Right is Sample.Correctness),
                (Wrong is Sample.Correctness)
            )
            "#,
            &mut gas,
        )
        .await
        .unwrap();

    // `Side` and `Correctness` both provide a `Right` constructor in the same library.
    // This ensures Rex keeps them distinct via explicit type ascription (`is Side` vs `is Sample.Correctness`).
    let correctness_ty = Type::con(virtual_export_name("sample", "Correctness"), 0);
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::String),
            Type::builtin(BuiltinTypeId::String),
            correctness_ty.clone(),
            correctness_ty,
        ])
    );
    let items = engine.heap.pointer_as_tuple(&value).unwrap();
    assert_eq!(items.len(), 4);
    assert_eq!(
        engine.heap.pointer_as_string(&items[0]).unwrap(),
        format!("{:<12}", "left")
    );
    assert_eq!(
        engine.heap.pointer_as_string(&items[1]).unwrap(),
        format!("{:>12}", "right")
    );
    let right = engine.heap.get(&items[2]).unwrap();
    match right.as_ref() {
        Value::Adt(tag, args) => {
            assert!(tag.as_ref().ends_with(".Right"));
            assert!(args.is_empty());
        }
        _ => panic!("expected ADT value for Correctness.Right"),
    }
    let wrong = engine.heap.get(&items[3]).unwrap();
    match wrong.as_ref() {
        Value::Adt(tag, args) => {
            assert!(tag.as_ref().ends_with(".Wrong"));
            assert!(args.is_empty());
        }
        _ => panic!("expected ADT value for Correctness.Wrong"),
    }
}

#[tokio::test]
async fn match_ascribed_library_type_with_overlapping_constructor_is_ambiguous_regression() {
    // Regression guard: when two library ADTs expose overlapping constructor names
    // (e.g. both have `Right`), `match` arms that use the bare constructor after an
    // `is Sample.Correctness` ascription currently remain ambiguous. This test ensures
    // we keep surfacing that ambiguity instead of silently picking one constructor.
    let mut engine: Engine<()> = Engine::with_prelude(()).unwrap();
    engine.add_default_resolvers();

    let mut library = Library::new("sample");
    library.inject_rex_adt::<Side>(&mut engine).unwrap();
    library.inject_rex_adt::<Correctness>(&mut engine).unwrap();
    engine.inject_library(library).unwrap();

    let mut gas = unlimited_gas();
    let err = engine
        .eval_snippet(
            r#"
            import sample (Right, Wrong)
            import sample as Sample
            let x = (Right is Sample.Correctness) in
            match (x is Sample.Correctness)
              when Right -> true
              when Wrong -> false
            "#,
            &mut gas,
        )
        .await
        .expect_err("expected ambiguity error for overlapping constructor in match pattern");

    match err {
        EngineError::Type(mut e) => {
            while let TypeError::Spanned { error, .. } = e {
                e = *error;
            }
            match e {
                TypeError::AmbiguousOverload(name) => {
                    assert!(name.as_ref().ends_with(".Right"));
                }
                other => panic!("expected ambiguous overload error, got {other:?}"),
            }
        }
        other => panic!("expected type error, got {other:?}"),
    }
}

#[derive(Clone, Debug, PartialEq, Rex)]
struct Entity1 {
    account_id: Uuid,
    project_id: Uuid,
    name: String,
    description: Option<String>,
    tags: Option<Vec<String>>,
    numbers: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Rex)]
struct Entity2 {
    account_id: Uuid,
    project_id: Uuid,
    name: String,
    description: Option<String>,
    tags: Option<Vec<String>>,
    numbers: Vec<u32>,
}

impl Entity2 {
    fn rex_new(state: &HostState, name: String, numbers: Vec<u32>) -> Result<Entity2, EngineError> {
        Ok(Entity2 {
            account_id: state.account_id,
            project_id: state.project_id,
            name,
            description: None,
            tags: None,
            numbers,
        })
    }
}

impl RexDefault<HostState> for Entity1 {
    fn rex_default(engine: &Engine<HostState>) -> Result<Pointer, EngineError> {
        let entity = Entity1 {
            account_id: engine.state.account_id,
            project_id: engine.state.project_id,
            name: "".to_string(),
            description: None,
            tags: None,
            numbers: vec![],
        };
        entity.into_pointer(&engine.heap)
    }
}

#[derive(Clone)]
struct HostState {
    account_id: Uuid,
    project_id: Uuid,
    is_admin: bool,
    roles: Vec<String>,
}

fn current_account_id(state: &HostState) -> Result<Uuid, EngineError> {
    Ok(state.account_id)
}

fn current_project_id(state: &HostState) -> Result<Uuid, EngineError> {
    Ok(state.project_id)
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

fn parse(code: &str) -> Arc<rexlang_ast::expr::Expr> {
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    parser.parse_program(&mut GasMeter::default()).unwrap().expr
}

fn unlimited_gas() -> GasMeter {
    GasMeter::default()
}

fn is_i32_or_var(ty: &Type) -> bool {
    matches!(ty.as_ref(), TypeKind::Con(tc) if tc.name.as_ref() == "i32")
        || matches!(ty.as_ref(), TypeKind::Var(_))
}

fn assert_overload_tuple_type_shape(ty: &Type) {
    let TypeKind::Tuple(items) = ty.as_ref() else {
        panic!("expected tuple type, got {ty}");
    };
    assert_eq!(items.len(), 6);
    assert!(
        is_i32_or_var(&items[0]),
        "expected i32/var at index 0, got {}",
        items[0]
    );
    assert_eq!(items[1], Type::builtin(BuiltinTypeId::String));
    assert_eq!(items[2], Type::builtin(BuiltinTypeId::Bool));
    assert!(
        is_i32_or_var(&items[3]),
        "expected i32/var at index 3, got {}",
        items[3]
    );
    assert_eq!(items[4], Type::builtin(BuiltinTypeId::Bool));
    assert_eq!(items[5], Type::builtin(BuiltinTypeId::String));
}

#[derive(Clone, Debug, PartialEq, Rex)]
struct EmbedRecord {
    n: i32,
}

#[tokio::test]
async fn injected_functions_can_read_shared_state_fields() {
    let account_id = uuid::uuid!("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
    let project_id = uuid::uuid!("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
    let mut engine: Engine<HostState> = Engine::with_prelude(HostState {
        account_id,
        project_id,
        is_admin: true,
        roles: vec!["admin".to_string(), "editor".to_string()],
    })
    .unwrap();

    engine
        .export("current_account_id", current_account_id)
        .unwrap();
    engine
        .export("current_project_id", current_project_id)
        .unwrap();
    engine.export("is_admin", is_admin).unwrap();
    engine.export("have_role", have_role).unwrap();

    let expr = parse(
        "(current_account_id, current_project_id, is_admin, have_role \"admin\", have_role \"viewer\")",
    );
    let mut gas = unlimited_gas();
    let (value, ty) = engine.eval(expr.as_ref(), &mut gas).await.unwrap();
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::Uuid),
            Type::builtin(BuiltinTypeId::Uuid),
            Type::builtin(BuiltinTypeId::Bool),
            Type::builtin(BuiltinTypeId::Bool),
            Type::builtin(BuiltinTypeId::Bool),
        ])
    );

    let items = engine.heap.pointer_as_tuple(&value).unwrap();
    assert_eq!(items.len(), 5);
    assert_eq!(engine.heap.pointer_as_uuid(&items[0]).unwrap(), account_id);
    assert_eq!(engine.heap.pointer_as_uuid(&items[1]).unwrap(), project_id);
    assert!(engine.heap.pointer_as_bool(&items[2]).unwrap());
    assert!(engine.heap.pointer_as_bool(&items[3]).unwrap());
    assert!(!engine.heap.pointer_as_bool(&items[4]).unwrap());
}

#[tokio::test]
async fn derived_rex_default_can_read_host_state() {
    let account_id = uuid::uuid!("11111111-1111-4111-8111-111111111111");
    let project_id = uuid::uuid!("22222222-2222-4222-8222-222222222222");
    let mut engine: Engine<HostState> = Engine::with_prelude(HostState {
        account_id,
        project_id,
        is_admin: true,
        roles: vec!["admin".to_string()],
    })
    .unwrap();

    Entity1::inject_rex_with_default(&mut engine).unwrap();

    let expr = parse("let e: Entity1 = default in e");
    let mut gas = unlimited_gas();
    let (value, ty) = engine.eval(expr.as_ref(), &mut gas).await.unwrap();
    assert_eq!(ty, Type::con("Entity1", 0));

    let decoded = Entity1::from_pointer(&engine.heap, &value).unwrap();
    assert_eq!(
        decoded,
        Entity1 {
            account_id,
            project_id,
            name: String::new(),
            description: None,
            tags: None,
            numbers: vec![],
        }
    );
}

#[tokio::test]
async fn derived_rex_default_record_update_can_override_fields() {
    let account_id = uuid::uuid!("33333333-3333-4333-8333-333333333333");
    let project_id = uuid::uuid!("44444444-4444-4444-8444-444444444444");
    let mut engine: Engine<HostState> = Engine::with_prelude(HostState {
        account_id,
        project_id,
        is_admin: false,
        roles: vec!["reader".to_string()],
    })
    .unwrap();

    Entity1::inject_rex_with_default(&mut engine).unwrap();

    let expr = parse(
        r#"let e: Entity1 = { default with { name = "sample", tags = Some ["x", "y"], numbers = [7, 11] } } in e"#,
    );
    let mut gas = unlimited_gas();
    let (value, ty) = engine.eval(expr.as_ref(), &mut gas).await.unwrap();
    assert_eq!(ty, Type::con("Entity1", 0));

    let decoded = Entity1::from_pointer(&engine.heap, &value).unwrap();
    assert_eq!(
        decoded,
        Entity1 {
            account_id,
            project_id,
            name: "sample".to_string(),
            description: None,
            tags: Some(vec!["x".to_string(), "y".to_string()]),
            numbers: vec![7, 11],
        }
    );
}

#[tokio::test]
async fn entity2_constructor_defaults_from_host_state_with_required_fields() {
    let account_id = uuid::uuid!("55555555-5555-4555-8555-555555555555");
    let project_id = uuid::uuid!("66666666-6666-4666-8666-666666666666");
    let mut engine: Engine<HostState> = Engine::with_prelude(HostState {
        account_id,
        project_id,
        is_admin: false,
        roles: vec!["reader".to_string()],
    })
    .unwrap();

    Entity2::inject_rex_with_constructor(&mut engine, Entity2::rex_new).unwrap();

    let expr = parse(r#"Entity2 "sample" [7, 11]"#);
    let mut gas = unlimited_gas();
    let (value, ty) = engine.eval(expr.as_ref(), &mut gas).await.unwrap();
    assert_eq!(ty, Type::con("Entity2", 0));

    let decoded = Entity2::from_pointer(&engine.heap, &value).unwrap();
    assert_eq!(
        decoded,
        Entity2 {
            account_id,
            project_id,
            name: "sample".to_string(),
            description: None,
            tags: None,
            numbers: vec![7, 11],
        }
    );
}

#[tokio::test]
async fn entity2_constructor_result_can_be_record_updated() {
    let account_id = uuid::uuid!("77777777-7777-4777-8777-777777777777");
    let project_id = uuid::uuid!("88888888-8888-4888-8888-888888888888");
    let mut engine: Engine<HostState> = Engine::with_prelude(HostState {
        account_id,
        project_id,
        is_admin: true,
        roles: vec!["admin".to_string()],
    })
    .unwrap();

    Entity2::inject_rex_with_constructor(&mut engine, Entity2::rex_new).unwrap();

    let expr = parse(
        r#"{
            (Entity2 "sample" [7, 11])
            with {
                description = Some "desc",
                tags = Some ["x", "y"]
            }
        }"#,
    );
    let mut gas = unlimited_gas();
    let (value, ty) = engine.eval(expr.as_ref(), &mut gas).await.unwrap();
    assert_eq!(ty, Type::con("Entity2", 0));

    let decoded = Entity2::from_pointer(&engine.heap, &value).unwrap();
    assert_eq!(
        decoded,
        Entity2 {
            account_id,
            project_id,
            name: "sample".to_string(),
            description: Some("desc".to_string()),
            tags: Some(vec!["x".to_string(), "y".to_string()]),
            numbers: vec![7, 11],
        }
    );
}

#[tokio::test]
async fn async_injected_functions_can_read_shared_state_fields() {
    let mut engine: Engine<HostState> = Engine::with_prelude(HostState {
        account_id: uuid::uuid!("cccccccc-cccc-4ccc-8ccc-cccccccccccc"),
        project_id: uuid::uuid!("dddddddd-dddd-4ddd-8ddd-dddddddddddd"),
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
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::Bool),
            Type::builtin(BuiltinTypeId::Bool)
        ])
    );

    let items = engine.heap.pointer_as_tuple(&value).unwrap();
    assert_eq!(items.len(), 2);
    assert!(engine.heap.pointer_as_bool(&items[0]).unwrap());
    assert!(!engine.heap.pointer_as_bool(&items[1]).unwrap());
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
    assert_overload_tuple_type_shape(&inferred);

    let value = engine
        .eval(parse(expr).as_ref(), &mut GasMeter::default())
        .await;
    assert!(value.is_ok(), "evaluation failed: {value:?}");
    let (value, ty) = value.unwrap();
    assert_overload_tuple_type_shape(&ty);

    let items = engine.heap.pointer_as_tuple(&value).unwrap();
    assert_eq!(items.len(), 6);
    assert_eq!(engine.heap.pointer_as_i32(&items[0]).unwrap(), 42);
    assert_eq!(
        engine.heap.pointer_as_string(&items[1]).unwrap(),
        "bool:true"
    );
    assert!(!engine.heap.pointer_as_bool(&items[2]).unwrap());
    assert_eq!(engine.heap.pointer_as_i32(&items[3]).unwrap(), 6);
    assert!(engine.heap.pointer_as_bool(&items[4]).unwrap());
    assert_eq!(
        engine.heap.pointer_as_string(&items[5]).unwrap(),
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
    assert_overload_tuple_type_shape(&inferred);

    let value = engine
        .eval(parse(expr).as_ref(), &mut GasMeter::default())
        .await;
    assert!(value.is_ok(), "evaluation failed: {value:?}");
    let (value, ty) = value.unwrap();
    assert_overload_tuple_type_shape(&ty);

    let items = engine.heap.pointer_as_tuple(&value).unwrap();
    assert_eq!(items.len(), 6);
    assert_eq!(engine.heap.pointer_as_i32(&items[0]).unwrap(), 42);
    assert_eq!(
        engine.heap.pointer_as_string(&items[1]).unwrap(),
        "bool:true"
    );
    assert!(!engine.heap.pointer_as_bool(&items[2]).unwrap());
    assert_eq!(engine.heap.pointer_as_i32(&items[3]).unwrap(), 6);
    assert!(engine.heap.pointer_as_bool(&items[4]).unwrap());
    assert_eq!(
        engine.heap.pointer_as_string(&items[5]).unwrap(),
        "records:1:2:3"
    );
}
