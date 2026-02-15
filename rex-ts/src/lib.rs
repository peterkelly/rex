#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

//! Hindley-Milner type system with parametric polymorphism, type classes, and ADTs.
//! The goal is to provide a reusable library for building typing environments for Rex.
//! Features:
//! - Type variables, type constructors, function and tuple types.
//! - Schemes with quantified variables and class constraints.
//! - Type classes with superclass relationships and instance resolution.
//! - Basic ADTs (List, Result, Option) and numeric/string primitives in the prelude.
//! - Utilities to register additional function/type declarations (e.g. `(-)`, `(/)`).

mod prelude;
mod typesystem;

pub use prelude::prelude_typeclasses_program;
pub use typesystem::*;
