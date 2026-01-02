#![forbid(unsafe_code)]

use std::fs;
use std::thread;

use clap::{Args, Parser, Subcommand};
use rex_ast::expr::Decl;
use rex_engine::Engine;
use rex_lexer::Token;
use rex_parser::Parser as RexParser;
use rex_ts::TypeSystem;

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
    #[arg(value_name = "FILE", required_unless_present = "code", conflicts_with = "code")]
    file: Option<String>,

    /// Inline Rex source code to run.
    #[arg(
        short = 'c',
        long = "code",
        value_name = "CODE",
        required_unless_present = "file",
        conflicts_with = "file"
    )]
    code: Option<String>,

    /// Print the parsed AST and exit.
    #[arg(long = "emit-ast")]
    emit_ast: bool,

    /// Print the inferred type and exit.
    #[arg(long = "type")]
    emit_type: bool,
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
        emit_ast,
        emit_type,
    } = args;

    let source = if let Some(code) = code {
        code
    } else if let Some(path) = file {
        fs::read_to_string(&path).map_err(|e| format!("failed to read `{path}`: {e}"))?
    } else {
        return Err("missing input (file or `-c/--code`)".into());
    };

    // Rex programs can be deeply nested (especially after desugaring). Run on a
    // slightly larger stack to reduce overflow risk in parser/type inference/eval.
    const STACK_SIZE: usize = 16 * 1024 * 1024;
    let handle = thread::Builder::new()
        .name("rex-run".to_string())
        .stack_size(STACK_SIZE)
        .spawn(move || run_source(&source, emit_ast, emit_type))
        .map_err(|e| format!("failed to spawn runner thread: {e}"))?;

    match handle.join() {
        Ok(res) => res,
        Err(_) => Err("runner thread panicked".into()),
    }
}

fn run_source(source: &str, emit_ast: bool, emit_type: bool) -> Result<(), String> {
    let tokens = Token::tokenize(source).map_err(|e| format!("lex error: {e}"))?;
    let mut parser = RexParser::new(tokens);
    let program = parser
        .parse_program()
        .map_err(|errs| format_parse_errors(&errs))?;

    if emit_ast {
        println!("{program:#?}");
    }

    if emit_type {
        let mut ts = TypeSystem::with_prelude();
        inject_type_env_decls(&mut ts, &program.decls).map_err(|e| format!("{e}"))?;
        let (preds, ty) = ts.infer(program.expr.as_ref()).map_err(|e| format!("{e}"))?;
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
    engine
        .inject_decls(&program.decls)
        .map_err(|e| format!("{e}"))?;
    let value = engine.eval(program.expr.as_ref()).map_err(|e| format!("{e}"))?;
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
