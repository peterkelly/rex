#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use std::fs;
use std::io::IsTerminal;
use std::io::{self, BufRead, Read, Write};

use clap::{Args, Parser, Subcommand};
use rex::{
    Engine, GasCosts, GasMeter, Parser as RexParser, ParserErr, ParserLimits, Program, ReplState,
    Token, ValueDisplayOptions, pointer_display_with,
};
use serde_json::json;

use rex_cli::cli_prelude;

#[derive(Parser)]
#[command(name = "rex")]
#[command(about = "Rex (Rush Expressions) CLI")]
#[command(arg_required_else_help = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Run(RunArgs),
    Repl(ReplArgs),
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

    /// Additional module include roots (searched after local-relative imports).
    #[arg(long = "include", value_name = "DIR")]
    include: Vec<String>,

    /// Stack size (in MiB) used for parsing/type inference/evaluation.
    #[arg(long = "stack-size-mb", default_value_t = 16)]
    stack_size_mb: usize,

    /// Maximum nesting depth allowed during parsing (defaults to a safe limit).
    #[arg(
        long = "max-nesting",
        value_name = "N",
        conflicts_with = "no_max_nesting"
    )]
    max_nesting: Option<usize>,

    /// Disable the parsing nesting-depth limit.
    #[arg(long = "no-max-nesting")]
    no_max_nesting: bool,

    /// Gas budget (in abstract units) for parsing + type inference + evaluation.
    #[arg(long = "gas", default_value_t = 10_000_000)]
    gas: u64,

    /// Disable gas metering.
    #[arg(long = "no-gas")]
    no_gas: bool,
}

#[derive(Args)]
struct ReplArgs {
    /// Additional module include roots (searched after local-relative imports).
    #[arg(long = "include", value_name = "DIR")]
    include: Vec<String>,

    /// Stack size (in MiB) used for parsing/type inference/evaluation.
    #[arg(long = "stack-size-mb", default_value_t = 16)]
    stack_size_mb: usize,

    /// Maximum nesting depth allowed during parsing (defaults to a safe limit).
    #[arg(
        long = "max-nesting",
        value_name = "N",
        conflicts_with = "no_max_nesting"
    )]
    max_nesting: Option<usize>,

    /// Disable the parsing nesting-depth limit.
    #[arg(long = "no-max-nesting")]
    no_max_nesting: bool,

    /// Gas budget (in abstract units) for parsing + type inference + evaluation (per input).
    #[arg(long = "gas", default_value_t = 10_000_000)]
    gas: u64,

    /// Disable gas metering.
    #[arg(long = "no-gas")]
    no_gas: bool,
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
    match cli.command {
        Command::Run(args) => run_cmd(args).await,
        Command::Repl(args) => repl_cmd(args).await,
    }
}

