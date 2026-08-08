#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

//! AST data structures for Rex.
//!
//! This crate is intentionally “dumb data” first: keep it easy to read, print,
//! and transform. Anything with complicated control flow generally belongs in a
//! later phase (parser, type checker, engine).

mod ast;
mod id;
mod macros;
mod span;
mod symbol;

pub use ast::{
    ClassDecl, ClassMethodSig, CompilationUnit, Decl, DeclareFnDecl, Expr, FnDecl, ImportClause,
    ImportDecl, ImportItem, ImportPath, InstanceDecl, InstanceMethodImpl, LetRecBinding, NameRef,
    Pattern, Scope, TypeConstraint, TypeDecl, TypeDeclKind, TypeExpr, TypeField, TypeParam,
    TypeVariant, TypeVariantArg, Var, char_literal,
};
pub use id::Id;
pub use span::{Position, Span, Spanned};
pub use symbol::Symbol;
