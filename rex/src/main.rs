use std::env;
use std::fs;

use rex_engine::Engine;
use rex_lexer::Token;
use rex_parser::Parser;

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let cmd = match args.next() {
        Some(cmd) => cmd,
        None => {
            print_usage();
            return Ok(());
        }
    };

    match cmd.as_str() {
        "run" => run_cmd(args),
        "-h" | "--help" => {
            print_usage();
            Ok(())
        }
        other => Err(format!("unknown command `{other}`")),
    }
}

fn run_cmd(mut args: impl Iterator<Item = String>) -> Result<(), String> {
    let mut code: Option<String> = None;
    let mut file: Option<String> = None;
    let mut emit_ast = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--emit-ast" => {
                emit_ast = true;
            }
            "-c" | "--code" => {
                if code.is_some() {
                    return Err("`-c/--code` provided more than once".into());
                }
                code = Some(
                    args.next()
                        .ok_or_else(|| "missing code after `-c/--code`".to_string())?,
                );
            }
            "-h" | "--help" => {
                print_run_usage();
                return Ok(());
            }
            _ => {
                if file.is_some() {
                    return Err(format!("unexpected extra argument `{arg}`"));
                }
                file = Some(arg);
            }
        }
    }

    if code.is_some() && file.is_some() {
        return Err("provide either a file or `-c/--code`, not both".into());
    }

    let source = if let Some(code) = code {
        code
    } else if let Some(path) = file {
        fs::read_to_string(&path)
            .map_err(|e| format!("failed to read `{path}`: {e}"))?
    } else {
        return Err("missing input (file or `-c/--code`)".into());
    };

    let tokens = Token::tokenize(&source).map_err(|e| format!("lex error: {e}"))?;
    let mut parser = Parser::new(tokens);
    let expr = parser
        .parse_program()
        .map_err(|errs| format_parse_errors(&errs))?;

    if emit_ast {
        println!("{expr:#?}");
        return Ok(());
    }

    let mut engine = Engine::with_prelude();
    let value = engine.eval(expr.as_ref()).map_err(|e| format!("{e}"))?;
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

fn print_usage() {
    eprintln!(
        "Usage:\n  rex run <file>\n  rex run -c <code>\n\nRun with -h/--help for more."
    );
}

fn print_run_usage() {
    eprintln!(
        "Usage:\n  rex run <file>\n  rex run -c <code>\n\nOptions:\n  --emit-ast   Print the parsed AST and exit"
    );
}
