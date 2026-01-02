use rex_ast::expr::Symbol;
use rex_ts::TypeError;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("unknown variable `{0}`")]
    UnknownVar(Symbol),
    #[error("value is not callable: {0}")]
    NotCallable(String),
    #[error("native `{name}` expected {expected} args, got {got}")]
    NativeArity {
        name: Symbol,
        expected: usize,
        got: usize,
    },
    #[error("native `{name}` expected {expected}, got {got}")]
    NativeType {
        name: Symbol,
        expected: String,
        got: String,
    },
    #[error("pattern match failure")]
    MatchFailure,
    #[error("expected boolean, got {0}")]
    ExpectedBool(String),
    #[error("type error: {0}")]
    Type(#[from] TypeError),
    #[error("ambiguous overload for `{name}`")]
    AmbiguousOverload { name: Symbol },
    #[error("no native implementation for `{name}` with type {typ}")]
    MissingImpl { name: Symbol, typ: String },
    #[error("ambiguous native implementation for `{name}` with type {typ}")]
    AmbiguousImpl { name: Symbol, typ: String },
    #[error("duplicate native implementation for `{name}` with type {typ}")]
    DuplicateImpl { name: Symbol, typ: String },
    #[error("no type class instance for `{class}` with type {typ}")]
    MissingTypeclassImpl { class: Symbol, typ: String },
    #[error("ambiguous type class instance for `{class}` with type {typ}")]
    AmbiguousTypeclassImpl { class: Symbol, typ: String },
    #[error("duplicate type class instance for `{class}` with type {typ}")]
    DuplicateTypeclassImpl { class: Symbol, typ: String },
    #[error("injected `{name}` has incompatible type {typ}")]
    InvalidInjection { name: Symbol, typ: String },
    #[error("unknown type for value in `{0}`")]
    UnknownType(Symbol),
    #[error("unknown field `{field}` on {value}")]
    UnknownField { field: Symbol, value: String },
    #[error("unsupported expression")]
    UnsupportedExpr,
    #[error("empty sequence")]
    EmptySequence,
    #[error("index {index} out of bounds in `{name}` (len {len})")]
    IndexOutOfBounds {
        name: Symbol,
        index: i32,
        len: usize,
    },
}

