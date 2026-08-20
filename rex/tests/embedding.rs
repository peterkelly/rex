mod common;

use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use blake3::Hash;
use rex::{
    Rex,
    ast::Symbol,
    engine::{
        Builder, CompileOptions, Context, EngineError, FromRex, ImportRequest, Importer, Module,
        ModuleId, ResolvedModule, ResolvedModuleContent, RexDefault, Value, virtual_export_name,
    },
    parser::parse as parse_rex,
    typesystem::{BuiltinTypeId, Scheme, Type, TypeError, TypeKind},
};
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

#[derive(Clone)]
struct LazySampleImporter {
    calls: Arc<AtomicUsize>,
}

impl Importer for LazySampleImporter {
    fn import<'a>(
        &'a self,
        req: ImportRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ResolvedModule>, EngineError>> + Send + 'a>>
    {
        Box::pin(async move {
            if req.module_id != ModuleId::parse("sample").unwrap() {
                return Ok(None);
            }
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut module = Module::new("sample", None);
            module.add_rex_adt::<Side>().unwrap();
            module.add_rex_adt::<Correctness>().unwrap();
            module.add_rex_adt::<Label>().unwrap();
            module
                .export("render_label", |_: (), label: Label| {
                    Ok::<String, EngineError>(render_label(label))
                })
                .unwrap();
            Ok(Some(ResolvedModule {
                id: req.module_id,
                content: ResolvedModuleContent::module(module),
            }))
        })
    }
}

#[tokio::test]
async fn module_render_label_with_module_scoped_adts_left_and_right() {
    let mut builder: Builder<()> = Builder::with_prelude(()).unwrap();

    let mut module = Module::new("sample", None);
    module.add_rex_adt::<Side>().unwrap();
    module.add_rex_adt::<Correctness>().unwrap();
    module.add_rex_adt::<Label>().unwrap();
    module
        .export("render_label", |_: (), label: Label| {
            Ok::<String, EngineError>(render_label(label))
        })
        .unwrap();
    builder.inject_module(module).unwrap();
    let compiler = builder.build_compiler();
    let parsed = parse_rex(
        r#"
            import sample (Label, Left, Right, Wrong, render_label);
            import sample as Sample;
            (
                render_label (Label { text = "left", side = Left }),
                render_label (Label { text = "right", side = (Right is Sample.Side) }),
                (Right is Sample.Correctness),
                (Wrong is Sample.Correctness)
            )
            "#,
    )
    .unwrap();
    let (program, evaluator) = compiler
        .compile_program(&parsed, CompileOptions::for_module("test.main").unwrap())
        .await
        .unwrap();
    let ty = program.result_type().clone();
    let value = evaluator.run(program, Default::default()).await.unwrap();

    // `Side` and `Correctness` both provide a `Right` constructor in the same module.
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
    let items = common::tuple_items(&value);
    assert_eq!(items.len(), 4);
    assert_eq!(
        items[0].to_rust::<String>().unwrap(),
        format!("{:<12}", "left")
    );
    assert_eq!(
        items[1].to_rust::<String>().unwrap(),
        format!("{:>12}", "right")
    );
    match &items[2] {
        Value::Adt(tag, args) => {
            assert_eq!(tag.as_ref(), "Right");
            assert!(args.is_empty());
        }
        _ => panic!("expected ADT value for Correctness.Right"),
    }
    match &items[3] {
        Value::Adt(tag, args) => {
            assert_eq!(tag.as_ref(), "Wrong");
            assert!(args.is_empty());
        }
        _ => panic!("expected ADT value for Correctness.Wrong"),
    }
}

