#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use std::fs;
use std::io::IsTerminal;
use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::Arc;

use clap::{Args, Parser};
use rex::{
    ast::CompilationUnit,
    engine::{
        CompileOptions, CompiledProgram, Compiler, Engine, Importer, MainInputSpec, Manifest,
        ModuleId, type_has_vars,
    },
    json::{json_to_main_inputs, rex_to_json},
    parser::{ParseError, parse as parse_rex},
};
use serde_json::json;

use rex_cli::cli_prelude;
use rex_cli::filesystem_importer::FilesystemImporter;

#[derive(Parser)]
#[command(name = "rex")]
#[command(about = "Rex (Rush Expressions) CLI")]
#[command(arg_required_else_help = true)]
struct Cli {
    #[command(flatten)]
    args: RunArgs,
}

#[derive(Args)]
#[command(arg_required_else_help = true)]
struct RunArgs {
    /// Path to a `.rex` file to run.
    #[arg(
        value_name = "FILE",
        required_unless_present_any = ["code", "stdin"],
        conflicts_with_all = ["code", "stdin"]
    )]
    file: Option<String>,

    /// Inline Rex source code to run.
    #[arg(
        short = 'c',
        long = "code",
        value_name = "CODE",
        required_unless_present_any = ["file", "stdin"],
        conflicts_with_all = ["file", "stdin"]
    )]
    code: Option<String>,

    /// Read Rex source code from stdin.
    #[arg(long = "stdin", required_unless_present_any = ["file", "code"])]
    stdin: bool,

    /// Print the parsed AST as JSON and exit.
    #[arg(long = "emit-ast")]
    emit_ast: bool,

    /// Print the inferred type as JSON and exit.
    #[arg(long = "emit-type", alias = "type")]
    emit_type: bool,

    /// Print input manifest information for a `main` function and exit.
    #[arg(long = "manifest", conflicts_with_all = ["emit_ast", "emit_type", "inputs"])]
    manifest: bool,

    /// JSON file containing inputs for a `main` function.
    #[arg(long = "inputs", value_name = "JSON")]
    inputs: Option<String>,

    /// Print string results directly instead of as JSON string literals.
    #[arg(long = "raw-output")]
    raw_output: bool,

    /// Stack size (in MiB) used for parsing/type inference/evaluation.
    #[arg(long = "stack-size-mb", default_value_t = 16)]
    stack_size_mb: usize,
}