async fn run_cmd(args: RunArgs) -> Result<(), String> {
    let RunArgs {
        file,
        code,
        stdin,
        emit_ast,
        emit_type,
        include,
        stack_size_mb: _stack_size_mb,
        max_nesting,
        no_max_nesting,
        gas,
        no_gas,
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

    let parser_limits = if no_max_nesting {
        ParserLimits::unlimited()
    } else if let Some(max_nesting) = max_nesting {
        ParserLimits {
            max_nesting: Some(max_nesting),
        }
    } else {
        ParserLimits::safe_defaults()
    };

    run_source(
        &source,
        RunSourceOpts {
            file,
            include,
            emit_ast,
            emit_type,
            gas,
            no_gas,
            parser_limits,
        },
    )
    .await
}

async fn repl_cmd(args: ReplArgs) -> Result<(), String> {
    let ReplArgs {
        include,
        stack_size_mb: _stack_size_mb,
        max_nesting,
        no_max_nesting,
        gas,
        no_gas,
    } = args;

    let parser_limits = if no_max_nesting {
        ParserLimits::unlimited()
    } else if let Some(max_nesting) = max_nesting {
        ParserLimits {
            max_nesting: Some(max_nesting),
        }
    } else {
        ParserLimits::safe_defaults()
    };

    repl_loop(include, gas, no_gas, parser_limits).await
}

async fn repl_loop(
    include: Vec<String>,
    gas_budget: u64,
    no_gas: bool,
    parser_limits: ParserLimits,
) -> Result<(), String> {
    let mut engine =
        Engine::with_prelude(()).map_err(|e| format!("failed to initialize engine: {e}"))?;
    engine.add_default_resolvers();
    cli_prelude::inject_cli_prelude_engine(&mut engine).map_err(|e| format!("{e}"))?;
    for root in include {
        engine
            .add_include_resolver(&root)
            .map_err(|e| format!("{e}"))?;
    }

    let mut state = ReplState::new();

    let interactive = io::stdin().is_terminal();
    let mut stdin = io::stdin().lock();
    let mut stderr = io::stderr().lock();

    let mut buffer = String::new();
    loop {
        if interactive {
            if buffer.is_empty() {
                write!(stderr, "rex> ").ok();
            } else {
                write!(stderr, "...> ").ok();
            }
            stderr.flush().ok();
        }

        let mut line = String::new();
        let n = stdin
            .read_line(&mut line)
            .map_err(|e| format!("failed to read input: {e}"))?;
        if n == 0 {
            if interactive {
                writeln!(stderr).ok();
            }
            break;
        }

        if buffer.is_empty() && line.trim().is_empty() {
            continue;
        }

        buffer.push_str(&line);
        let parse_source = if is_imports_only(&buffer) {
            format!("{buffer}\n()")
        } else {
            buffer.clone()
        };

        let costs = GasCosts::sensible_defaults();
        let mut gas = if no_gas {
            GasMeter::unlimited(costs)
        } else {
            GasMeter::new(Some(gas_budget), costs)
        };

        let tokens = match Token::tokenize(&parse_source) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("error: lex error: {e}");
                buffer.clear();
                continue;
            }
        };
        let mut parser = RexParser::new(tokens);
        parser.set_limits(parser_limits);
        let program = match parser.parse_program(&mut gas) {
            Ok(p) => p,
            Err(errs) => {
                let incomplete =
                    !errs.is_empty() && errs.iter().all(|e| e.message.trim() == "unexpected EOF");
                if incomplete {
                    continue;
                }
                eprintln!("{}", format_parse_errors(&errs));
                buffer.clear();
                continue;
            }
        };

        match rex::Evaluator::new_with_compiler(
            rex::RuntimeEnv::new(engine.clone()),
            rex::Compiler::new(engine.clone()),
        )
        .eval_repl_program(&program, &mut state, &mut gas)
        .await
        {
            Ok((v, _)) => {
                let rendered =
                    pointer_display_with(&engine.heap, &v, ValueDisplayOptions::unsanitized())
                        .unwrap_or_else(|e| format!("<display error: {e}>"));
                println!("{rendered}");
            }
            Err(e) => eprintln!("error: {e}"),
        }
        buffer.clear();
    }

    Ok(())
}

fn is_imports_only(buffer: &str) -> bool {
    let mut saw_import = false;
    for line in buffer.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("import ") || trimmed.starts_with("pub import ") {
            saw_import = true;
            continue;
        }
        return false;
    }
    saw_import
}

struct RunSourceOpts {
    file: Option<String>,
    include: Vec<String>,
    emit_ast: bool,
    emit_type: bool,
    gas: u64,
    no_gas: bool,
    parser_limits: ParserLimits,
}

fn init_engine(include: &[String]) -> Result<Engine, String> {
    let mut engine =
        Engine::with_prelude(()).map_err(|e| format!("failed to initialize engine: {e}"))?;
    engine.add_default_resolvers();
    cli_prelude::inject_cli_prelude_engine(&mut engine).map_err(|e| e.to_string())?;
    for root in include {
        engine
            .add_include_resolver(root)
            .map_err(|e| e.to_string())?;
    }
    Ok(engine)
}