#[tokio::test]
async fn importer_rust_module_preserves_module_scoped_adts() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut builder: Builder<()> = Builder::with_prelude(()).unwrap();
    builder.add_importer(Arc::new(LazySampleImporter {
        calls: Arc::clone(&calls),
    }));
    let compiler = builder.build_compiler();
    let parsed = parse_rex(
        r#"
            import sample (Label, Left, Right, Wrong, render_label);
            import sample as Sample;
            (
                render_label (Label { text = "left", side = Left }),
                render_label (Label { text = "right", side = (Right is Sample.Side) }),
                (Right is Sample.Correctness),
                (Wrong is Sample.Correctness)
            )
            "#,
    )
    .unwrap();
    let (program, evaluator) = compiler
        .compile_program(&parsed, CompileOptions::for_module("test.main").unwrap())
        .await
        .unwrap();
    let ty = program.result_type().clone();
    let value = evaluator.run(program, Default::default()).await.unwrap();

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
    let items = common::tuple_items(&value);
    assert_eq!(items.len(), 4);
    assert_eq!(items[0].to_rust::<String>().unwrap(), "left        ");
    assert_eq!(items[1].to_rust::<String>().unwrap(), "       right");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn module_inject_rex_adt_registers_acyclic_dependency_closure() {
    let mut builder: Builder<()> = Builder::with_prelude(()).unwrap();

    let mut module = Module::new("sample", None);
    module.add_rex_adt::<Label>().unwrap();
    module
        .export("render_label", |_: (), label: Label| {
            Ok::<String, EngineError>(render_label(label))
        })
        .unwrap();
    builder.inject_module(module).unwrap();
    let compiler = builder.build_compiler();
    let parsed = parse_rex(
        r#"
            import sample (Label, Left, render_label);
            render_label (Label { text = "left", side = Left })
        "#,
    )
    .unwrap();
    let (program, evaluator) = compiler
        .compile_program(&parsed, CompileOptions::for_module("test.main").unwrap())
        .await
        .unwrap();
    let ty = program.result_type().clone();
    let value = evaluator.run(program, Default::default()).await.unwrap();

    assert_eq!(ty, Type::builtin(BuiltinTypeId::String));
    assert_eq!(
        value.to_rust::<String>().unwrap(),
        format!("{:<12}", "left")
    );
}

#[tokio::test]
async fn match_ascribed_module_type_with_overlapping_constructor_is_ambiguous_regression() {
    // Regression guard: when two module ADTs expose overlapping constructor names
    // (e.g. both have `Right`), `match` arms that use the bare constructor after an
    // `is Sample.Correctness` ascription currently remain ambiguous. This test ensures
    // we keep surfacing that ambiguity instead of silently picking one constructor.
    let mut builder: Builder<()> = Builder::with_prelude(()).unwrap();

    let mut module = Module::new("sample", None);
    module.add_rex_adt::<Side>().unwrap();
    module.add_rex_adt::<Correctness>().unwrap();
    builder.inject_module(module).unwrap();
    let compiler = builder.build_compiler();
    let parsed = parse_rex(
        r#"
            import sample (Right, Wrong);
            import sample as Sample;
            let x = (Right is Sample.Correctness) in
            match (x is Sample.Correctness) with {
              case Right -> true;
              case Wrong -> false;
            }
            "#,
    )
    .unwrap();
    let err = match compiler
        .compile_program(&parsed, CompileOptions::for_module("test.main").unwrap())
        .await
    {
        Ok(_) => panic!("expected ambiguity error for overlapping constructor in match pattern"),
        Err(err) => err,
    };

    match err {
        EngineError::Type(e) => match common::strip_type_span(e) {
            TypeError::AmbiguousOverload(name) => {
                assert!(name.as_ref().ends_with(".Right"));
            }
            other => panic!("expected ambiguous overload error, got {other:?}"),
        },
        other => panic!("expected type error, got {other:?}"),
    }
}

