mod common;

use rex::{
    engine::{Builder, CompiledProgram, EngineError, Evaluator},
    json::{json_to_main_inputs, rex_to_json},
    parser::parse as parse_rex,
};
use serde_json::{Value, json};

struct Prepared {
    compiled: CompiledProgram,
    evaluator: Evaluator,
    manifest: Value,
}

async fn prepare(source: &str) -> Prepared {
    let parsed = parse_rex(source).unwrap();
    let mut compiler = Builder::with_prelude(()).unwrap().build_compiler();
    let compiled = compiler
        .compile_program(&parsed, Default::default())
        .await
        .unwrap();
    let manifest = compiled
        .main_signature()
        .manifest(compiler.type_system())
        .unwrap();
    let manifest = serde_json::to_value(manifest).unwrap();
    let evaluator = compiler.into_evaluator();
    Prepared {
        compiled,
        evaluator,
        manifest,
    }
}

async fn run_with_json_inputs(source: &str, input_json: Value) -> (Value, Value) {
    let Prepared {
        compiled,
        evaluator,
        manifest,
    } = prepare(source).await;
    let result_type = compiled.result_type().clone();
    let type_system = evaluator.type_system();
    let inputs = json_to_main_inputs(
        evaluator.heap(),
        input_json,
        compiled.main_signature(),
        type_system.as_ref(),
    )
    .unwrap();
    let value = evaluator.run(compiled, inputs).await.unwrap();
    (
        rex_to_json(&value, &result_type, type_system.as_ref()).unwrap(),
        manifest,
    )
}

async fn json_input_error(source: &str, input_json: Value) -> (EngineError, Value) {
    let Prepared {
        compiled,
        evaluator,
        manifest,
    } = prepare(source).await;
    let type_system = evaluator.type_system();
    let err = json_to_main_inputs(
        evaluator.heap(),
        input_json,
        compiled.main_signature(),
        type_system.as_ref(),
    )
    .unwrap_err();
    (err, manifest)
}

fn assert_manifest(manifest: &Value, inputs: &[(&str, &str)], result: &str) {
    let types = manifest
        .pointer("/typeBundle/types")
        .and_then(Value::as_object)
        .unwrap();
    let mut actual_names = types.keys().map(String::as_str).collect::<Vec<_>>();
    actual_names.sort_unstable();

    let mut expected_names = inputs
        .iter()
        .map(|(name, _)| format!("input.{name}"))
        .collect::<Vec<_>>();
    expected_names.push("result".to_string());
    expected_names.sort();
    assert_eq!(actual_names, expected_names);

    for (name, typ) in inputs {
        assert_manifest_builtin_type(types.get(&format!("input.{name}")).unwrap(), typ);
    }
    assert_manifest_builtin_type(types.get("result").unwrap(), result);
}

fn assert_manifest_builtin_type(scheme: &Value, expected: &str) {
    assert_eq!(scheme.pointer("/type/kind"), Some(&json!("builtin")));
    assert_eq!(scheme.pointer("/type/name"), Some(&json!(expected)));
}

fn assert_main_input_mismatch(err: EngineError, mut missing: Vec<&str>, mut extra: Vec<&str>) {
    let EngineError::MainInputMismatch {
        missing: mut actual_missing,
        extra: mut actual_extra,
    } = err
    else {
        panic!("expected MainInputMismatch, got {err:?}");
    };
    missing.sort_unstable();
    extra.sort_unstable();
    actual_missing.sort();
    actual_extra.sort();
    assert_eq!(actual_missing, missing);
    assert_eq!(actual_extra, extra);
}

#[tokio::test]
async fn json_to_main_inputs_accepts_zero_inputs() {
    let (actual, manifest) = run_with_json_inputs("41", json!({})).await;
    assert_manifest(&manifest, &[], "i32");
    assert_eq!(actual, json!(41));
}

#[tokio::test]
async fn json_to_main_inputs_rejects_extra_zero_input() {
    let (err, manifest) = json_input_error("41", json!({ "extra": 1 })).await;
    assert_manifest(&manifest, &[], "i32");
    assert_main_input_mismatch(err, vec![], vec!["extra"]);
}

#[tokio::test]
async fn json_to_main_inputs_accepts_one_input() {
    let (actual, manifest) = run_with_json_inputs(
        r#"
            fn main x: i32 -> i32 = x + 1;
        "#,
        json!({ "x": 41 }),
    )
    .await;

    assert_manifest(&manifest, &[("x", "i32")], "i32");
    assert_eq!(actual, json!(42));
}

#[tokio::test]
async fn json_to_main_inputs_rejects_missing_one_input() {
    let (err, manifest) = json_input_error(
        r#"
            fn main x: i32 -> i32 = x + 1;
        "#,
        json!({}),
    )
    .await;

    assert_manifest(&manifest, &[("x", "i32")], "i32");
    assert_main_input_mismatch(err, vec!["x"], vec![]);
}

#[tokio::test]
async fn json_to_main_inputs_rejects_wrong_type_one_input() {
    let (err, manifest) = json_input_error(
        r#"
            fn main x: i32 -> i32 = x + 1;
        "#,
        json!({ "x": "not an integer" }),
    )
    .await;

    assert_manifest(&manifest, &[("x", "i32")], "i32");
    let EngineError::Custom(message) = err else {
        panic!("expected Custom conversion error, got {err:?}");
    };
    assert!(message.contains("failed to convert input `x` from JSON"));
}

#[tokio::test]
async fn json_to_main_inputs_accepts_multiple_inputs() {
    let (actual, manifest) = run_with_json_inputs(
        r#"
            fn main x: i32 -> y: i32 -> i32 = x + y;
        "#,
        json!({ "y": 5, "x": 37 }),
    )
    .await;

    assert_manifest(&manifest, &[("x", "i32"), ("y", "i32")], "i32");
    assert_eq!(actual, json!(42));
}

#[tokio::test]
async fn json_to_main_inputs_rejects_missing_and_extra_multiple_inputs() {
    let (err, manifest) = json_input_error(
        r#"
            fn main x: i32 -> y: i32 -> i32 = x + y;
        "#,
        json!({ "x": 37, "z": 5 }),
    )
    .await;

    assert_manifest(&manifest, &[("x", "i32"), ("y", "i32")], "i32");
    assert_main_input_mismatch(err, vec!["y"], vec!["z"]);
}
