#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

//! Parsing for Rex.
//!
//! The parser is a token-level PEG parser written directly in Rust. It keeps
//! ordered-choice decisions and semantic actions explicit so it remains
//! straightforward to step through in a debugger.

pub mod error;
#[doc(hidden)]
pub mod lexer;
pub mod op;

mod ast_builder;
mod grammar;
mod macros;
mod peg;
mod rex;
// The `.peg` file parser is a test-time verifier for checked grammar specs.
// Runtime Rex parsing uses the Rust data structure in `rex.rs` directly.
#[cfg(test)]
mod peg_syntax;

use rex_ast::expr::CompilationUnit;

use crate::{
    error::ParseError,
    lexer::{Token, Tokens},
};

#[doc(hidden)]
pub const MAX_AST_DEPTH: usize = 256;

pub fn parse(input: &str) -> Result<CompilationUnit, Vec<ParseError>> {
    let tokens = Token::tokenize(input).map_err(|err| vec![ParseError::from_lexical_error(err)])?;
    parse_with_tokens(tokens)
}

#[doc(hidden)]
pub fn parse_with_tokens(tokens: Tokens) -> Result<CompilationUnit, Vec<ParseError>> {
    let mut parser = ast_builder::PegParser::new(tokens);
    parser.parse_program()
}