#[tokio::test]
async fn hash_values_cross_native_runtime_boundaries() {
    let expected = blake3::hash(b"rex native hash");
    let mut builder = Builder::with_prelude(()).unwrap();
    common::inject_globals(&mut builder, |module| {
        module.export_value("expected_hash", expected)?;
        module.export("identity_hash", |_: (), value: Hash| {
            Ok::<Hash, EngineError>(value)
        })
    })
    .unwrap();

    let (_, value, ty) = common::eval_source(
        builder,
        "(identity_hash expected_hash, expected_hash == identity_hash expected_hash, show expected_hash)",
    )
    .await
    .unwrap();
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::Hash),
            Type::builtin(BuiltinTypeId::Bool),
            Type::builtin(BuiltinTypeId::String),
        ])
    );

    let items = common::tuple_items(&value);
    assert_eq!(items[0].to_rust::<Hash>().unwrap(), expected);
    assert!(items[1].to_rust::<bool>().unwrap());
    assert_eq!(
        items[2].to_rust::<String>().unwrap(),
        expected.to_hex().to_string()
    );
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

#[derive(Clone, Debug, Default, PartialEq, Rex)]
struct RustDefaultEntity {
    enabled: bool,
    count: i32,
}

impl Entity2 {
    fn rex_new(state: HostState, name: String, numbers: Vec<u32>) -> Result<Entity2, EngineError> {
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
    fn rex_default(engine: Context<HostState>) -> Result<Self, EngineError> {
        Ok(Entity1 {
            account_id: engine.state().account_id,
            project_id: engine.state().project_id,
            name: "".to_string(),
            description: None,
            tags: None,
            numbers: vec![],
        })
    }
}

#[derive(Clone)]
struct HostState {
    account_id: Uuid,
    project_id: Uuid,
    is_admin: bool,
    roles: Vec<String>,
}

pub trait OffsetState {
    fn offset(&self) -> i32;
}

#[derive(Clone)]
struct FirstOffsetState(i32);

impl OffsetState for FirstOffsetState {
    fn offset(&self) -> i32 {
        self.0
    }
}

#[derive(Clone)]
struct SecondOffsetState(i32);

impl OffsetState for SecondOffsetState {
    fn offset(&self) -> i32 {
        self.0
    }
}

#[rex::module(name = "host.generic_state")]
mod generic_state_exports {
    use super::OffsetState;
    use rex::engine::EngineError;

    #[rex::export]
    pub fn add_offset<T>(state: T, value: i32) -> Result<i32, EngineError>
    where
        T: OffsetState,
    {
        Ok(state.offset() + value)
    }

