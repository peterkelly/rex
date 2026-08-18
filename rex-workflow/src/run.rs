use crate::{
    modules::{
        std::{artifacts, storage::storage_module},
        tools,
    },
    state::State,
};
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

    builder.inject_module(artifacts::module()?)?;
    builder.inject_module(storage_module()?)?;
    builder.add_importer(tools::importer());

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

    #[tokio::test]
    async fn shared_artifacts_construct_and_roundtrip_as_json() {
        let source = r#"
            import std.artifacts (Image, JsonFile, Media, Pdf);

            fn main (content: Hash) -> (Pdf, Image, Media, JsonFile) =
                (
                    Pdf { content = content },
                    Image { content = content },
                    Media { content = content },
                    JsonFile { content = content }
                );
        "#;
        let hash = "ea8f163db38682925e4491c5e58d4bb3506ef8c14eb78a86e908c5624a67200f";
        let result = eval_rex(
            source,
            Some(json!({ "content": hash })),
            State::local(Store::new_in_memory()),
        )
        .await
        .unwrap();
        assert_eq!(
            result,
            json!([
                { "content": hash },
                { "content": hash },
                { "content": hash },
                { "content": hash }
            ])
        );
    }

    #[tokio::test]
    async fn shared_pdf_composes_qpdf_and_poppler() {
        let source = r#"
            import std.artifacts (Pdf);
            import tools.poppler as P;
            import tools.qpdf as Q;

            fn rewrite (pdf: Pdf) -> Result Q.PdfOutput Q.QpdfError =
                Q.transform pdf None [];

            fn inspect (output: Q.PdfOutput) -> Result P.PdfInfo P.PopplerError =
                P.pdfinfo output.pdf default;

            fn main (value: Bool) -> Bool = value;
        "#;
        let result = eval_rex(
            source,
            Some(json!({ "value": true })),
            State::local(Store::new_in_memory()),
        )
        .await
        .unwrap();
        assert_eq!(result, json!(true));
    }

    #[tokio::test]
    async fn examples_compile() {
        let examples = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
        let mut paths = [
            "imagemagick",
            "ffmpeg",
            "gnuplot",
            "graphviz",
            "qpdf",
            "poppler",
            "imagemagick_ffmpeg",
        ]
        .into_iter()
        .flat_map(|directory| std::fs::read_dir(examples.join(directory)).unwrap())
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "rex"))
        .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            let source = std::fs::read_to_string(&path).unwrap();
            let program = parse_rex(&source)
                .unwrap_or_else(|errors| panic!("{} did not parse: {errors:?}", path.display()));
            compile_cli_program(&program, State::local(Store::new_in_memory()))
                .await
                .unwrap_or_else(|error| panic!("{} did not compile: {error}", path.display()));
        }
    }
}
