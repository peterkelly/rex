#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use std::collections::BTreeSet;
use std::fs;
use std::io::IsTerminal;
use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::Arc;

use clap::{Args, Parser};
use rex::{
    ast::{CompilationUnit, Expr, FnDecl, Span, Var},
    engine::{Context, Engine, Importer, Module, ModuleId},
    json::{json_to_rex, rex_to_json},
    parser::{ParseError, parse as parse_rex},
    typesystem::{Scheme, Type, TypeKind},
};
use serde_json::json;

use rex_cli::cli_prelude;
use rex_cli::filesystem_importer::FilesystemImporter;
use rex_cli::manifest::{Manifest, build_manifest, type_has_vars};

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
    let main_decl = program.get_fn_decl("main");

    if manifest {
        let manifest = main_manifest(&program, main_decl, file.as_deref()).await?;
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
            Some(infer_type_json(&program, main_decl, file.as_deref()).await?)
        } else {
            None
        };
        let out = emit_json(&program, emit_ast, type_json)?;
        println!("{out}");
        return Ok(());
    }

    let result_json =
        eval_result_json(&program, main_decl, file.as_deref(), inputs.as_deref()).await?;
    let rendered = render_result_json(&result_json, raw_output)?;
    println!("{rendered}");
    Ok(())
}

async fn eval_result_json(
    program: &CompilationUnit,
    main_decl: Option<&FnDecl>,
    file: Option<&str>,
    inputs_path: Option<&str>,
) -> Result<serde_json::Value, String> {
    let signature = inspect_main_signature(program, main_decl, file).await?;
    ensure_concrete_inputs(&signature.params)?;
    let input_value = match inputs_path {
        Some(path) => {
            let raw =
                fs::read_to_string(path).map_err(|e| format!("failed to read `{path}`: {e}"))?;
            serde_json::from_str(&raw)
                .map_err(|e| format!("failed to parse input JSON `{path}`: {e}"))?
        }
        None if main_decl.is_some() => {
            return Err("program defines `main`; pass `--inputs <JSON>`".to_string());
        }
        None => serde_json::json!({}),
    };
    let inputs = read_main_inputs(input_value, &signature)?;

    let (mut engine, _importer) = init_engine()?;
    inject_input_values(&mut engine, inputs)?;
    let mut compiler = engine.into_compiler();
    let call_program = program_with_body(program, signature.body);
    let importer_path = file.map(PathBuf::from);
    let prefix_source = snippet_prefix_source(importer_path.as_ref());

    let compiled = compiler
        .compile_snippet_program_with_importer_and_prefix(
            &call_program,
            importer_path,
            Some(prefix_source),
        )
        .await
        .map_err(|e| format!("{e}"))?;

    let result_type = compiled.result_type().clone();
    let evaluator = compiler.into_evaluator();
    let type_system = evaluator.type_system();
    let value = evaluator.run(compiled).await.map_err(|e| format!("{e}"))?;

    rex_to_json(&value, &result_type, type_system.as_ref())
        .map_err(|e| format!("failed to convert result to JSON: {e}"))
}

async fn main_manifest(
    program: &CompilationUnit,
    main_decl: Option<&FnDecl>,
    file: Option<&str>,
) -> Result<Manifest, String> {
    let signature = inspect_main_signature(program, main_decl, file).await?;
    Ok(signature.manifest)
}

struct MainSignature {
    params: Vec<MainParam>,
    input_symbols: Vec<String>,
    body: Arc<Expr>,
    result_type: Type,
    manifest: Manifest,
}

struct MainParam {
    name: String,
    typ: Type,
}

struct MainInput {
    symbol: String,
    typ: Type,
    value: serde_json::Value,
}

