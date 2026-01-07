#![forbid(unsafe_code)]

use std::fs;
use std::io::{self, Read};
use std::thread;

use clap::{Args, Parser, Subcommand};
use rex_ast::expr::Decl;
use rex_engine::Engine;
use rex_gas::{GasCosts, GasMeter};
use rex_lexer::Token;
use rex_parser::{Parser as RexParser, ParserLimits};
use rex_ts::TypeSystem;

mod cli_prelude;

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

    /// Print the parsed AST and exit.
    #[arg(long = "emit-ast")]
    emit_ast: bool,

    /// Print the inferred type and exit.
    #[arg(long = "type")]
    emit_type: bool,

    /// Stack size (in MiB) used for parsing/type inference/evaluation.
    #[arg(long = "stack-size-mb", default_value_t = 16)]
    stack_size_mb: usize,

    /// Maximum nesting depth allowed during parsing (defaults to a safe limit).
    #[arg(long = "max-nesting", value_name = "N", conflicts_with = "no_max_nesting")]
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

fn main() {
    let cli = Cli::parse();
    if let Err(err) = run(cli) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Command::Run(args) => run_cmd(args),
    }
}

fn run_cmd(args: RunArgs) -> Result<(), String> {
    let RunArgs {
        file,
        code,
        stdin,
        emit_ast,
        emit_type,
        stack_size_mb,
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
    } else if let Some(path) = file {
        fs::read_to_string(&path).map_err(|e| format!("failed to read `{path}`: {e}"))?
    } else {
        return Err("missing input (file or `-c/--code`)".into());
    };

    // Rex programs can be deeply nested (especially after desugaring). Run on a
    // slightly larger stack to reduce overflow risk in parser/type inference/eval.
    let stack_size = stack_size_mb
        .checked_mul(1024 * 1024)
        .ok_or_else(|| "stack size overflow".to_string())?;

    let parser_limits = if no_max_nesting {
        ParserLimits::unlimited()
    } else if let Some(max_nesting) = max_nesting {
        ParserLimits {
            max_nesting: Some(max_nesting),
        }
    } else {
        ParserLimits::safe_defaults()
    };

    let handle = thread::Builder::new()
        .name("rex-run".to_string())
        .stack_size(stack_size)
        .spawn(move || run_source(&source, emit_ast, emit_type, gas, no_gas, parser_limits))
        .map_err(|e| format!("failed to spawn runner thread: {e}"))?;

    match handle.join() {
        Ok(res) => res,
        Err(_) => Err("runner thread panicked".into()),
    }
}

fn run_source(
    source: &str,
    emit_ast: bool,
    emit_type: bool,
    gas: u64,
    no_gas: bool,
    parser_limits: ParserLimits,
) -> Result<(), String> {
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
        .parse_program_with_gas(&mut gas)
        .map_err(|errs| format_parse_errors(&errs))?;

    if emit_ast {
        println!("{program:#?}");
    }

    if emit_type {
        let mut ts = TypeSystem::with_prelude();
        cli_prelude::inject_cli_prelude_schemes(&mut ts);
        inject_type_env_decls(&mut ts, &program.decls).map_err(|e| format!("{e}"))?;
        let (preds, ty) = ts
            .infer_with_gas(program.expr.as_ref(), &mut gas)
            .map_err(|e| format!("{e}"))?;
        if preds.is_empty() {
            println!("{ty}");
        } else {
            let constraints = preds
                .iter()
                .map(|p| format!("{} {}", p.class, p.typ))
                .collect::<Vec<_>>()
                .join(", ");
            println!("type: {ty}");
            println!("constraints: {constraints}");
        }
    }

    if emit_ast || emit_type {
        return Ok(());
    }

    let mut engine = Engine::with_prelude();
    cli_prelude::inject_cli_prelude_engine(&mut engine).map_err(|e| format!("{e}"))?;
    engine
        .inject_decls(&program.decls)
        .map_err(|e| format!("{e}"))?;
    let value = engine
        .eval_with_gas(program.expr.as_ref(), &mut gas)
        .map_err(|e| format!("{e}"))?;
    println!("{value}");
    Ok(())
}

fn format_parse_errors(errs: &[rex_parser::error::ParserErr]) -> String {
    let mut out = String::from("parse error:");
    for err in errs {
        out.push_str(&format!("\n  {err}"));
    }
    out
}

fn inject_type_env_decls(ts: &mut TypeSystem, decls: &[Decl]) -> Result<(), rex_ts::TypeError> {
    for decl in decls {
        match decl {
            Decl::Type(ty) => ts.inject_type_decl(ty)?,
            Decl::Class(class_decl) => ts.inject_class_decl(class_decl)?,
            Decl::Instance(inst_decl) => {
                ts.inject_instance_decl(inst_decl)?;
            }
            Decl::Fn(fd) => ts.inject_fn_decl(fd)?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_shape_is_stable() {
        Cli::command().debug_assert();
    }
}
