use crate::{modules::storage::storage_module, state::State};
use rex::{
    ast::CompilationUnit,
    engine::{
        Builder, CompileOptions, CompiledProgram, Evaluator, MainInputSpec, ModuleId, type_has_vars,
    },
    json::{json_to_main_inputs, rex_to_json},
    parser::{ParseError, parse as parse_rex},
};

pub async fn eval_rex(
    source: &str,
    inputs: Option<serde_json::Value>,
    state: State,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let program = parse_rex(source).map_err(|errs| format_parse_errors(&errs))?;
    let result = eval_result_json(&program, inputs, state).await?;
    Ok(result)
}

pub async fn eval_result_json(
    program: &CompilationUnit,
    inputs: Option<serde_json::Value>,
    state: State,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let (evaluator, compiled) = compile_cli_program(program, state).await?;
    let signature = compiled.main_signature().clone();
    ensure_concrete_inputs(signature.inputs())?;
    let input_value: serde_json::Value = match inputs {
        Some(inputs) => inputs,
        None if !signature.inputs().is_empty() => {
            return Err("program requires `main` inputs".to_string().into());
        }
        None => serde_json::json!({}),
    };
    let result_type = compiled.result_type().clone();
    let type_system = evaluator.type_system();
    let inputs = json_to_main_inputs(input_value, &signature, type_system.as_ref())
        .map_err(|e| format!("{e}"))?;
    let value = evaluator
        .run(compiled, inputs)
        .await
        .map_err(|e| format!("{e}"))?;

    rex_to_json(&value, &result_type, type_system.as_ref())
        .map_err(|e| format!("failed to convert result to JSON: {e}").into())
}

pub fn format_parse_errors(errs: &[ParseError]) -> String {
    let mut out = String::from("parse error:");
    for err in errs {
        out.push_str(&format!("\n  {err}"));
    }
    out
}

async fn compile_cli_program(
    program: &CompilationUnit,
    state: State,
) -> Result<(Evaluator<State>, CompiledProgram), Box<dyn std::error::Error>> {
    let module_id = ModuleId::parse("main").map_err(|err| err.to_string())?;
    let options = CompileOptions::new(module_id);
    let mut builder =
        Builder::with_prelude(state).map_err(|e| format!("failed to initialize builder: {e}"))?;

    builder.inject_module(storage_module()?)?;

    let compiler = builder.build_compiler();
    let (compiled, evaluator) = compiler
        .compile_program(program, options)
        .await
        .map_err(|e| format!("{e}"))?;
    Ok((evaluator, compiled))
}

fn ensure_concrete_inputs(inputs: &[MainInputSpec]) -> Result<(), Box<dyn std::error::Error>> {
    for input in inputs {
        if type_has_vars(&input.typ) {
            return Err(format!(
                "`main` parameter `{}` has polymorphic type `{}`; JSON inputs require concrete parameter types",
                input.name, input.typ
            ).into());
        }
    }
    Ok(())
}

pub fn render_result_json(value: &serde_json::Value, raw_output: bool) -> Result<String, String> {
    if raw_output && let Some(value) = value.as_str() {
        return Ok(value.to_string());
    }

    serde_json::to_string_pretty(value)
        .map_err(|e| format!("failed to serialize result to JSON: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::store::Store;
    use serde_json::json;

    #[tokio::test]
    async fn main_with_inputs() {
        let source = r#"
            type SharedMeta = SharedMeta {
                label: String,
                weight: i32
            };

            type Measurement = Measurement {
                meta: SharedMeta,
                value: i32
            };

            type Threshold = Threshold {
                meta: SharedMeta,
                limit: i32
            };

            type Output = Output {
                total: i32,
                measurement_label: String,
                threshold_label: String,
                combined_weight: i32
            };

            fn main scale: i32 -> measurement: Measurement -> threshold: Threshold -> Output =
                let
                    measurement_meta = measurement.meta,
                    threshold_meta = threshold.meta,
                    total = (measurement.value * scale)
                        + measurement_meta.weight
                        + threshold.limit
                        + threshold_meta.weight
                in
                    Output {
                        total = total,
                        measurement_label = measurement_meta.label,
                        threshold_label = threshold_meta.label,
                        combined_weight = measurement_meta.weight + threshold_meta.weight
                    };
        "#;
        let inputs = json!({
            "scale": 10,
            "measurement": {
                "meta": {
                    "label": "sample",
                    "weight": 3
                },
                "value": 4
            },
            "threshold": {
                "meta": {
                    "label": "control",
                    "weight": 5
                },
                "limit": 7
            }
        });

        let state = State::local(Store::new_in_memory());
        let result = eval_rex(source, Some(inputs), state).await.unwrap();
        assert_eq!(
            result,
            json!({
                "combined_weight": 8,
                "measurement_label": "sample",
                "threshold_label": "control",
                "total": 55
            })
        );
    }
}