async fn inspect_main_signature(
    program: &CompilationUnit,
    main_decl: Option<&FnDecl>,
    file: Option<&str>,
) -> Result<MainSignature, String> {
    if main_decl.is_some() && program.body.is_some() {
        return Err(
            "program defines `main` and also has a final expression; remove one entry point".into(),
        );
    }

    let (engine, _importer) = init_engine()?;
    let mut compiler = engine.into_compiler();
    let signature_body = signature_body_expr(program, main_decl);
    let main_program = program_with_body(program, Arc::clone(&signature_body));
    let importer_path = file.map(PathBuf::from);
    let prefix_source = snippet_prefix_source(importer_path.as_ref());
    let compiled = compiler
        .compile_snippet_program_with_importer_and_prefix(
            &main_program,
            importer_path,
            Some(prefix_source),
        )
        .await
        .map_err(|e| format!("{e}"))?;

    let main_type = compiled.result_type().clone();
    let (param_types, result_type) = if main_decl.is_some() {
        decompose_fun_type(&main_type)
    } else {
        (Vec::new(), main_type)
    };
    let params = main_params(main_decl, param_types)?;

    let input_symbols = params
        .iter()
        .enumerate()
        .map(|(idx, _)| format!("@rex.cli.input.{idx}"))
        .collect::<Vec<_>>();
    let body = if main_decl.is_some() {
        main_call_expr(&input_symbols)
    } else {
        signature_body
    };
    let manifest = build_manifest(
        params.iter().map(|param| (param.name.as_str(), &param.typ)),
        &result_type,
        compiler.type_system(),
    )?;

    Ok(MainSignature {
        params,
        input_symbols,
        body,
        result_type,
        manifest,
    })
}

fn read_main_inputs(
    value: serde_json::Value,
    signature: &MainSignature,
) -> Result<Vec<MainInput>, String> {
    let inputs = value.as_object().ok_or_else(|| {
        "input JSON must be an object whose fields are parameter names".to_string()
    })?;

    let expected = signature
        .params
        .iter()
        .map(|param| param.name.as_str())
        .collect::<BTreeSet<_>>();
    let actual = inputs.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if expected != actual {
        let missing = expected
            .difference(&actual)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        let extra = actual
            .difference(&expected)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "inputs do not match entry point parameters (missing: [{}], extra: [{}])",
            missing, extra
        ));
    }

    signature
        .params
        .iter()
        .zip(&signature.input_symbols)
        .map(|(param, symbol)| {
            let value = inputs
                .get(&param.name)
                .ok_or_else(|| format!("missing input `{}`", param.name))?
                .clone();
            Ok(MainInput {
                symbol: symbol.clone(),
                typ: param.typ.clone(),
                value,
            })
        })
        .collect()
}

fn inject_input_values(engine: &mut Engine, inputs: Vec<MainInput>) -> Result<(), String> {
    let mut module = Module::global();
    for input in inputs {
        let MainInput { symbol, typ, value } = input;
        let scheme = Scheme::new(vec![], vec![], typ.clone());
        module
            .export_native(
                symbol,
                scheme,
                0,
                move |ctx: Context<()>, _typ: &Type, _args| {
                    json_to_rex(ctx.heap(), &value, &typ, ctx.type_system())
                },
            )
            .map_err(|e| format!("{e}"))?;
    }
    engine.inject_module(module).map_err(|e| format!("{e}"))
}

fn ensure_concrete_inputs(params: &[MainParam]) -> Result<(), String> {
    for param in params {
        if type_has_vars(&param.typ) {
            return Err(format!(
                "`main` parameter `{}` has polymorphic type `{}`; JSON inputs require concrete parameter types",
                param.name, param.typ
            ));
        }
    }
    Ok(())
}

fn signature_body_expr(program: &CompilationUnit, main_decl: Option<&FnDecl>) -> Arc<Expr> {
    if main_decl.is_some() {
        Arc::new(Expr::Var(Var::new("main")))
    } else {
        program.body.clone().unwrap_or_else(unit_expr)
    }
}