    #[rex::export]
    pub async fn add_offset_async<T>(state: T, value: i32) -> Result<i32, EngineError>
    where
        T: OffsetState,
    {
        Ok(state.offset() + value)
    }
}

fn current_account_id(state: HostState) -> Result<Uuid, EngineError> {
    Ok(state.account_id)
}

fn current_project_id(state: HostState) -> Result<Uuid, EngineError> {
    Ok(state.project_id)
}

fn is_admin(state: HostState) -> Result<bool, EngineError> {
    Ok(state.is_admin)
}

fn have_role(state: HostState, role: String) -> Result<bool, EngineError> {
    Ok(state.roles.iter().any(|r| r == &role))
}

async fn have_role_async(state: HostState, role: String) -> Result<bool, EngineError> {
    Ok(state.roles.iter().any(|r| r == &role))
}

fn assert_overload_tuple_type_shape(ty: &Type) {
    let TypeKind::Tuple(items) = ty.as_ref() else {
        panic!("expected tuple type, got {ty}");
    };
    assert_eq!(items.len(), 6);
    assert!(
        common::is_i32_or_var(&items[0]),
        "expected i32/var at index 0, got {}",
        items[0]
    );
    assert_eq!(items[1], Type::builtin(BuiltinTypeId::String));
    assert_eq!(items[2], Type::builtin(BuiltinTypeId::Bool));
    assert!(
        common::is_i32_or_var(&items[3]),
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
    let mut builder: Builder<HostState> = Builder::with_prelude(HostState {
        account_id,
        project_id,
        is_admin: true,
        roles: vec!["admin".to_string(), "editor".to_string()],
    })
    .unwrap();

    common::inject_globals(&mut builder, |module| {
        module.export("current_account_id", current_account_id)?;
        module.export("current_project_id", current_project_id)?;
        module.export("is_admin", is_admin)?;
        module.export("have_role", have_role)?;
        Ok(())
    })
    .unwrap();

    let (_heap, value, ty) = common::eval_source(
        builder,
        "(current_account_id, current_project_id, is_admin, have_role \"admin\", have_role \"viewer\")",
    )
    .await
    .unwrap();
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

    let items = common::tuple_items(&value);
    assert_eq!(items.len(), 5);
    assert_eq!(items[0].to_rust::<Uuid>().unwrap(), account_id);
    assert_eq!(items[1].to_rust::<Uuid>().unwrap(), project_id);
    assert!(items[2].to_rust::<bool>().unwrap());
    assert!(items[3].to_rust::<bool>().unwrap());
    assert!(!items[4].to_rust::<bool>().unwrap());
}

#[tokio::test]
async fn rust_default_types_get_rex_default_instance() {
    let mut builder = Builder::with_prelude(()).unwrap();
    RustDefaultEntity::inject_rex_with_default(&mut builder).unwrap();

    let (_heap, value, ty) =
        common::eval_source(builder, "let e: RustDefaultEntity = default in e")
            .await
            .unwrap();
    assert_eq!(ty, Type::con("RustDefaultEntity", 0));
    assert_eq!(
        RustDefaultEntity::from_rex(value).unwrap(),
        RustDefaultEntity::default()
    );
}

#[tokio::test]
async fn derived_rex_default_can_read_host_state() {
    let account_id = uuid::uuid!("11111111-1111-4111-8111-111111111111");
    let project_id = uuid::uuid!("22222222-2222-4222-8222-222222222222");
    let mut builder: Builder<HostState> = Builder::with_prelude(HostState {
        account_id,
        project_id,
        is_admin: true,
        roles: vec!["admin".to_string()],
    })
    .unwrap();

    Entity1::inject_rex_with_default(&mut builder).unwrap();

    let (_heap, value, ty) = common::eval_source(builder, "let e: Entity1 = default in e")
        .await
        .unwrap();
    assert_eq!(ty, Type::con("Entity1", 0));

    let decoded = Entity1::from_rex(value).unwrap();
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
async fn derived_rex_default_partial_constructor_can_override_fields() {
    let account_id = uuid::uuid!("33333333-3333-4333-8333-333333333333");
    let project_id = uuid::uuid!("44444444-4444-4444-8444-444444444444");
    let mut builder: Builder<HostState> = Builder::with_prelude(HostState {
        account_id,
        project_id,
        is_admin: false,
        roles: vec!["reader".to_string()],
    })
    .unwrap();

    Entity1::inject_rex_with_default(&mut builder).unwrap();

    let (_heap, value, ty) = common::eval_source(
        builder,
        r#"Entity1 { name = "sample", tags = Some ["x", "y"], numbers = [7, 11] }"#,
    )
    .await
    .unwrap();
    assert_eq!(ty, Type::con("Entity1", 0));

    let decoded = Entity1::from_rex(value).unwrap();
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
    let mut builder: Builder<HostState> = Builder::with_prelude(HostState {
        account_id,
        project_id,
        is_admin: false,
        roles: vec!["reader".to_string()],
    })
    .unwrap();

    Entity2::inject_rex_with_constructor(&mut builder, Entity2::rex_new).unwrap();

    let (_heap, value, ty) = common::eval_source(builder, r#"Entity2 "sample" [7, 11]"#)
        .await
        .unwrap();
    assert_eq!(ty, Type::con("Entity2", 0));

    let decoded = Entity2::from_rex(value).unwrap();
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
    let mut builder: Builder<HostState> = Builder::with_prelude(HostState {
        account_id,
        project_id,
        is_admin: true,
        roles: vec!["admin".to_string()],
    })
    .unwrap();

    Entity2::inject_rex_with_constructor(&mut builder, Entity2::rex_new).unwrap();

    let (_heap, value, ty) = common::eval_source(
        builder,
        r#"{
            (Entity2 "sample" [7, 11])
            with {
                description = Some "desc",
                tags = Some ["x", "y"]
            }
        }"#,
    )
    .await
    .unwrap();
    assert_eq!(ty, Type::con("Entity2", 0));

    let decoded = Entity2::from_rex(value).unwrap();
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
    let mut builder: Builder<HostState> = Builder::with_prelude(HostState {
        account_id: uuid::uuid!("cccccccc-cccc-4ccc-8ccc-cccccccccccc"),
        project_id: uuid::uuid!("dddddddd-dddd-4ddd-8ddd-dddddddddddd"),
        is_admin: false,
        roles: vec!["reader".to_string(), "editor".to_string()],
    })
    .unwrap();

    common::inject_globals(&mut builder, |module| {
        module.export_async("have_role_async", have_role_async)
    })
    .unwrap();

    let (_heap, value, ty) = common::eval_source(
        builder,
        "(have_role_async \"editor\", have_role_async \"admin\")",
    )
    .await
    .unwrap();
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::Bool),
            Type::builtin(BuiltinTypeId::Bool)
        ])
    );

    let items = common::tuple_items(&value);
    assert_eq!(items.len(), 2);
    assert!(items[0].to_rust::<bool>().unwrap());
    assert!(!items[1].to_rust::<bool>().unwrap());
}

