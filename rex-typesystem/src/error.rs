use crate::types::{Type, TypeVarId};
use rex_ast::expr::Symbol;
use rex_lexer::span::Span;
use rex_util::OutOfGas;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TypeError {
    #[error("types do not unify: {0} vs {1}")]
    Unification(String, String),
    #[error("occurs check failed for {0} in {1}")]
    Occurs(TypeVarId, String),
    #[error("unknown class {0}")]
    UnknownClass(Symbol),
    #[error("no instance for {0} {1}")]
    NoInstance(Symbol, String),
    #[error("unknown type {0}")]
    UnknownTypeName(Symbol),
    #[error("cannot redefine reserved builtin type `{0}`")]
    ReservedTypeName(Symbol),
    #[error("duplicate value definition `{0}`")]
    DuplicateValue(Symbol),
    #[error("duplicate class definition `{0}`")]
    DuplicateClass(Symbol),
    #[error("class `{class}` must have at least one type parameter (got {got})")]
    InvalidClassArity { class: Symbol, got: usize },
    #[error("duplicate class method `{0}`")]
    DuplicateClassMethod(Symbol),
    #[error("unknown method `{method}` in instance of class `{class}`")]
    UnknownInstanceMethod { class: Symbol, method: Symbol },
    #[error("missing implementation of `{method}` for instance of class `{class}`")]
    MissingInstanceMethod { class: Symbol, method: Symbol },
    #[error(
        "instance method `{method}` requires constraint {class} {typ}, but it is not in the instance context"
    )]
    MissingInstanceConstraint {
        method: Symbol,
        class: Symbol,
        typ: String,
    },
    #[error("unbound variable {0}")]
    UnknownVar(Symbol),
    #[error("ambiguous overload for {0}")]
    AmbiguousOverload(Symbol),
    #[error("ambiguous type variable(s) {vars:?} in constraints: {constraints}")]
    AmbiguousTypeVars {
        vars: Vec<TypeVarId>,
        constraints: String,
    },
    #[error(
        "kind mismatch for class `{class}`: expected {expected} type argument(s) remaining, got {got} for {typ}"
    )]
    KindMismatch {
        class: Symbol,
        expected: usize,
        got: usize,
        typ: String,
    },
    #[error("missing type class constraint(s): {constraints}")]
    MissingConstraints { constraints: String },
    #[error("unsupported expression {0}")]
    UnsupportedExpr(&'static str),
    #[error("unknown field `{field}` on {typ}")]
    UnknownField { field: Symbol, typ: String },
    #[error("field `{field}` is not definitely available on {typ}")]
    FieldNotKnown { field: Symbol, typ: String },
    #[error("non-exhaustive match for {typ}: missing {missing:?}")]
    NonExhaustiveMatch { typ: String, missing: Vec<Symbol> },
    #[error("at {span}: {error}")]
    Spanned { span: Span, error: Box<TypeError> },
    #[error("internal error: {0}")]
    Internal(String),
    #[error("{0}")]
    OutOfGas(#[from] OutOfGas),
}

impl TypeError {
    pub fn with_span(self, span: &Span) -> TypeError {
        match self {
            TypeError::Spanned { .. } => self,
            other => TypeError::Spanned {
                span: *span,
                error: Box::new(other),
            },
        }
    }
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct AdtConflict {
    pub name: Symbol,
    pub definitions: Vec<Type>,
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[error("conflicting ADT definitions: {conflicts:?}")]
pub struct CollectAdtsError {
    pub conflicts: Vec<AdtConflict>,
}
