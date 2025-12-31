#![forbid(unsafe_code)]

//! AST data structures for Rex.
//!
//! This crate is intentionally “dumb data” first: keep it easy to read, print,
//! and transform. Anything with complicated control flow generally belongs in a
//! later phase (parser, type checker, engine).

pub mod expr;
pub mod id;
pub mod macros;
