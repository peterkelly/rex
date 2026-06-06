use std::path::PathBuf;

use rex_ast::Symbol;
use rex_parser::{error::ParseError, lexer::LexicalError};
use rex_typesystem::error::TypeError;

use crate::modules::{ModuleId, ModuleIdError};

#[derive(Debug, Eq, PartialEq)]
pub enum ModuleError {
    NotFound {
        module_name: String,
    },
    ImportsDisabled {
        module_name: String,
    },
    ImportEscapesRoot,
    EmptyModulePath,
    StatePoisoned,
    CyclicImport {
        id: ModuleId,
    },
    NotUtf8 {
        kind: &'static str,
        path: PathBuf,
        source: std::string::FromUtf8Error,
    },
    MissingExport {
        module: Symbol,
        export: Symbol,
    },
    DuplicateImportedName {
        name: Symbol,
    },
    ImportNameConflictsWithLocal {
        module: Symbol,
        name: Symbol,
    },
    Lex {
        source: LexicalError,
    },
    Parse {
        errors: Vec<ParseError>,
    },
    ParseInModule {
        module: ModuleId,
        errors: Vec<ParseError>,
    },
    TopLevelExprInModule {
        module: ModuleId,
    },
}

impl std::fmt::Display for ModuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModuleError::NotFound { module_name } => {
                write!(f, "module not found: {module_name}")
            }
            ModuleError::ImportsDisabled { module_name } => {
                write!(f, "module imports are disabled: {module_name}")
            }
            ModuleError::ImportEscapesRoot => write!(f, "import path escapes filesystem root"),
            ModuleError::EmptyModulePath => write!(f, "empty module path"),
            ModuleError::StatePoisoned => write!(f, "module state poisoned"),
            ModuleError::CyclicImport { id } => write!(f, "cyclic module import: {id}"),
            ModuleError::NotUtf8 { kind, path, source } => {
                write!(
                    f,
                    "{kind} module `{}` was not utf-8: {source}",
                    path.display()
                )
            }
            ModuleError::MissingExport { module, export } => {
                write!(f, "module `{module}` does not export `{export}`")
            }
            ModuleError::DuplicateImportedName { name } => {
                write!(f, "duplicate imported name `{name}`")
            }
            ModuleError::ImportNameConflictsWithLocal { module, name } => {
                write!(
                    f,
                    "imported name `{name}` from module `{module}` conflicts with local declaration"
                )
            }
            ModuleError::Lex { source } => write!(f, "lex error: {source}"),
            ModuleError::Parse { errors } => {
                write!(f, "parse error:")?;
                for err in errors {
                    write!(f, "\n  {err}")?;
                }
                Ok(())
            }
            ModuleError::ParseInModule { module, errors } => {
                write!(f, "parse error in module {module}:")?;
                for err in errors {
                    write!(f, "\n  {err}")?;
                }
                Ok(())
            }
            ModuleError::TopLevelExprInModule { module } => {
                write!(
                    f,
                    "module {module} cannot contain a top-level expression; module files must be declaration-only"
                )
            }
        }
    }
}

impl std::error::Error for ModuleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ModuleError::NotUtf8 { source, .. } => Some(source),
            ModuleError::Lex { source } => Some(source),
            _ => None,
        }
    }
}

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
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
    #[error("expected {expected}, got {got}")]
    NativeType { expected: String, got: String },
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
    #[error("{context} must contain a final expression")]
    MissingBody { context: &'static str },
    #[error("program must define `main` or contain a final expression")]
    MissingMain,
    #[error("program defines `main` and also has a final expression; remove one entry point")]
    MainWithFinalExpression,
    #[error("duplicate `main` parameter `{name}`")]
    DuplicateMainInput { name: String },
    #[error(
        "`main` declares {declared} parameter(s), but its inferred type has {inferred} argument(s)"
    )]
    MainArityMismatch { declared: usize, inferred: usize },
    #[error("inputs do not match `main` parameters (missing: {missing:?}, extra: {extra:?})")]
    MainInputMismatch {
        missing: Vec<String>,
        extra: Vec<String>,
    },
    #[error("{0}")]
    InvalidModuleId(#[from] ModuleIdError),
    #[error("empty sequence")]
    EmptySequence,
    #[error("index {index} out of bounds in `{name}` (len {len})")]
    IndexOutOfBounds {
        name: Symbol,
        index: i32,
        len: usize,
    },
    #[error("internal error: {0}")]
    Internal(String),
    #[error(transparent)]
    Module(#[from] Box<ModuleError>),
    #[error("{0}")]
    Custom(String),
    #[error("Evaluation suspended")]
    Suspended,
}

#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error(transparent)]
    Compile(#[from] EngineError),
    #[error(transparent)]
    Eval(EngineError),
}

impl ExecutionError {
    pub fn as_engine_error(&self) -> &EngineError {
        match self {
            ExecutionError::Compile(err) => err,
            ExecutionError::Eval(err) => err,
        }
    }

    pub fn into_engine_error(self) -> EngineError {
        match self {
            ExecutionError::Compile(err) => err,
            ExecutionError::Eval(err) => err,
        }
    }
}

impl From<ModuleError> for EngineError {
    fn from(err: ModuleError) -> Self {
        EngineError::Module(Box::new(err))
    }
}

impl From<&str> for EngineError {
    fn from(msg: &str) -> Self {
        EngineError::Custom(msg.to_string())
    }
}

impl From<String> for EngineError {
    fn from(msg: String) -> Self {
        EngineError::Custom(msg)
    }
}