async fn run_source(source: &str, opts: RunSourceOpts) -> Result<(), String> {
    let RunSourceOpts {
        file,
        include,
        emit_ast,
        emit_type,
        gas,
        no_gas,
        parser_limits,
    } = opts;
    let costs = GasCosts::sensible_defaults();
    let mut gas = if no_gas {
        GasMeter::unlimited(costs)
    } else {
        GasMeter::new(Some(gas), costs)
    };

    let tokens = Token::tokenize(source).map_err(|e| format!("lex error: {e}"))?;
    let mut parser = RexParser::new(tokens);
    parser.set_limits(parser_limits);
    let program = parser
        .parse_program(&mut gas)
        .map_err(|errs| format_parse_errors(&errs))?;

    if emit_ast || emit_type {
        let type_json = if emit_type {
            Some(infer_type_json(
                source,
                file.as_deref(),
                &include,
                &mut gas,
            )?)
        } else {
            None
        };
        let out = emit_json(&program, emit_ast, type_json)?;
        println!("{out}");
        return Ok(());
    }

    let engine = init_engine(&include)?;

    let (pointer, _) = if let Some(path) = file {
        rex::Evaluator::new_with_compiler(
            rex::RuntimeEnv::new(engine.clone()),
            rex::Compiler::new(engine.clone()),
        )
        .eval_module_file(&path, &mut gas)
        .await
        .map_err(|e| format!("{e}"))?
    } else {
        rex::Evaluator::new_with_compiler(
            rex::RuntimeEnv::new(engine.clone()),
            rex::Compiler::new(engine.clone()),
        )
        .eval_snippet(source, &mut gas)
        .await
        .map_err(|e| format!("{e}"))?
    };
    let rendered = pointer_display_with(&engine.heap, &pointer, ValueDisplayOptions::unsanitized())
        .unwrap_or_else(|e| format!("<display error: {e}>"));
    println!("{rendered}");
    Ok(())
}

fn emit_json(
    program: &Program,
    emit_ast: bool,
    type_json: Option<serde_json::Value>,
) -> Result<String, String> {
    match (emit_ast, type_json) {
        (true, None) => serde_json::to_string_pretty(program)
            .map_err(|e| format!("failed to serialize AST to JSON: {e}")),
        (false, Some(type_json)) => serde_json::to_string_pretty(&type_json)
            .map_err(|e| format!("failed to serialize type to JSON: {e}")),
        (true, Some(type_json)) => serde_json::to_string_pretty(&json!({
            "ast": program,
            "type": type_json,
        }))
        .map_err(|e| format!("failed to serialize outputs to JSON: {e}")),
        (false, None) => Err("internal error: emit_json called with no outputs".into()),
    }
}

fn infer_type_json(
    source: &str,
    file: Option<&str>,
    include: &[String],
    gas: &mut GasMeter,
) -> Result<serde_json::Value, String> {
    let mut engine = init_engine(include)?;

    let (preds, ty) = if let Some(path) = file {
        engine
            .infer_module_file(path, gas)
            .map_err(|e| format!("{e}"))?
    } else {
        engine
            .infer_snippet(source, gas)
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

fn format_parse_errors(errs: &[ParserErr]) -> String {
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
    fn emit_ast_and_type_are_json() {
        let source = "1 + 2";
        let tokens = Token::tokenize(source).expect("lex");
        let mut parser = RexParser::new(tokens);
        parser.set_limits(ParserLimits::safe_defaults());

        let costs = GasCosts::sensible_defaults();
        let mut gas = GasMeter::unlimited(costs);
        let program = parser.parse_program(&mut gas).expect("parse");

        let ty_json = infer_type_json(source, None, &[], &mut gas).expect("infer");
        let ast_out = emit_json(&program, true, None).expect("emit ast");
        let type_out = emit_json(&program, false, Some(ty_json.clone())).expect("emit type");
        let both_out = emit_json(&program, true, Some(ty_json)).expect("emit both");

        serde_json::from_str::<serde_json::Value>(&ast_out).expect("ast json");
        serde_json::from_str::<serde_json::Value>(&type_out).expect("type json");
        serde_json::from_str::<serde_json::Value>(&both_out).expect("both json");
    }

    #[test]
    fn emit_type_resolves_imports() {
        let costs = GasCosts::sensible_defaults();
        let mut gas = GasMeter::unlimited(costs);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock is before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("rex-import-test-{nonce}"));
        std::fs::create_dir_all(root.join("foo")).expect("create temp module dir");

        std::fs::write(
            root.join("foo/bar.rex"),
            r#"
                pub fn add : i32 -> i32 -> i32 = \x y -> x + y
                pub fn triple : i32 -> i32 = \x -> x * 3
            "#,
        )
        .expect("write bar.rex");
        let source = r#"
            import foo.bar

            bar.add (bar.triple 10) 2
        "#;
        let include = vec![root.to_string_lossy().to_string()];
        let json = infer_type_json(source, None, &include, &mut gas).expect("infer");
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(json.get("type").and_then(|v| v.as_str()), Some("i32"));
    }
}
