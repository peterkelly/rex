//! Parse Rex source text into AST values.
//!
//! The parser is the first stage of the Rex pipeline. It returns syntax data
//! from [`ast`](crate::ast) without performing type inference or evaluation.

/// A parse diagnostic with source location information.
pub use rex_parser::error::ParseError;

/// Parse a Rex source string into a [`CompilationUnit`](crate::ast::CompilationUnit).
pub use rex_parser::parse;
