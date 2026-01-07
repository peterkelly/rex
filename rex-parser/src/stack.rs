use std::thread;

use rex_ast::expr::Program;
use rex_lexer::span::Span;

use crate::{error::ParserErr, Parser};

pub const DEFAULT_STACK_SIZE_BYTES: usize = 16 * 1024 * 1024;

pub fn parse_program_with_stack_size(
    parser: Parser,
    stack_size: usize,
) -> Result<Program, Vec<ParserErr>> {
    let handle = thread::Builder::new()
        .name("rex-parser".to_string())
        .stack_size(stack_size)
        .spawn(move || {
            let mut parser = parser;
            parser.parse_program()
        })
        .map_err(|e| {
            vec![ParserErr::new(
                Span::default(),
                format!("internal error: {e}"),
            )]
        })?;
    handle
        .join()
        .map_err(|_| vec![ParserErr::new(Span::default(), "parser thread panicked")])?
}
