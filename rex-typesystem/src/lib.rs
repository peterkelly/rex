#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

//! Hindley-Milner type system with parametric polymorphism, type classes, and ADTs.
//! The goal is to provide a reusable crate for building typing environments for Rex.
//! Features:
//! - Type variables, type constructors, function and tuple types.
//! - Schemes with quantified variables and class constraints.
//! - Type classes with superclass relationships and instance resolution.
//! - Utilities to register function/type declarations, ADTs, classes, and instances.
//!
//! The standard Rex prelude is owned by `rex-engine`, which builds a prelude-enabled
//! `TypeSystem` from parsed Rex declarations and engine-native primitive schemes.

pub mod error;
pub mod inference;
pub mod types;
pub mod typesystem;
pub mod unification;
pub mod wire;
