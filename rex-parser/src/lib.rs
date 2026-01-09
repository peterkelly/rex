#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

//! Parsing for Rex.
//!
//! The parser is written to be straightforward to step through in a debugger:
//! no parser-generator indirection, and (mostly) explicit control flow.

pub mod error;
pub mod op;

mod parser;
mod stack;

pub use parser::Parser;
pub use parser::ParserLimits;
pub use stack::DEFAULT_STACK_SIZE_BYTES;
