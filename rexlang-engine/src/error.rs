use std::path::PathBuf;
use std::process::ExitStatus;

use rexlang_ast::expr::Symbol;
use rexlang_lexer::LexicalError;
use rexlang_parser::error::ParserErr;
use rexlang_ts::TypeError;
use rexlang_util::OutOfGas;

use crate::modules::ModuleId;

#[derive(Debug)]
pub enum ModuleError {
    NotFound {
        module_name: String,
    },
    NoBaseDirectory,
    ImportEscapesRoot,
    EmptyModulePath,
    StatePoisoned,
    CyclicImport {
        id: ModuleId,
    },
    InvalidIncludeRoot {
        path: PathBuf,
        source: std::io::Error,
    },
    InvalidModulePath {
        path: PathBuf,
        source: std::io::Error,
    },
    ReadFailed {
        path: PathBuf,
        source: std::io::Error,
    },
    NotUtf8 {
        kind: &'static str,
        path: PathBuf,
        source: std::string::FromUtf8Error,
    },
    NotUtf8Remote {
        url: String,
        source: std::string::FromUtf8Error,
    },
    ShaMismatchStdlib {
        module: String,
        expected: String,
        actual: String,
    },
    ShaMismatchPath {
        kind: &'static str,
        path: PathBuf,
        expected: String,
        actual: String,
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
    LexInModule {
        module: ModuleId,
        source: LexicalError,
    },
    Parse {
        errors: Vec<ParserErr>,
    },
    ParseInModule {
        module: ModuleId,
        errors: Vec<ParserErr>,
    },
    TopLevelExprInModule {
        module: ModuleId,
    },
    InvalidGithubImport {
        url: String,
    },
    UnpinnedGithubImport {
        url: String,
    },
    CurlFailed {
        source: std::io::Error,
    },
    CurlNonZeroExit {
        url: String,
        status: ExitStatus,
    },
}

impl std::fmt::Display for ModuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModuleError::NotFound { module_name } => write!(f, "module not found: {module_name}"),
            ModuleError::NoBaseDirectory => {
                write!(f, "cannot resolve local import without a base directory")
            }
            ModuleError::ImportEscapesRoot => write!(f, "import path escapes filesystem root"),
            ModuleError::EmptyModulePath => write!(f, "empty module path"),
            ModuleError::StatePoisoned => write!(f, "module state poisoned"),
            ModuleError::CyclicImport { id } => write!(f, "cyclic module import: {id}"),
            ModuleError::InvalidIncludeRoot { path, source } => {
                write!(f, "invalid include root `{}`: {source}", path.display())
            }
            ModuleError::InvalidModulePath { path, source } => {
                write!(f, "invalid module path `{}`: {source}", path.display())
            }
            ModuleError::ReadFailed { path, source } => {
                write!(f, "failed to read module `{}`: {source}", path.display())
            }
            ModuleError::NotUtf8 { kind, path, source } => {
                write!(
                    f,
                    "{kind} module `{}` was not utf-8: {source}",
                    path.display()
                )
            }
            ModuleError::NotUtf8Remote { url, source } => {
                write!(f, "remote module `{url}` was not utf-8: {source}")
            }
            ModuleError::ShaMismatchStdlib {
                module,
                expected,
                actual,
            } => write!(
                f,
                "sha mismatch for `{module}`: expected #{expected}, got #{actual}"
            ),
            ModuleError::ShaMismatchPath {
                kind,
                path,
                expected,
                actual,
            } => write!(
                f,
                "{kind} import sha mismatch for {}: expected #{expected}, got #{actual}",
                path.display()
            ),
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
            ModuleError::LexInModule { module, source } => {
                write!(f, "lex error in module {module}: {source}")
            }
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
            ModuleError::InvalidGithubImport { url } => write!(
                f,
                "github import must be `https://github.com/<owner>/<repo>/<path>.rex#<sha>` (got {url})"
            ),
            ModuleError::UnpinnedGithubImport { url } => {
                write!(f, "github import must be pinned: add `#<sha>` (got {url})")
            }
            ModuleError::CurlFailed { source } => write!(f, "failed to run curl: {source}"),
            ModuleError::CurlNonZeroExit { url, status } => {
                write!(f, "failed to fetch {url} (curl exit {status})")
            }
        }
    }
}

impl std::error::Error for ModuleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ModuleError::InvalidIncludeRoot { source, .. } => Some(source),
            ModuleError::InvalidModulePath { source, .. } => Some(source),
            ModuleError::ReadFailed { source, .. } => Some(source),
            ModuleError::NotUtf8 { source, .. } => Some(source),
            ModuleError::NotUtf8Remote { source, .. } => Some(source),
            ModuleError::Lex { source } => Some(source),
            ModuleError::LexInModule { source, .. } => Some(source),
            ModuleError::CurlFailed { source } => Some(source),
            _ => None,
        }
    }
}

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
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    OutOfGas(#[from] OutOfGas),
    #[error("{0}")]
    Custom(String),
    #[error("Evaluation suspended")]
    Suspended,
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