#[tokio::test]
async fn export_and_module_macros_support_generic_state_types() -> Result<(), EngineError> {
    let export = generic_state_exports::add_offset_rex_export::<FirstOffsetState>()?;
    assert_eq!(export.name, "add_offset");

    let first_module = generic_state_exports::rex_module::<FirstOffsetState>()?;
    assert_eq!(first_module.exports().len(), 2);

    let mut builder = Builder::with_prelude(FirstOffsetState(10))?;
    builder.inject_module(first_module)?;
    let (_heap, value, _typ) = common::eval_source(
        builder,
        r#"
        import host.generic_state as Generic;
        (Generic.add_offset 5, Generic.add_offset_async 7)
        "#,
    )
    .await?;

    assert_eq!(value, Value::Tuple(vec![Value::I32(15), Value::I32(17)]));

    let second_module = generic_state_exports::rex_module::<SecondOffsetState>()?;
    let mut builder = Builder::with_prelude(SecondOffsetState(20))?;
    builder.inject_module(second_module)?;
    let (_heap, value, _typ) = common::eval_source(
        builder,
        r#"
        import host.generic_state (add_offset, add_offset_async);
        (add_offset 3, add_offset_async 4)
        "#,
    )
    .await?;

    assert_eq!(value, Value::Tuple(vec![Value::I32(23), Value::I32(24)]));
    Ok(())
}

