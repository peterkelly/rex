#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

//! Parsing for Rex.
//!
//! The parser is a token-level PEG parser written directly in Rust. It keeps
//! ordered-choice decisions and semantic actions explicit so it remains
//! straightforward to step through in a debugger.

pub mod error;
pub mod op;

mod ast_builder;
mod formal;
mod grammar;
mod peg;

use rex_ast::expr::Program;
use rex_lexer::Tokens;

use crate::error::ParserErr;

#[derive(Clone, Copy, Debug)]
pub struct ParserLimits {
    pub max_nesting: Option<usize>,
}

impl ParserLimits {
    pub fn unlimited() -> Self {
        Self { max_nesting: None }
    }

    pub fn safe_defaults() -> Self {
        Self {
            max_nesting: Some(512),
        }
    }
}

impl Default for ParserLimits {
    fn default() -> Self {
        Self::unlimited()
    }
}

pub struct Parser {
    peg: ast_builder::PegParser,
}

impl Parser {
    pub fn ast_boundary() -> &'static str {
        grammar::AST_BOUNDARY
    }

    pub fn grammar() -> &'static str {
        grammar::REX_PEG_GRAMMAR
    }

    pub fn new(tokens: Tokens) -> Parser {
        Parser {
            peg: ast_builder::PegParser::new(tokens),
        }
    }

    pub fn set_limits(&mut self, limits: ParserLimits) {
        self.peg.set_limits(limits);
    }

    pub fn parse_program(&mut self) -> Result<Program, Vec<ParserErr>> {
        self.peg.parse_program()
    }
}
