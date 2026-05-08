//! Type inference and Rex type metadata.
//!
//! Embedders usually interact with these types indirectly through
//! [`Engine`](crate::engine::Engine) and [`Module`](crate::engine::Module). Use
//! this module when you need to build dynamic native signatures, inspect
//! inferred types, register Rust ADTs, or run type inference without evaluating
//! a program.

/// Conflict found while collecting algebraic data type declarations.
pub use rex_typesystem::error::AdtConflict;

/// Error returned while discovering the ADT family referenced by Rust-facing types.
pub use rex_typesystem::error::CollectAdtsError;

/// Error produced by type inference, type registration, or type class resolution.
pub use rex_typesystem::error::TypeError;

/// Infer only the final type of a source expression.
pub use rex_typesystem::inference::infer;

/// Infer a typed expression tree, preserving type information at each node.
pub use rex_typesystem::inference::infer_typed;

/// Return the parsed Rex program that implements prelude type class methods.
pub use rex_typesystem::prelude::prelude_typeclasses_program;

/// Structured declaration for a Rex algebraic data type.
pub use rex_typesystem::types::AdtDecl;

/// A named type parameter in an algebraic data type declaration.
pub use rex_typesystem::types::AdtParam;

/// A constructor variant inside an algebraic data type declaration.
pub use rex_typesystem::types::AdtVariant;

/// Identifier for one of Rex's built-in host-compatible types.
pub use rex_typesystem::types::BuiltinTypeId;

/// A registered type class instance.
pub use rex_typesystem::types::Instance;

/// A type class constraint such as `Show a`.
pub use rex_typesystem::types::Predicate;

/// Trait for Rust ADT types that can expose structured Rex declarations.
pub use rex_typesystem::types::RexAdt;

/// Trait for Rust types that have a corresponding Rex type.
pub use rex_typesystem::types::RexType;

/// A polymorphic type scheme with quantified variables and constraints.
pub use rex_typesystem::types::Scheme;

/// A Rex type after parsing or inference.
pub use rex_typesystem::types::Type;

/// A built-in or user-defined type constructor.
pub use rex_typesystem::types::TypeConst;

/// The internal shape of a Rex [`Type`].
pub use rex_typesystem::types::TypeKind;

/// A type variable used during Hindley-Milner inference.
pub use rex_typesystem::types::TypeVar;

/// Discover the user-defined ADTs mentioned by a collection of Rex types.
pub use rex_typesystem::types::collect_adts_in_types;

/// Order an ADT family so dependencies appear before dependents.
pub use rex_typesystem::types::order_adt_family;

/// Mutable typing environment used by inference and the engine.
pub use rex_typesystem::typesystem::TypeSystem;

/// Generator for fresh type variables during inference.
pub use rex_typesystem::typesystem::TypeVarSupply;