#[tokio::main]
async fn main() {
    init_tracing();
    let cli = Cli::parse();
    if let Err(err) = run(cli).await {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_env("REX_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let ansi = std::io::stderr().is_terminal();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(ansi)
        .with_target(true)
        .with_level(true)
        .with_thread_names(true)
        .with_thread_ids(true)
        .compact()
        .try_init();
}

async fn run(cli: Cli) -> Result<(), String> {
    run_cmd(cli.args).await
}

async fn run_cmd(args: RunArgs) -> Result<(), String> {
    let RunArgs {
        file,
        code,
        stdin,
        emit_ast,
        emit_type,
        manifest,
        inputs,
        raw_output,
        stack_size_mb: _stack_size_mb,
    } = args;

    let source = if let Some(code) = code {
        code
    } else if stdin {
        let mut buf = String::new();
        io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("failed to read stdin: {e}"))?;
        buf
    } else if let Some(path) = &file {
        fs::read_to_string(path).map_err(|e| format!("failed to read `{path}`: {e}"))?
    } else {
        return Err("missing input (file or `-c/--code`)".into());
    };

    run_source(
        &source,
        RunSourceOpts {
            file,
            emit_ast,
            emit_type,
            manifest,
            inputs,
            raw_output,
        },
    )
    .await
}

struct RunSourceOpts {
    file: Option<String>,
    emit_ast: bool,
    emit_type: bool,
    manifest: bool,
    inputs: Option<String>,
    raw_output: bool,
}

fn init_engine() -> Result<(Engine, Arc<dyn Importer>), String> {
    let mut engine =
        Engine::with_prelude(()).map_err(|e| format!("failed to initialize engine: {e}"))?;
    cli_prelude::inject_cli_prelude_engine(&mut engine).map_err(|e| e.to_string())?;
    let filesystem_importer = FilesystemImporter::new();
    let importer: Arc<dyn Importer> = Arc::new(filesystem_importer);
    engine.add_importer("filesystem", Arc::clone(&importer));
    Ok((engine, importer))
}

async fn run_source(source: &str, opts: RunSourceOpts) -> Result<(), String> {
    let RunSourceOpts {
        file,
        emit_ast,
        emit_type,
        manifest,
        inputs,
        raw_output,
    } = opts;

    let program = parse_rex(source).map_err(|errs| format_parse_errors(&errs))?;

    if manifest {
        let manifest = main_manifest(&program, file.as_deref()).await?;
        let rendered = serde_json::to_string_pretty(&manifest)
            .map_err(|e| format!("failed to serialize manifest JSON: {e}"))?;
        println!("{rendered}");
        return Ok(());
    }

    if emit_ast || emit_type {
        if inputs.is_some() {
            return Err("`--inputs` cannot be used with compiler-output flags".into());
        }
        let type_json = if emit_type {
            Some(infer_type_json(&program, file.as_deref()).await?)
        } else {
            None
        };
        let out = emit_json(&program, emit_ast, type_json)?;
        println!("{out}");
        return Ok(());
    }

    let result_json = eval_result_json(&program, file.as_deref(), inputs.as_deref()).await?;
    let rendered = render_result_json(&result_json, raw_output)?;
    println!("{rendered}");
    Ok(())
}

async fn eval_result_json(
    program: &CompilationUnit,
    file: Option<&str>,
    inputs_path: Option<&str>,
) -> Result<serde_json::Value, String> {
    let (compiler, compiled) = compile_cli_program(program, file).await?;
    let signature = compiled.main_signature().clone();
    ensure_concrete_inputs(signature.inputs())?;
    let input_value = match inputs_path {
        Some(path) => {
            let raw =
                fs::read_to_string(path).map_err(|e| format!("failed to read `{path}`: {e}"))?;
            serde_json::from_str(&raw)
                .map_err(|e| format!("failed to parse input JSON `{path}`: {e}"))?
        }
        None if !signature.inputs().is_empty() => {
            return Err("program requires `main` inputs; pass `--inputs <JSON>`".to_string());
        }
        None => serde_json::json!({}),
    };
    let result_type = compiled.result_type().clone();
    let evaluator = compiler.into_evaluator();
    let type_system = evaluator.type_system();
    let inputs = json_to_main_inputs(
        evaluator.heap(),
        input_value,
        &signature,
        type_system.as_ref(),
    )
    .map_err(|e| format!("{e}"))?;
    let value = evaluator
        .run(compiled, inputs)
        .await
        .map_err(|e| format!("{e}"))?;

    rex_to_json(&value, &result_type, type_system.as_ref())
        .map_err(|e| format!("failed to convert result to JSON: {e}"))
}

async fn main_manifest(program: &CompilationUnit, file: Option<&str>) -> Result<Manifest, String> {
    let (compiler, compiled) = compile_cli_program(program, file).await?;
    compiled
        .main_signature()
        .manifest(compiler.type_system())
        .map_err(|e| format!("{e}"))
}

fn ensure_concrete_inputs(inputs: &[MainInputSpec]) -> Result<(), String> {
    for input in inputs {
        if type_has_vars(&input.typ) {
            return Err(format!(
                "`main` parameter `{}` has polymorphic type `{}`; JSON inputs require concrete parameter types",
                input.name, input.typ
            ));
        }
    }
    Ok(())
}

async fn compile_cli_program(
    program: &CompilationUnit,
    file: Option<&str>,
) -> Result<(Compiler, CompiledProgram), String> {
    let (engine, _importer) = init_engine()?;
    let mut compiler = engine.into_compiler();
    let importer_path = file.map(PathBuf::from);
    let compiled = compiler
        .compile_program(program, compile_options(importer_path))
        .await
        .map_err(|e| format!("{e}"))?;
    Ok((compiler, compiled))
}

fn snippet_prefix_source(importer_path: Option<&PathBuf>) -> ModuleId {
    importer_path
        .map(|path| ModuleId::Local { path: path.clone() })
        .unwrap_or_else(|| ModuleId::Virtual("__snippet__".to_string()))
}

fn compile_options(importer_path: Option<PathBuf>) -> CompileOptions {
    let prefix_source = snippet_prefix_source(importer_path.as_ref());
    let options = CompileOptions::default().with_prefix_source(prefix_source);
    match importer_path {
        Some(path) => options.with_importer_path(path),
        None => options,
    }
}

fn render_result_json(value: &serde_json::Value, raw_output: bool) -> Result<String, String> {
    if raw_output && let Some(value) = value.as_str() {
        return Ok(value.to_string());
    }

    serde_json::to_string_pretty(value)
        .map_err(|e| format!("failed to serialize result to JSON: {e}"))
}

fn emit_json(
    compilation_unit: &CompilationUnit,
    emit_ast: bool,
    type_json: Option<serde_json::Value>,
) -> Result<String, String> {
    match (emit_ast, type_json) {
        (true, None) => serde_json::to_string_pretty(compilation_unit)
            .map_err(|e| format!("failed to serialize AST to JSON: {e}")),
        (false, Some(type_json)) => serde_json::to_string_pretty(&type_json)
            .map_err(|e| format!("failed to serialize type to JSON: {e}")),
        (true, Some(type_json)) => serde_json::to_string_pretty(&json!({
            "ast": compilation_unit,
            "type": type_json,
        }))
        .map_err(|e| format!("failed to serialize outputs to JSON: {e}")),
        (false, None) => Err("internal error: emit_json called with no outputs".into()),
    }
}

async fn infer_type_json(
    program: &CompilationUnit,
    file: Option<&str>,
) -> Result<serde_json::Value, String> {
    let (_compiler, compiled) = compile_cli_program(program, file).await?;

    Ok(json!({
        "type": compiled.result_type().to_string(),
        "constraints": [],
    }))
}

fn format_parse_errors(errs: &[ParseError]) -> String {
    let mut out = String::from("parse error:");
    for err in errs {
        out.push_str(&format!("\n  {err}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_shape_is_stable() {
        Cli::command().debug_assert();
    }

    #[test]
    fn raw_output_flag_parses() {
        let cli = Cli::try_parse_from(["rex", "--raw-output", "-c", "\"hello\""]).expect("parse");
        assert!(cli.args.raw_output);
    }

    #[test]
    fn get_fn_decl_uses_supplied_name() {
        let source = r#"
            fn main x: i32 -> i32 = x;
            fn worker x: i32 -> i32 = x + 1;
        "#;
        let program = parse_rex(source).expect("parse");

        assert!(program.get_fn_decl("worker").is_some());
        assert!(program.get_fn_decl("missing").is_none());
    }

    #[tokio::test]
    async fn emit_ast_and_type_are_json() {
        let source = "1 + 2";
        let program = parse_rex(source).expect("parse");

        let ty_json = infer_type_json(&program, None).await.expect("infer");
        let ast_out = emit_json(&program, true, None).expect("emit ast");
        let type_out = emit_json(&program, false, Some(ty_json.clone())).expect("emit type");
        let both_out = emit_json(&program, true, Some(ty_json)).expect("emit both");

        serde_json::from_str::<serde_json::Value>(&ast_out).expect("ast json");
        serde_json::from_str::<serde_json::Value>(&type_out).expect("type json");
        serde_json::from_str::<serde_json::Value>(&both_out).expect("both json");
    }

    #[tokio::test]
    async fn result_output_is_pretty_json() {
        let source = r#"
            type Shape = Circle i32 | Rect i32 i32;

            Rect 2 3
        "#;

        let program = parse_rex(source).expect("parse");
        let value = eval_result_json(&program, None, None)
            .await
            .expect("eval result json");
        assert_eq!(value, serde_json::json!({ "Rect": [2, 3] }));

        let rendered = render_result_json(&value, false).expect("render result json");
        assert_eq!(rendered, "{\n  \"Rect\": [\n    2,\n    3\n  ]\n}");
    }

    #[test]
    fn raw_output_unwraps_string_results_only() {
        let string_value = serde_json::json!("hello\nworld");
        let non_string_value = serde_json::json!(["hello"]);

        let json_rendered = render_result_json(&string_value, false).expect("render json string");
        let raw_rendered = render_result_json(&string_value, true).expect("render raw string");
        let non_string_rendered =
            render_result_json(&non_string_value, true).expect("render non-string");

        assert_eq!(json_rendered, "\"hello\\nworld\"");
        assert_eq!(raw_rendered, "hello\nworld");
        assert_eq!(non_string_rendered, "[\n  \"hello\"\n]");
    }

    #[tokio::test]
    async fn emit_type_file_uses_file_as_import_base() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock is before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("rex-file-snippet-test-{nonce}"));
        std::fs::create_dir_all(root.join("foo")).expect("create temp module dir");

        std::fs::write(
            root.join("foo/bar.rex"),
            r#"
                pub fn inc : i32 -> i32 = \x -> x + 1;
            "#,
        )
        .expect("write bar.rex");

        let main = root.join("main.rex");
        let source = r#"
            import foo.bar as Bar;

            Bar.inc 41
        "#;
        std::fs::write(&main, source).expect("write main.rex");
        let main = main.to_string_lossy().to_string();
        let program = parse_rex(source).expect("parse");

        let json = infer_type_json(&program, Some(&main))
            .await
            .expect("infer file");
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(json.get("type").and_then(|v| v.as_str()), Some("i32"));
    }

    #[tokio::test]
    async fn emit_type_reports_entrypoint_result_type() {
        let source = r#"
            fn main x: i32 -> i32 = x + 1;
        "#;
        let program = parse_rex(source).expect("parse");

        let json = infer_type_json(&program, None)
            .await
            .expect("infer entrypoint");

        assert_eq!(json.get("type").and_then(|v| v.as_str()), Some("i32"));
    }

    #[tokio::test]
    async fn explicit_main_rejects_final_expression() {
        let source = r#"
            fn main x: i32 -> i32 = x + 1;

            41
        "#;
        let program = parse_rex(source).expect("parse");

        let err = infer_type_json(&program, None)
            .await
            .expect_err("main plus final expression should fail");

        assert!(err.contains("defines `main`"), "unexpected error: {err}");
        assert!(err.contains("final expression"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn main_inputs_are_loaded_from_json_by_parameter_name() {
        let source = r#"
            fn main x: i32 -> y: i32 -> i32 = x + y;
        "#;
        let program = parse_rex(source).expect("parse");

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock is before unix epoch")
            .as_nanos();
        let inputs = std::env::temp_dir().join(format!("rex-main-inputs-{nonce}.json"));
        std::fs::write(&inputs, r#"{ "y": 5, "x": 37 }"#).expect("write inputs");

        let actual = eval_result_json(&program, None, Some(&inputs.to_string_lossy()))
            .await
            .expect("eval main");
        let _ = std::fs::remove_file(&inputs);

        assert_eq!(actual, serde_json::json!(42));
    }

    #[tokio::test]
    async fn manifest_reports_main_parameters_result_and_adts() {
        let source = r#"
            type Box = Box { value: i32 };

            fn main box: Box -> i32 = box.value;
        "#;
        let program = parse_rex(source).expect("parse");

        let manifest = main_manifest(&program, None).await.expect("manifest");
        let manifest = serde_json::to_value(manifest).expect("manifest json");

        assert!(manifest.get("entrypoint").is_none());
        assert!(manifest.get("parameters").is_none());
        assert!(manifest.get("inputShape").is_none());
        assert!(
            manifest.pointer("/typeBundle/types/input.box").is_some(),
            "manifest should include a round-trippable parameter type"
        );
        assert!(
            manifest.pointer("/typeBundle/types/result").is_some(),
            "manifest should include a round-trippable result type"
        );
        assert!(
            manifest
                .pointer("/typeBundle/adts")
                .and_then(|v| v.as_array())
                .is_some_and(|adts| !adts.is_empty()),
            "manifest should include referenced ADTs"
        );
    }

    #[tokio::test]
    async fn inputs_work_for_implicit_zero_arg_main() {
        let source = "1 + 2";
        let program = parse_rex(source).expect("parse");

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock is before unix epoch")
            .as_nanos();
        let inputs = std::env::temp_dir().join(format!("rex-implicit-main-inputs-{nonce}.json"));
        std::fs::write(&inputs, r#"{}"#).expect("write inputs");

        let actual = eval_result_json(&program, None, Some(&inputs.to_string_lossy()))
            .await
            .expect("eval implicit main");
        let manifest = main_manifest(&program, None)
            .await
            .expect("implicit manifest");
        let manifest = serde_json::to_value(manifest).expect("manifest json");
        let _ = std::fs::remove_file(&inputs);

        assert_eq!(actual, serde_json::json!(3));
        assert!(manifest.get("parameters").is_none());
        assert!(manifest.pointer("/typeBundle/types/result").is_some());
    }
}
