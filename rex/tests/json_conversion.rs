mod common;

use rex::{
    Rex,
    ast::Symbol,
    engine::{CompileOptions, Engine, Handle, Heap, Value},
    json::{json_to_rex, rex_to_json},
    parser::parse as parse_rex,
    typesystem::{AdtDecl, BuiltinTypeId, Type, TypeSystem, TypeVarSupply},
};
use serde::Serialize;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

fn mk_type_system() -> TypeSystem {
    TypeSystem::new_with_prelude().unwrap()
}

fn mk_unit_enum(name: &str, variants: &[&str]) -> AdtDecl {
    let mut supply = TypeVarSupply::new();
    let mut adt = AdtDecl::new(&Symbol::intern(name), &[], &mut supply);
    for variant in variants {
        adt.add_variant(Symbol::intern(variant), vec![]);
    }
    adt
}

fn temp_dir(name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    dir.push(format!("rex-json-eval-{name}-{nanos}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn fixed_uuid() -> Uuid {
    Uuid::parse_str("12345678-1234-5678-90ab-cdef12345678").unwrap()
}

#[derive(Rex, Serialize)]
struct EvalJsonRecord {
    id: i32,
    values: Vec<i32>,
}

fn assert_eval_json(
    type_system: &TypeSystem,
    handle: &Handle,
    typ: &Type,
    expected: serde_json::Value,
) {
    let actual = rex_to_json(handle, typ, type_system).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn primitive_roundtrip() {
    let ts = mk_type_system();
    let heap = Heap::new();

    let cases = vec![
        (Type::builtin(BuiltinTypeId::Bool), json!(true)),
        (Type::builtin(BuiltinTypeId::I32), json!(-7)),
        (Type::builtin(BuiltinTypeId::String), json!("hello")),
    ];

    for (ty, expected_json) in cases {
        let handle = json_to_rex(&heap, &expected_json, &ty, &ts).unwrap();
        let actual_json = rex_to_json(&handle, &ty, &ts).unwrap();
        assert_eq!(actual_json, expected_json);
    }
}

#[test]
fn option_and_result_roundtrip() {
    let ts = mk_type_system();
    let heap = Heap::new();

    let opt_ty = Type::option(Type::builtin(BuiltinTypeId::I32));
    let some = json!(9);
    let none = serde_json::Value::Null;

    let some_handle = json_to_rex(&heap, &some, &opt_ty, &ts).unwrap();
    let none_handle = json_to_rex(&heap, &none, &opt_ty, &ts).unwrap();
    assert_eq!(rex_to_json(&some_handle, &opt_ty, &ts).unwrap(), some);
    assert_eq!(rex_to_json(&none_handle, &opt_ty, &ts).unwrap(), none);

    let res_ty = Type::result(
        Type::builtin(BuiltinTypeId::I32),
        Type::builtin(BuiltinTypeId::String),
    );
    let ok_json = json!({ "Ok": 1 });
    let err_json = json!({ "Err": "bad" });
    let ok_handle = json_to_rex(&heap, &ok_json, &res_ty, &ts).unwrap();
    let err_handle = json_to_rex(&heap, &err_json, &res_ty, &ts).unwrap();
    assert_eq!(rex_to_json(&ok_handle, &res_ty, &ts).unwrap(), ok_json);
    assert_eq!(rex_to_json(&err_handle, &res_ty, &ts).unwrap(), err_json);
}

#[test]
fn promise_roundtrip_from_json() {
    let ts = mk_type_system();
    let heap = Heap::new();
    let promise_ty = Type::promise(Type::builtin(BuiltinTypeId::I32));
    let promise_json = json!(fixed_uuid());

    let promise_handle = json_to_rex(&heap, &promise_json, &promise_ty, &ts).unwrap();
    let Value::Adt(tag, args) = promise_handle.value().unwrap() else {
        panic!("expected Promise ADT");
    };
    assert_eq!(tag.as_ref(), "Promise");
    assert_eq!(args.len(), 1);
    assert_eq!(args[0].to_rust::<Uuid>().unwrap(), fixed_uuid());
    assert_eq!(
        rex_to_json(&promise_handle, &promise_ty, &ts).unwrap(),
        promise_json
    );
}

#[test]
fn promise_roundtrip_from_runtime_value() {
    let ts = mk_type_system();
    let heap = Heap::new();
    let promise_ty = Type::promise(Type::builtin(BuiltinTypeId::String));
    let promise_id = heap.alloc_uuid(fixed_uuid()).unwrap();
    let promise_handle = heap
        .alloc_adt(Symbol::intern("Promise"), vec![promise_id])
        .unwrap();

    let promise_json = rex_to_json(&promise_handle, &promise_ty, &ts).unwrap();
    assert_eq!(promise_json, json!(fixed_uuid()));

    let roundtrip_handle = json_to_rex(&heap, &promise_json, &promise_ty, &ts).unwrap();
    let Value::Adt(tag, args) = roundtrip_handle.value().unwrap() else {
        panic!("expected Promise ADT");
    };
    assert_eq!(tag.as_ref(), "Promise");
    assert_eq!(args.len(), 1);
    assert_eq!(args[0].to_rust::<Uuid>().unwrap(), fixed_uuid());
}

#[test]
fn json_array_maps_to_array_not_list() {
    let ts = mk_type_system();
    let heap = Heap::new();
    let array_json = json!([1, 2, 3]);

    let array_ty = Type::array(Type::builtin(BuiltinTypeId::I32));
    let array_handle = json_to_rex(&heap, &array_json, &array_ty, &ts).unwrap();
    let Value::Array(items) = array_handle.value().unwrap() else {
        panic!("expected array");
    };
    assert_eq!(items.len(), 3);
    assert_eq!(
        rex_to_json(&array_handle, &array_ty, &ts).unwrap(),
        array_json
    );

    let list_ty = Type::list(Type::builtin(BuiltinTypeId::I32));
    let list_handle = json_to_rex(&heap, &array_json, &list_ty, &ts).unwrap();
    let Value::Adt(tag, _args) = list_handle.value().unwrap() else {
        panic!("expected list ADT");
    };
    assert_eq!(tag.as_ref(), "Cons");
}

#[test]
fn struct_like_single_variant_adt_roundtrip() {
    let mut ts = mk_type_system();
    let heap = Heap::new();

    let mut supply = TypeVarSupply::new();
    let mut foo = AdtDecl::new(&Symbol::intern("Foo"), &[], &mut supply);
    foo.add_variant(
        Symbol::intern("Foo"),
        vec![Type::record(vec![
            (Symbol::intern("a"), Type::builtin(BuiltinTypeId::U64)),
            (Symbol::intern("b"), Type::builtin(BuiltinTypeId::String)),
        ])],
    );
    ts.register_adt(&foo);

    let foo_ty = Type::con("Foo", 0);
    let foo_json = json!({ "a": 42, "b": "Hello" });

    let foo_handle = json_to_rex(&heap, &foo_json, &foo_ty, &ts).unwrap();
    let Value::Adt(tag, args) = foo_handle.value().unwrap() else {
        panic!("expected Foo ADT");
    };
    assert_eq!(tag.as_ref(), "Foo");
    assert_eq!(args.len(), 1);
    assert_eq!(rex_to_json(&foo_handle, &foo_ty, &ts).unwrap(), foo_json);
}

#[test]
fn unit_enum_string_roundtrip() {
    let mut ts = mk_type_system();
    let heap = Heap::new();

    let color = mk_unit_enum("Color", &["Red", "Green", "Blue"]);
    ts.register_adt(&color);
    let color_ty = Type::con("Color", 0);

    for v in [json!("Red"), json!("Green"), json!("Blue")] {
        let handle = json_to_rex(&heap, &v, &color_ty, &ts).unwrap();
        let actual = rex_to_json(&handle, &color_ty, &ts).unwrap();
        assert_eq!(actual, v);
    }
}

#[tokio::test]
async fn eval_entry_points_return_type_for_json_eval() {
    fn engine_with_eval_json_record() -> Engine {
        let mut engine = Engine::with_prelude(()).unwrap();
        EvalJsonRecord::inject_rex(&mut engine).unwrap();
        engine
    }

    let rex_code = "EvalJsonRecord { id = 7, values = to_array [1, 2, 3, 5, 8] }";
    let expected_json = json!({
        "id": 7,
        "values": [1, 2, 3, 5, 8]
    });
    assert_eq!(
        serde_json::to_value(EvalJsonRecord {
            id: 7,
            values: vec![1, 2, 3, 5, 8],
        })
        .unwrap(),
        expected_json
    );
    let parsed = parse_rex(rex_code).unwrap();
    let engine = engine_with_eval_json_record();
    let type_system = engine.type_system.clone();
    let mut compiler = engine.into_compiler();
    assert!(parsed.decls.is_empty());
    let compiled = compiler
        .compile_program(&parsed, Default::default())
        .await
        .unwrap();
    let ty_eval = compiled.result_type().clone();
    let handle_eval = compiler
        .into_evaluator()
        .run(compiled, Default::default())
        .await
        .unwrap();
    assert_eval_json(&type_system, &handle_eval, &ty_eval, expected_json.clone());
    let mut compiler = engine_with_eval_json_record().into_compiler();
    let parsed = parse_rex(rex_code).unwrap();
    let program = compiler
        .compile_program(&parsed, CompileOptions::default())
        .await
        .unwrap();
    let ty_snippet = program.result_type().clone();
    let handle_snippet = compiler
        .into_evaluator()
        .run(program, Default::default())
        .await
        .unwrap();
    assert_eval_json(
        &type_system,
        &handle_snippet,
        &ty_snippet,
        expected_json.clone(),
    );

    let dir = temp_dir("snippet-at");
    let importer = dir.join("main.rex");
    fs::write(&importer, "()").unwrap();
    let mut compiler = engine_with_eval_json_record().into_compiler();
    let parsed = parse_rex(rex_code).unwrap();
    let program = compiler
        .compile_program(
            &parsed,
            CompileOptions::default().with_importer_path(importer.as_path()),
        )
        .await
        .unwrap();
    let ty_snippet_at = program.result_type().clone();
    let handle_snippet_at = compiler
        .into_evaluator()
        .run(program, Default::default())
        .await
        .unwrap();
    assert_eval_json(
        &type_system,
        &handle_snippet_at,
        &ty_snippet_at,
        expected_json,
    );
}
