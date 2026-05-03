#![allow(clippy::disallowed_names)]

use rex::{
    Rex,
    engine::{Compiler, Engine, EngineError, Evaluator, Handle, Module, RuntimeEnv},
    json::{JsonOptions, rex_to_json},
    typesystem::Type,
};
use serde::{Deserialize, Serialize};

fn engine_with_prelude() -> Engine {
    Engine::with_prelude(()).unwrap()
}
async fn eval_snippet<State: Clone + Send + Sync + 'static>(
    engine: &mut Engine<State>,
    source: &str,
) -> Result<(Handle, Type), EngineError> {
    Evaluator::new_with_compiler(
        RuntimeEnv::new(engine.clone()),
        Compiler::new(engine.clone()),
    )
    .eval_snippet(source)
    .await
    .map_err(|err| err.into_engine_error())
}

#[derive(Rex, Clone, Debug, PartialEq, Deserialize, Serialize)]
enum EchoEnum {
    Foo,
    #[serde(rename = "BAR")]
    Bar,
}

#[derive(Rex, Clone, Debug, PartialEq, Deserialize, Serialize)]
struct EchoRecord {
    foo: u8,
    bar: u8,
    optbar: Option<u8>,
}

#[tokio::test]
async fn injected_echo_module_roundtrips_embedder_types_through_json() {
    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();

    let mut module = Module::new("echo");
    module.add_rex_adt::<EchoEnum>().unwrap();
    module.add_rex_adt::<EchoRecord>().unwrap();
    module
        .export(
            "echo",
            |_state: &(), variant: EchoEnum, record: EchoRecord| Ok((variant, record)),
        )
        .unwrap();
    engine.inject_module(module).unwrap();

    let (value_handle, ty) = eval_snippet(
        &mut engine,
        r#"
        import echo (EchoEnum, EchoRecord, Foo, BAR, echo);

        let
          foo_variant: EchoEnum = Foo,
          foo_record: EchoRecord =
            EchoRecord {
              foo = (1 is u8),
              bar = (2 is u8),
              optbar = Some (3 is u8)
            },
          bar_variant: EchoEnum = BAR,
          bar_record: EchoRecord =
            EchoRecord {
              foo = (4 is u8),
              bar = (5 is u8),
              optbar = None
            },
          foo_result = echo foo_variant foo_record,
          bar_result = echo bar_variant bar_record
        in
          [foo_result, bar_result]
        "#,
    )
    .await
    .unwrap();

    let parsed = rex_to_json(
        &value_handle,
        &ty,
        &engine.type_system,
        &JsonOptions::default(),
    )
    .unwrap();
    let items = parsed.as_array().expect("expected top-level array");
    assert_eq!(items.len(), 2);

    let first = items[0].as_array().expect("expected tuple JSON array");
    assert_eq!(first.len(), 2);
    assert_eq!(
        first[1],
        serde_json::json!({ "foo": 1, "bar": 2, "optbar": 3 })
    );
    let first_variant = first[0].as_str().expect("expected enum JSON string");
    assert_eq!(first_variant, "Foo");

    let second = items[1].as_array().expect("expected tuple JSON array");
    assert_eq!(second.len(), 2);
    assert_eq!(
        second[1],
        serde_json::json!({ "foo": 4, "bar": 5, "optbar": null })
    );
    let second_variant = second[0].as_str().expect("expected enum JSON string");
    assert_eq!(second_variant, "BAR");
}