fn main_params(
    main_decl: Option<&FnDecl>,
    param_types: Vec<Type>,
) -> Result<Vec<MainParam>, String> {
    let Some(main_decl) = main_decl else {
        if param_types.is_empty() {
            return Ok(Vec::new());
        }
        return Err(format!(
            "implicit `main` should have 0 argument(s), but its inferred type has {}",
            param_types.len()
        ));
    };

    if param_types.len() != main_decl.params.len() {
        return Err(format!(
            "`main` declares {} parameter(s), but its inferred type has {} argument(s)",
            main_decl.params.len(),
            param_types.len()
        ));
    }

    let mut seen = BTreeSet::new();
    main_decl
        .params
        .iter()
        .zip(param_types)
        .map(|((var, _), typ)| {
            let name = var.name.to_string();
            if !seen.insert(name.clone()) {
                return Err(format!("duplicate `main` parameter `{name}`"));
            }
            Ok(MainParam { name, typ })
        })
        .collect()
}

fn program_with_body(program: &CompilationUnit, body: Arc<Expr>) -> CompilationUnit {
    CompilationUnit {
        decls: program.decls.clone(),
        body: Some(body),
    }
}

fn unit_expr() -> Arc<Expr> {
    Arc::new(Expr::Tuple(Span::default(), Vec::new()))
}

fn snippet_prefix_source(importer_path: Option<&PathBuf>) -> ModuleId {
    importer_path
        .map(|path| ModuleId::Local { path: path.clone() })
        .unwrap_or_else(|| ModuleId::Virtual("__snippet__".to_string()))
}

fn main_call_expr(input_symbols: &[String]) -> Arc<Expr> {
    let mut expr = Arc::new(Expr::Var(Var::new("main")));
    for symbol in input_symbols {
        expr = Arc::new(Expr::App(
            Span::default(),
            expr,
            Arc::new(Expr::Var(Var::new(symbol))),
        ));
    }
    expr
}

fn decompose_fun_type(typ: &Type) -> (Vec<Type>, Type) {
    let mut params = Vec::new();
    let mut cur = typ.clone();
    while let TypeKind::Fun(param, ret) = cur.as_ref() {
        params.push(param.clone());
        cur = ret.clone();
    }
    (params, cur)
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
    main_decl: Option<&FnDecl>,
    file: Option<&str>,
) -> Result<serde_json::Value, String> {
    let signature = inspect_main_signature(program, main_decl, file).await?;

    Ok(json!({
        "type": signature.result_type.to_string(),
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
        let main_decl = program.get_fn_decl("main");

        let ty_json = infer_type_json(&program, main_decl, None)
            .await
            .expect("infer");
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
        let main_decl = program.get_fn_decl("main");
        let value = eval_result_json(&program, main_decl, None, None)
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
        let main_decl = program.get_fn_decl("main");

        let json = infer_type_json(&program, main_decl, Some(&main))
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
        let main_decl = program.get_fn_decl("main").expect("main decl");

        let json = infer_type_json(&program, Some(main_decl), None)
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
        let main_decl = program.get_fn_decl("main");

        let err = infer_type_json(&program, main_decl, None)
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
        let main_decl = program.get_fn_decl("main").expect("main decl");

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock is before unix epoch")
            .as_nanos();
        let inputs = std::env::temp_dir().join(format!("rex-main-inputs-{nonce}.json"));
        std::fs::write(&inputs, r#"{ "y": 5, "x": 37 }"#).expect("write inputs");

        let actual = eval_result_json(
            &program,
            Some(main_decl),
            None,
            Some(&inputs.to_string_lossy()),
        )
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
        let main_decl = program.get_fn_decl("main").expect("main decl");

        let manifest = main_manifest(&program, Some(main_decl), None)
            .await
            .expect("manifest");
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

        let actual = eval_result_json(&program, None, None, Some(&inputs.to_string_lossy()))
            .await
            .expect("eval implicit main");
        let manifest = main_manifest(&program, None, None)
            .await
            .expect("implicit manifest");
        let manifest = serde_json::to_value(manifest).expect("manifest json");
        let _ = std::fs::remove_file(&inputs);

        assert_eq!(actual, serde_json::json!(3));
        assert!(manifest.get("parameters").is_none());
        assert!(manifest.pointer("/typeBundle/types/result").is_some());
    }
}