#[tokio::test]
async fn generic_export_can_repeat_a_value_into_a_list() {
    let mut builder: Builder<()> = Builder::with_prelude(()).unwrap();

    // This demonstrates how to write a generic function by declaring a fresh
    // type variable `T` and using it in the exported Rex type scheme.
    let t_var = builder
        .type_system_mut()
        .fresh_type_var(Some(Symbol::intern("T")));
    let t = Type::var(t_var.clone());
    let scheme = Scheme::new(
        vec![t_var],
        vec![],
        Type::fun(
            t.clone(),
            Type::fun(Type::builtin(BuiltinTypeId::I32), Type::list(t)),
        ),
    );
    common::inject_globals(&mut builder, |module| {
        module.export_native("repeat_value", scheme, 2, |_engine, _, args| {
            let value = args[0].clone();
            let len = args[1].to_rust::<i32>()?;
            let copies = (0..len.max(0)).map(|_| value.clone()).collect();
            Ok(common::list_from_values(copies))
        })
    })
    .unwrap();

    let (_heap, value, ty) =
        common::eval_source(builder, r#"(repeat_value "rex" 3, repeat_value true 2)"#)
            .await
            .unwrap();
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::list(Type::builtin(BuiltinTypeId::String)),
            Type::list(Type::builtin(BuiltinTypeId::Bool)),
        ])
    );

    let items = common::tuple_items(&value);
    let repeated_strings = common::list_elements(&items[0]);
    assert_eq!(repeated_strings.len(), 3);
    assert_eq!(repeated_strings[0].to_rust::<String>().unwrap(), "rex");
    assert_eq!(repeated_strings[1].to_rust::<String>().unwrap(), "rex");
    assert_eq!(repeated_strings[2].to_rust::<String>().unwrap(), "rex");

    let repeated_bools = common::list_elements(&items[1]);
    assert_eq!(repeated_bools.len(), 2);
    assert!(repeated_bools[0].to_rust::<bool>().unwrap());
    assert!(repeated_bools[1].to_rust::<bool>().unwrap());
}

#[tokio::test]
async fn generic_export_can_swap_two_values_of_different_types() {
    let mut builder: Builder<()> = Builder::with_prelude(()).unwrap();

    // This is another example of writing a generic function: it introduces
    // independent type variables `P` and `Q` and returns them in swapped order.
    let p_var = builder
        .type_system_mut()
        .fresh_type_var(Some(Symbol::intern("P")));
    let q_var = builder
        .type_system_mut()
        .fresh_type_var(Some(Symbol::intern("Q")));
    let p = Type::var(p_var.clone());
    let q = Type::var(q_var.clone());
    let scheme = Scheme::new(
        vec![p_var, q_var],
        vec![],
        Type::fun(p.clone(), Type::fun(q.clone(), Type::tuple(vec![q, p]))),
    );
    common::inject_globals(&mut builder, |module| {
        module.export_native("swap_pair", scheme, 2, |_engine, _, args| {
            Ok(Value::Tuple(vec![args[1].clone(), args[0].clone()]))
        })
    })
    .unwrap();

    let (_heap, value, ty) =
        common::eval_source(builder, r#"(swap_pair "left" 7, swap_pair true "right")"#)
            .await
            .unwrap();
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::tuple(vec![
                Type::builtin(BuiltinTypeId::I32),
                Type::builtin(BuiltinTypeId::String),
            ]),
            Type::tuple(vec![
                Type::builtin(BuiltinTypeId::String),
                Type::builtin(BuiltinTypeId::Bool),
            ]),
        ])
    );

    let items = common::tuple_items(&value);

    let first_swap = common::tuple_items(&items[0]);
    assert_eq!(first_swap[0].to_rust::<i32>().unwrap(), 7);
    assert_eq!(first_swap[1].to_rust::<String>().unwrap(), "left");

    let second_swap = common::tuple_items(&items[1]);
    assert_eq!(second_swap[0].to_rust::<String>().unwrap(), "right");
    assert!(second_swap[1].to_rust::<bool>().unwrap());
}

