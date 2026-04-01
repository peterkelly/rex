use std::path::PathBuf;
use std::process::ExitStatus;

use rexlang_ast::expr::Symbol;
use rexlang_lexer::LexicalError;
use rexlang_parser::error::ParserErr;
use rexlang_typesystem::TypeError;
use rexlang_util::OutOfGas;

use crate::libraries::LibraryId;

#[derive(Debug)]
pub enum LibraryError {
    NotFound {
        library_name: String,
    },
    NoBaseDirectory,
    ImportEscapesRoot,
    EmptyLibraryPath,
    StatePoisoned,
    CyclicImport {
        id: LibraryId,
    },
    InvalidIncludeRoot {
        path: PathBuf,
        source: std::io::Error,
    },
    InvalidLibraryPath {
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
        library: String,
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
        library: Symbol,
        export: Symbol,
    },
    DuplicateImportedName {
        name: Symbol,
    },
    ImportNameConflictsWithLocal {
        library: Symbol,
        name: Symbol,
    },
    Lex {
        source: LexicalError,
    },
    LexInLibrary {
        library: LibraryId,
        source: LexicalError,
    },
    Parse {
        errors: Vec<ParserErr>,
    },
    ParseInLibrary {
        library: LibraryId,
        errors: Vec<ParserErr>,
    },
    TopLevelExprInLibrary {
        library: LibraryId,
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

impl std::fmt::Display for LibraryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LibraryError::NotFound { library_name } => {
                write!(f, "library not found: {library_name}")
            }
            LibraryError::NoBaseDirectory => {
                write!(f, "cannot resolve local import without a base directory")
            }
            LibraryError::ImportEscapesRoot => write!(f, "import path escapes filesystem root"),
            LibraryError::EmptyLibraryPath => write!(f, "empty library path"),
            LibraryError::StatePoisoned => write!(f, "library state poisoned"),
            LibraryError::CyclicImport { id } => write!(f, "cyclic library import: {id}"),
            LibraryError::InvalidIncludeRoot { path, source } => {
                write!(f, "invalid include root `{}`: {source}", path.display())
            }
            LibraryError::InvalidLibraryPath { path, source } => {
                write!(f, "invalid library path `{}`: {source}", path.display())
            }
            LibraryError::ReadFailed { path, source } => {
                write!(f, "failed to read library `{}`: {source}", path.display())
            }
            LibraryError::NotUtf8 { kind, path, source } => {
                write!(
                    f,
                    "{kind} library `{}` was not utf-8: {source}",
                    path.display()
                )
            }
            LibraryError::NotUtf8Remote { url, source } => {
                write!(f, "remote library `{url}` was not utf-8: {source}")
            }
            LibraryError::ShaMismatchStdlib {
                library,
                expected,
                actual,
            } => write!(
                f,
                "sha mismatch for `{library}`: expected #{expected}, got #{actual}"
            ),
            LibraryError::ShaMismatchPath {
                kind,
                path,
                expected,
                actual,
            } => write!(
                f,
                "{kind} import sha mismatch for {}: expected #{expected}, got #{actual}",
                path.display()
            ),
            LibraryError::MissingExport { library, export } => {
                write!(f, "library `{library}` does not export `{export}`")
            }
            LibraryError::DuplicateImportedName { name } => {
                write!(f, "duplicate imported name `{name}`")
            }
            LibraryError::ImportNameConflictsWithLocal { library, name } => {
                write!(
                    f,
                    "imported name `{name}` from library `{library}` conflicts with local declaration"
                )
            }
            LibraryError::Lex { source } => write!(f, "lex error: {source}"),
            LibraryError::LexInLibrary { library, source } => {
                write!(f, "lex error in library {library}: {source}")
            }
            LibraryError::Parse { errors } => {
                write!(f, "parse error:")?;
                for err in errors {
                    write!(f, "\n  {err}")?;
                }
                Ok(())
            }
            LibraryError::ParseInLibrary { library, errors } => {
                write!(f, "parse error in library {library}:")?;
                for err in errors {
                    write!(f, "\n  {err}")?;
                }
                Ok(())
            }
            LibraryError::TopLevelExprInLibrary { library } => {
                write!(
                    f,
                    "library {library} cannot contain a top-level expression; library files must be declaration-only"
                )
            }
            LibraryError::InvalidGithubImport { url } => write!(
                f,
                "github import must be `https://github.com/<owner>/<repo>/<path>.rex#<sha>` (got {url})"
            ),
            LibraryError::UnpinnedGithubImport { url } => {
                write!(f, "github import must be pinned: add `#<sha>` (got {url})")
            }
            LibraryError::CurlFailed { source } => write!(f, "failed to run curl: {source}"),
            LibraryError::CurlNonZeroExit { url, status } => {
                write!(f, "failed to fetch {url} (curl exit {status})")
            }
        }
    }
}

impl std::error::Error for LibraryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LibraryError::InvalidIncludeRoot { source, .. } => Some(source),
            LibraryError::InvalidLibraryPath { source, .. } => Some(source),
            LibraryError::ReadFailed { source, .. } => Some(source),
            LibraryError::NotUtf8 { source, .. } => Some(source),
            LibraryError::NotUtf8Remote { source, .. } => Some(source),
            LibraryError::Lex { source } => Some(source),
            LibraryError::LexInLibrary { source, .. } => Some(source),
            LibraryError::CurlFailed { source } => Some(source),
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
    Library(#[from] Box<LibraryError>),
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    OutOfGas(#[from] OutOfGas),
    #[error("{0}")]
    Custom(String),
    #[error("Evaluation suspended")]
    Suspended,
}

#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct CompileError(#[from] EngineError);

impl CompileError {
    pub fn as_engine_error(&self) -> &EngineError {
        &self.0
    }

    pub fn into_engine_error(self) -> EngineError {
        self.0
    }
}

#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct EvalError(#[from] EngineError);

impl EvalError {
    pub fn as_engine_error(&self) -> &EngineError {
        &self.0
    }

    pub fn into_engine_error(self) -> EngineError {
        self.0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error(transparent)]
    Compile(#[from] CompileError),
    #[error(transparent)]
    Eval(#[from] EvalError),
}

impl ExecutionError {
    pub fn as_engine_error(&self) -> &EngineError {
        match self {
            ExecutionError::Compile(err) => err.as_engine_error(),
            ExecutionError::Eval(err) => err.as_engine_error(),
        }
    }

    pub fn into_engine_error(self) -> EngineError {
        match self {
            ExecutionError::Compile(err) => err.into_engine_error(),
            ExecutionError::Eval(err) => err.into_engine_error(),
        }
    }
}

impl From<LibraryError> for EngineError {
    fn from(err: LibraryError) -> Self {
        EngineError::Library(Box::new(err))
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
