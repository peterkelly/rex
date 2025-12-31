use std::fs;
use std::path::{Path, PathBuf};
use std::thread;

use rex_ast::expr::Decl;
use rex_engine::Engine;
use rex_lexer::Token;
use rex_parser::Parser;
use rex_ts::TypeSystem;

fn example_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(name)
}

fn format_parse_errors(errs: &[rex_parser::error::ParserErr]) -> String {
    let mut out = String::from("parse error:");
    for err in errs {
        out.push_str(&format!("\n  {err}"));
    }
    out
}

fn inject_type_decls_ts(ts: &mut TypeSystem, decls: &[Decl]) -> Result<(), rex_ts::TypeError> {
    for decl in decls {
        let Decl::Type(ty) = decl;
        ts.inject_type_decl(ty)?;
    }
    Ok(())
}

fn inject_type_decls_engine(
    engine: &mut Engine,
    decls: &[Decl],
) -> Result<(), rex_engine::EngineError> {
    for decl in decls {
        let Decl::Type(ty) = decl;
        engine.inject_type_decl(ty)?;
    }
    Ok(())
}

fn assert_example_ok(name: &str) {
    let name = name.to_string();
    let handle = thread::Builder::new()
        .name(format!("example-{name}"))
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            let path = example_path(&name);
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));

            let tokens = Token::tokenize(&source)
                .unwrap_or_else(|err| panic!("lex error in {}: {err}", path.display()));
            let mut parser = Parser::new(tokens);
            let program = parser.parse_program().unwrap_or_else(|errs| {
                panic!(
                    "parse error in {}:\n{}",
                    path.display(),
                    format_parse_errors(&errs)
                )
            });

            let mut ts = TypeSystem::with_prelude();
            inject_type_decls_ts(&mut ts, &program.decls)
                .unwrap_or_else(|err| panic!("type decl error in {}: {err}", path.display()));
            ts.infer(program.expr.as_ref())
                .unwrap_or_else(|err| panic!("type error in {}: {err}", path.display()));

            let mut engine = Engine::with_prelude();
            inject_type_decls_engine(&mut engine, &program.decls).unwrap_or_else(|err| {
                panic!("engine type decl error in {}: {err}", path.display())
            });
            engine
                .eval(program.expr.as_ref())
                .unwrap_or_else(|err| panic!("eval error in {}: {err}", path.display()));
        })
        .unwrap();
    handle.join().expect("example thread panicked");
}

#[test]
fn example_adt() {
    assert_example_ok("adt.rex");
}

#[test]
fn example_lots_of_lambdas() {
    assert_example_ok("lots_of_lambdas.rex");
}

#[test]
fn example_lots_of_lets() {
    assert_example_ok("lots_of_lets.rex");
}

#[test]
fn example_mega() {
    assert_example_ok("mega.rex");
}

#[test]
fn example_complex() {
    assert_example_ok("complex.rex");
}