#[tokio::test]
async fn overloaded_exports_types_and_values() {
    let mut builder: Builder<()> = Builder::with_prelude(()).unwrap();

    EmbedRecord::inject_rex(&mut builder).unwrap();

    common::inject_globals(&mut builder, |module| {
        module.export("over1", |_state: (), x: i32| Ok(x + 1))?;
        module.export("over1", |_state: (), x: bool| {
            Ok(if x {
                "bool:true".to_string()
            } else {
                "bool:false".to_string()
            })
        })?;
        module.export("over1", |_state: (), rec: EmbedRecord| Ok(rec.n > 10))?;
        module.export("over3", |_state: (), a: i32, b: i32, c: i32| Ok(a + b + c))?;
        module.export("over3", |_state: (), a: String, b: String, c: String| {
            Ok(a.len() < b.len() + c.len())
        })?;
        module.export(
            "over3",
            |_state: (), a: EmbedRecord, b: EmbedRecord, c: EmbedRecord| {
                Ok(format!("records:{}:{}:{}", a.n, b.n, c.n))
            },
        )?;
        Ok(())
    })
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

    let body_program = parse_rex(expr).unwrap();
    let compiler = builder.build_compiler();
    let (compiled, evaluator) = compiler
        .compile_program(
            &body_program,
            CompileOptions::for_module("test.main").unwrap(),
        )
        .await
        .unwrap();
    let ty = compiled.result_type().clone();
    assert_overload_tuple_type_shape(&ty);
    let value = evaluator.run(compiled, Default::default()).await;
    assert!(value.is_ok(), "evaluation failed: {value:?}");
    let value = value.unwrap();

    let items = common::tuple_items(&value);
    assert_eq!(items.len(), 6);
    assert_eq!(items[0].to_rust::<i32>().unwrap(), 42);
    assert_eq!(items[1].to_rust::<String>().unwrap(), "bool:true");
    assert!(!items[2].to_rust::<bool>().unwrap());
    assert_eq!(items[3].to_rust::<i32>().unwrap(), 6);
    assert!(items[4].to_rust::<bool>().unwrap());
    assert_eq!(items[5].to_rust::<String>().unwrap(), "records:1:2:3");
}

#[tokio::test]
async fn overloaded_async_exports_types_and_values() {
    let mut builder: Builder<()> = Builder::with_prelude(()).unwrap();
    EmbedRecord::inject_rex(&mut builder).unwrap();

    common::inject_globals(&mut builder, |module| {
        module.export_async("a1", |_state: (), x: i32| async move { Ok(x + 1) })?;
        module.export_async("a1", |_state: (), x: bool| async move {
            Ok(if x {
                "bool:true".to_string()
            } else {
                "bool:false".to_string()
            })
        })?;
        module.export_async("a1", |_state: (), rec: EmbedRecord| async move {
            Ok(rec.n > 10)
        })?;
        module.export_async("a3", |_state: (), a: i32, b: i32, c: i32| async move {
            Ok(a + b + c)
        })?;
        module.export_async(
            "a3",
            |_state: (), a: String, b: String, c: String| async move {
                Ok(a.len() < b.len() + c.len())
            },
        )?;
        module.export_async(
            "a3",
            |_state: (), a: EmbedRecord, b: EmbedRecord, c: EmbedRecord| async move {
                Ok(format!("records:{}:{}:{}", a.n, b.n, c.n))
            },
        )?;
        Ok(())
    })
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

    let body_program = parse_rex(expr).unwrap();
    let compiler = builder.build_compiler();
    let (compiled, evaluator) = compiler
        .compile_program(
            &body_program,
            CompileOptions::for_module("test.main").unwrap(),
        )
        .await
        .unwrap();
    let ty = compiled.result_type().clone();
    assert_overload_tuple_type_shape(&ty);
    let value = evaluator.run(compiled, Default::default()).await;
    assert!(value.is_ok(), "evaluation failed: {value:?}");
    let value = value.unwrap();

    let items = common::tuple_items(&value);
    assert_eq!(items.len(), 6);
    assert_eq!(items[0].to_rust::<i32>().unwrap(), 42);
    assert_eq!(items[1].to_rust::<String>().unwrap(), "bool:true");
    assert!(!items[2].to_rust::<bool>().unwrap());
    assert_eq!(items[3].to_rust::<i32>().unwrap(), 6);
    assert!(items[4].to_rust::<bool>().unwrap());
    assert_eq!(items[5].to_rust::<String>().unwrap(), "records:1:2:3");
}
