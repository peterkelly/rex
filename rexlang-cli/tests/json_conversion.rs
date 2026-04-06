use rexlang::{
    AdtDecl, BuiltinTypeId, Engine, EngineError, EnumPatch, GasMeter, Heap, JsonOptions, Parser,
    Pointer, Program, ReplState, Rex, Token, Type, TypeSystem, TypeVarSupply, intern, json_to_rex,
    rex_to_json, sym,
};
use serde::Serialize;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn mk_type_system() -> TypeSystem {
    TypeSystem::new_with_prelude().unwrap()
}

fn mk_unit_enum(name: &str, variants: &[&str]) -> AdtDecl {
    let mut supply = TypeVarSupply::new();
    let mut adt = AdtDecl::new(&intern(name), &[], &mut supply);
    for variant in variants {
        adt.add_variant(intern(variant), vec![]);
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

fn parse_program(source: &str) -> Program {
    let tokens = Token::tokenize(source).unwrap();
    let mut parser = Parser::new(tokens);
    parser.parse_program(&mut GasMeter::default()).unwrap()
}

#[derive(Rex, Serialize)]
struct EvalJsonRecord {
    id: i32,
    values: Vec<i32>,
}

fn assert_eval_json(
    engine: &Engine<()>,
    pointer: &Pointer,
    typ: &Type,
    expected: serde_json::Value,
) {
    let actual = rex_to_json(
        &engine.heap,
        pointer,
        typ,
        &engine.type_system,
        &JsonOptions::default(),
    )
    .unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn primitive_roundtrip() {
    let ts = mk_type_system();
    let heap = Heap::new();
    let opts = JsonOptions::default();

    let cases = vec![
        (Type::builtin(BuiltinTypeId::Bool), json!(true)),
        (Type::builtin(BuiltinTypeId::I32), json!(-7)),
        (Type::builtin(BuiltinTypeId::String), json!("hello")),
    ];

    for (ty, expected_json) in cases {
        let ptr = json_to_rex(&heap, &expected_json, &ty, &ts, &opts).unwrap();
        let actual_json = rex_to_json(&heap, &ptr, &ty, &ts, &opts).unwrap();
        assert_eq!(actual_json, expected_json);
    }
}

#[test]
fn option_and_result_roundtrip() {
    let ts = mk_type_system();
    let heap = Heap::new();
    let opts = JsonOptions::default();

    let opt_ty = Type::option(Type::builtin(BuiltinTypeId::I32));
    let some = json!(9);
    let none = serde_json::Value::Null;

    let some_ptr = json_to_rex(&heap, &some, &opt_ty, &ts, &opts).unwrap();
    let none_ptr = json_to_rex(&heap, &none, &opt_ty, &ts, &opts).unwrap();
    assert_eq!(
        rex_to_json(&heap, &some_ptr, &opt_ty, &ts, &opts).unwrap(),
        some
    );
    assert_eq!(
        rex_to_json(&heap, &none_ptr, &opt_ty, &ts, &opts).unwrap(),
        none
    );

    let res_ty = Type::result(
        Type::builtin(BuiltinTypeId::I32),
        Type::builtin(BuiltinTypeId::String),
    );
    let ok_json = json!({ "Ok": 1 });
    let err_json = json!({ "Err": "bad" });
    let ok_ptr = json_to_rex(&heap, &ok_json, &res_ty, &ts, &opts).unwrap();
    let err_ptr = json_to_rex(&heap, &err_json, &res_ty, &ts, &opts).unwrap();
    assert_eq!(
        rex_to_json(&heap, &ok_ptr, &res_ty, &ts, &opts).unwrap(),
        ok_json
    );
    assert_eq!(
        rex_to_json(&heap, &err_ptr, &res_ty, &ts, &opts).unwrap(),
        err_json
    );
}

#[test]
fn json_array_maps_to_array_not_list() {
    let ts = mk_type_system();
    let heap = Heap::new();
    let opts = JsonOptions::default();
    let array_json = json!([1, 2, 3]);

    let array_ty = Type::array(Type::builtin(BuiltinTypeId::I32));
    let array_ptr = json_to_rex(&heap, &array_json, &array_ty, &ts, &opts).unwrap();
    let items = heap.pointer_as_array(&array_ptr).unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(
        rex_to_json(&heap, &array_ptr, &array_ty, &ts, &opts).unwrap(),
        array_json
    );

    let list_ty = Type::list(Type::builtin(BuiltinTypeId::I32));
    let list_ptr = json_to_rex(&heap, &array_json, &list_ty, &ts, &opts).unwrap();
    let (tag, _args) = heap.pointer_as_adt(&list_ptr).unwrap();
    assert_eq!(tag.as_ref(), "Cons");
}

#[test]
fn struct_like_single_variant_adt_roundtrip() {
    let mut ts = mk_type_system();
    let heap = Heap::new();
    let opts = JsonOptions::default();

    let mut supply = TypeVarSupply::new();
    let mut foo = AdtDecl::new(&sym("Foo"), &[], &mut supply);
    foo.add_variant(
        sym("Foo"),
        vec![Type::record(vec![
            (sym("a"), Type::builtin(BuiltinTypeId::U64)),
            (sym("b"), Type::builtin(BuiltinTypeId::String)),
        ])],
    );
    ts.register_adt(&foo);

    let foo_ty = Type::con("Foo", 0);
    let foo_json = json!({ "a": 42, "b": "Hello" });

    let foo_ptr = json_to_rex(&heap, &foo_json, &foo_ty, &ts, &opts).unwrap();
    let (tag, args) = heap.pointer_as_adt(&foo_ptr).unwrap();
    assert_eq!(tag.as_ref(), "Foo");
    assert_eq!(args.len(), 1);
    assert_eq!(
        rex_to_json(&heap, &foo_ptr, &foo_ty, &ts, &opts).unwrap(),
        foo_json
    );
}

#[test]
fn unit_enum_string_roundtrip() {
    let mut ts = mk_type_system();
    let heap = Heap::new();
    let opts = JsonOptions::default();

    let color = mk_unit_enum("Color", &["Red", "Green", "Blue"]);
    ts.register_adt(&color);
    let color_ty = Type::con("Color", 0);

    for v in [json!("Red"), json!("Green"), json!("Blue")] {
        let ptr = json_to_rex(&heap, &v, &color_ty, &ts, &opts).unwrap();
        let actual = rex_to_json(&heap, &ptr, &color_ty, &ts, &opts).unwrap();
        assert_eq!(actual, v);
    }
}

#[test]
fn unit_enum_integer_roundtrip_with_patches() {
    let mut ts = mk_type_system();
    let heap = Heap::new();
    let color = mk_unit_enum("Color", &["Red", "Green", "Blue"]);
    ts.register_adt(&color);
    let color_ty = Type::con("Color", 0);

    let mut opts = JsonOptions::default();
    opts.add_int_enum("Color");

    let red = heap.alloc_adt(sym("Red"), vec![]).unwrap();
    let green = heap.alloc_adt(sym("Green"), vec![]).unwrap();
    let blue = heap.alloc_adt(sym("Blue"), vec![]).unwrap();
    assert_eq!(
        rex_to_json(&heap, &red, &color_ty, &ts, &opts).unwrap(),
        json!(0)
    );
    assert_eq!(
        rex_to_json(&heap, &green, &color_ty, &ts, &opts).unwrap(),
        json!(1)
    );
    assert_eq!(
        rex_to_json(&heap, &blue, &color_ty, &ts, &opts).unwrap(),
        json!(2)
    );

    let ptr = json_to_rex(&heap, &json!(2), &color_ty, &ts, &opts).unwrap();
    let (tag, args) = heap.pointer_as_adt(&ptr).unwrap();
    assert_eq!(tag.as_ref(), "Blue");
    assert!(args.is_empty());

    opts.add_int_enum_with_patches(
        "Color",
        vec![
            EnumPatch {
                enum_name: "Red".to_string(),
                discriminant: 10,
            },
            EnumPatch {
                enum_name: "Blue".to_string(),
                discriminant: 42,
            },
        ],
    );

    assert_eq!(
        rex_to_json(&heap, &red, &color_ty, &ts, &opts).unwrap(),
        json!(10)
    );
    assert_eq!(
        rex_to_json(&heap, &green, &color_ty, &ts, &opts).unwrap(),
        json!(1)
    );
    assert_eq!(
        rex_to_json(&heap, &blue, &color_ty, &ts, &opts).unwrap(),
        json!(42)
    );

    let blue_from_patch = json_to_rex(&heap, &json!(42), &color_ty, &ts, &opts).unwrap();
    let (tag, args) = heap.pointer_as_adt(&blue_from_patch).unwrap();
    assert_eq!(tag.as_ref(), "Blue");
    assert!(args.is_empty());
}

#[test]
fn unit_enum_integer_unknown_discriminant_errors() {
    let mut ts = mk_type_system();
    let heap = Heap::new();
    let color = mk_unit_enum("Color", &["Red", "Green", "Blue"]);
    ts.register_adt(&color);
    let color_ty = Type::con("Color", 0);

    let mut opts = JsonOptions::default();
    opts.add_int_enum("Color");

    let err = json_to_rex(&heap, &json!(99), &color_ty, &ts, &opts).unwrap_err();
    let EngineError::Custom(msg) = err else {
        panic!("expected EngineError::Custom");
    };
    assert!(msg.contains("expected integer enum JSON for `Color`"));
}

#[tokio::test]
async fn eval_entry_points_return_type_for_json_eval() {
    let mut engine = Engine::with_prelude(()).unwrap();
    EvalJsonRecord::inject_rex(&mut engine).unwrap();
    let rex_code = "EvalJsonRecord { id = 7, values = [1, 2, 3, 5, 8] }";
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

    let mut gas = GasMeter::default();
    let expr_program = parse_program(rex_code);
    let (ptr_eval, ty_eval) = rexlang::Evaluator::new_with_compiler(
        rexlang::RuntimeEnv::new(engine.clone()),
        rexlang::Compiler::new(engine.clone()),
    )
    .eval(expr_program.expr.as_ref(), &mut gas)
    .await
    .unwrap();
    assert_eval_json(&engine, &ptr_eval, &ty_eval, expected_json.clone());

    let mut gas = GasMeter::default();
    let (ptr_snippet, ty_snippet) = rexlang::Evaluator::new_with_compiler(
        rexlang::RuntimeEnv::new(engine.clone()),
        rexlang::Compiler::new(engine.clone()),
    )
    .eval_snippet(rex_code, &mut gas)
    .await
    .unwrap();
    assert_eval_json(&engine, &ptr_snippet, &ty_snippet, expected_json.clone());

    let dir = temp_dir("snippet-at");
    let importer = dir.join("main.rex");
    fs::write(&importer, "()").unwrap();
    let mut gas = GasMeter::default();
    let (ptr_snippet_at, ty_snippet_at) = rexlang::Evaluator::new_with_compiler(
        rexlang::RuntimeEnv::new(engine.clone()),
        rexlang::Compiler::new(engine.clone()),
    )
    .eval_snippet_at(rex_code, &importer, &mut gas)
    .await
    .unwrap();
    assert_eval_json(
        &engine,
        &ptr_snippet_at,
        &ty_snippet_at,
        expected_json.clone(),
    );

    let repl_program = parse_program(rex_code);
    let mut repl_state = ReplState::new();
    let mut gas = GasMeter::default();
    let (ptr_repl, ty_repl) = rexlang::Evaluator::new_with_compiler(
        rexlang::RuntimeEnv::new(engine.clone()),
        rexlang::Compiler::new(engine.clone()),
    )
    .eval_repl_program(&repl_program, &mut repl_state, &mut gas)
    .await
    .unwrap();
    assert_eval_json(&engine, &ptr_repl, &ty_repl, expected_json);
}
