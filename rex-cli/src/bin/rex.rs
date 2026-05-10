#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use std::fs;
use std::io::IsTerminal;
use std::io::{self, Read};
use std::sync::Arc;

use clap::{Args, Parser};
use rex::{
    ast::CompilationUnit,
    engine::{Engine, ImportRequest, Importer},
    json::rex_to_json,
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

    /// Treat the input file as a snippet instead of a module.
    #[arg(long = "snippet", requires = "file")]
    snippet: bool,

    /// Print the parsed AST as JSON and exit.
    #[arg(long = "emit-ast")]
    emit_ast: bool,

    /// Print the inferred type as JSON and exit.
    #[arg(long = "emit-type", alias = "type")]
    emit_type: bool,

    /// Additional module include roots (searched after local-relative imports).
    #[arg(long = "include", value_name = "DIR")]
    include: Vec<String>,

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
        snippet,
        emit_ast,
        emit_type,
        include,
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
            snippet,
            include,
            emit_ast,
            emit_type,
        },
    )
    .await
}

struct RunSourceOpts {
    file: Option<String>,
    snippet: bool,
    include: Vec<String>,
    emit_ast: bool,
    emit_type: bool,
}

fn init_engine(include: &[String]) -> Result<(Engine, Arc<dyn Importer>), String> {
    let mut engine =
        Engine::with_prelude(()).map_err(|e| format!("failed to initialize engine: {e}"))?;
    cli_prelude::inject_cli_prelude_engine(&mut engine).map_err(|e| e.to_string())?;
    let mut filesystem_importer = FilesystemImporter::new();
    for root in include {
        filesystem_importer
            .add_include_root(root)
            .map_err(|e| e.to_string())?;
    }
    let importer: Arc<dyn Importer> = Arc::new(filesystem_importer);
    engine.add_importer_arc("filesystem", Arc::clone(&importer));
    Ok((engine, importer))
}

async fn run_source(source: &str, opts: RunSourceOpts) -> Result<(), String> {
    let RunSourceOpts {
        file,
        snippet,
        include,
        emit_ast,
        emit_type,
    } = opts;

    let program = parse_rex(source).map_err(|errs| format_parse_errors(&errs))?;

    if emit_ast || emit_type {
        let type_json = if emit_type {
            Some(infer_type_json(source, file.as_deref(), snippet, &include).await?)
        } else {
            None
        };
        let out = emit_json(&program, emit_ast, type_json)?;
        println!("{out}");
        return Ok(());
    }

    let result_json = eval_result_json(source, file.as_deref(), snippet, &include).await?;
    let rendered = render_result_json(&result_json)?;
    println!("{rendered}");
    Ok(())
}

async fn eval_result_json(
    source: &str,
    file: Option<&str>,
    snippet: bool,
    include: &[String],
) -> Result<serde_json::Value, String> {
    let (engine, importer) = init_engine(include)?;
    let mut compiler = engine.into_compiler();

    let program = if let Some(path) = file {
        if snippet {
            compiler
                .compile_snippet_at(source, path)
                .await
                .map_err(|e| format!("{e}"))?
        } else {
            compiler
                .compile_module_with_importer(ImportRequest::new(path), importer)
                .await
                .map_err(|e| format!("{e}"))?
        }
    } else {
        compiler
            .compile_snippet(source)
            .await
            .map_err(|e| format!("{e}"))?
    };

    let result_type = program.result_type().clone();
    let evaluator = compiler.into_evaluator();
    let type_system = evaluator.type_system();
    let value = evaluator.run(program).await.map_err(|e| format!("{e}"))?;

    rex_to_json(&value, &result_type, type_system.as_ref())
        .map_err(|e| format!("failed to convert result to JSON: {e}"))
}

fn render_result_json(value: &serde_json::Value) -> Result<String, String> {
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
    source: &str,
    file: Option<&str>,
    snippet: bool,
    include: &[String],
) -> Result<serde_json::Value, String> {
    let (mut engine, importer) = init_engine(include)?;

    let (preds, ty) = if let Some(path) = file {
        if snippet {
            engine
                .infer_snippet_at(source, path)
                .await
                .map_err(|e| format!("{e}"))?
        } else {
            engine
                .infer_module_with_importer(ImportRequest::new(path), importer)
                .await
                .map_err(|e| format!("{e}"))?
        }
    } else {
        engine
            .infer_snippet(source)
            .await
            .map_err(|e| format!("{e}"))?
    };

    let constraints = preds
        .iter()
        .map(|p| {
            json!({
                "class": p.class.to_string(),
                "type": p.typ.to_string(),
            })
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "type": ty.to_string(),
        "constraints": constraints,
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

    #[tokio::test]
    async fn emit_ast_and_type_are_json() {
        let source = "1 + 2";
        let program = parse_rex(source).expect("parse");

        let ty_json = infer_type_json(source, None, false, &[])
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

        let value = eval_result_json(source, None, false, &[])
            .await
            .expect("eval result json");
        assert_eq!(value, serde_json::json!({ "Rect": [2, 3] }));

        let rendered = render_result_json(&value).expect("render result json");
        assert_eq!(rendered, "{\n  \"Rect\": [\n    2,\n    3\n  ]\n}");
    }

    #[tokio::test]
    async fn emit_type_resolves_imports() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock is before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("rex-import-test-{nonce}"));
        std::fs::create_dir_all(root.join("foo")).expect("create temp module dir");

        std::fs::write(
            root.join("foo/bar.rex"),
            r#"
                pub fn add : i32 -> i32 -> i32 = \x y -> x + y;
                pub fn triple : i32 -> i32 = \x -> x * 3;
            "#,
        )
        .expect("write bar.rex");
        let source = r#"
            import foo.bar;

            bar.add (bar.triple 10) 2
        "#;
        let include = vec![root.to_string_lossy().to_string()];
        let json = infer_type_json(source, None, false, &include)
            .await
            .expect("infer");
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(json.get("type").and_then(|v| v.as_str()), Some("i32"));
    }

    #[tokio::test]
    async fn emit_type_file_snippet_uses_file_as_import_base() {
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

        let module_err = infer_type_json(source, Some(&main), false, &[])
            .await
            .expect_err("module");
        assert!(
            module_err.contains("declaration-only"),
            "unexpected error: {module_err}"
        );

        let json = infer_type_json(source, Some(&main), true, &[])
            .await
            .expect("infer snippet file");
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(json.get("type").and_then(|v| v.as_str()), Some("i32"));
    }
}
