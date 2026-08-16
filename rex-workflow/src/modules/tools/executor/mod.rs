mod catalog;
mod docker;
mod local;
mod workspace;

use crate::storage::store::Store;
use blake3::Hash;
use std::{collections::BTreeMap, error::Error, fmt, future::Future, pin::Pin};

pub use docker::{DockerToolExecutor, DockerToolImages, docker_executor};
pub use local::{LocalToolExecutor, local_executor};

const DEFAULT_MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

pub type InputId = usize;
pub type OutputId = usize;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PathSlot {
    Input(InputId),
    InputParent(InputId),
    Output(OutputId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolArgument {
    Literal(String),
    Path {
        slot: PathSlot,
        prefix: String,
        suffix: String,
    },
    Joined(Vec<ToolArgument>),
}

impl ToolArgument {
    pub fn literal(value: impl Into<String>) -> Self {
        Self::Literal(value.into())
    }

    pub fn input(id: InputId) -> Self {
        Self::Path {
            slot: PathSlot::Input(id),
            prefix: String::new(),
            suffix: String::new(),
        }
    }

    pub fn input_decorated(
        id: InputId,
        prefix: impl Into<String>,
        suffix: impl Into<String>,
    ) -> Self {
        Self::Path {
            slot: PathSlot::Input(id),
            prefix: prefix.into(),
            suffix: suffix.into(),
        }
    }

    pub fn input_parent_decorated(
        id: InputId,
        prefix: impl Into<String>,
        suffix: impl Into<String>,
    ) -> Self {
        Self::Path {
            slot: PathSlot::InputParent(id),
            prefix: prefix.into(),
            suffix: suffix.into(),
        }
    }

    pub fn output(id: OutputId) -> Self {
        Self::Path {
            slot: PathSlot::Output(id),
            prefix: String::new(),
            suffix: String::new(),
        }
    }

    pub fn output_decorated(id: OutputId, prefix: impl Into<String>) -> Self {
        Self::Path {
            slot: PathSlot::Output(id),
            prefix: prefix.into(),
            suffix: String::new(),
        }
    }

    pub fn output_with_suffix(id: OutputId, suffix: impl Into<String>) -> Self {
        Self::Path {
            slot: PathSlot::Output(id),
            prefix: String::new(),
            suffix: suffix.into(),
        }
    }

    pub fn joined(parts: Vec<ToolArgument>) -> Self {
        Self::Joined(parts)
    }
}

#[derive(Clone, Debug)]
pub struct CasInput {
    pub hash: Hash,
    pub extension: String,
    pub kind: InputKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputKind {
    Blob,
    Tree,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputKind {
    Single,
    Numbered,
    Directory,
    Tree,
}

#[derive(Clone, Debug)]
pub struct ExpectedOutput {
    pub kind: OutputKind,
    pub extension: String,
}

/// An executor-neutral tool invocation expressed only through arguments and files.
///
/// Standard input is always closed. Executors stage `inputs`, resolve symbolic paths in
/// `arguments`, and collect `outputs` after the backend reports that the job has finished.
#[derive(Clone, Debug)]
pub struct ToolExecutionPlan {
    pub program: ToolProgram,
    pub arguments: Vec<ToolArgument>,
    pub inputs: Vec<CasInput>,
    pub outputs: Vec<ExpectedOutput>,
}

/// One headless external program that the workflow host may execute.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolProgram {
    Ffmpeg,
    Ffprobe,
    ImageMagick,
    ImageMagickMogrify,
    ImageMagickIdentify,
    ImageMagickCompare,
    ImageMagickComposite,
    ImageMagickMontage,
    ImageMagickStream,
    Qpdf,
    PdfInfo,
    PdfToText,
    PdfToCairo,
    PdfImages,
    Gnuplot,
    Graphviz,
}

/// A set of programs installed together in one tool runtime image.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ToolBundle {
    Ffmpeg,
    Gnuplot,
    Graphviz,
    ImageMagick,
    Qpdf,
    Poppler,
}

impl ToolProgram {
    /// Return the runtime image bundle containing this program.
    pub fn bundle(self) -> ToolBundle {
        catalog::runtime(self).bundle
    }
}

impl ToolBundle {
    pub const ALL: [Self; 6] = [
        Self::Ffmpeg,
        Self::Gnuplot,
        Self::Graphviz,
        Self::ImageMagick,
        Self::Qpdf,
        Self::Poppler,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Ffmpeg => "ffmpeg",
            Self::Gnuplot => "gnuplot",
            Self::Graphviz => "graphviz",
            Self::ImageMagick => "imagemagick",
            Self::Qpdf => "qpdf",
            Self::Poppler => "poppler",
        }
    }
}

impl fmt::Display for ToolBundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A completed tool result reconstructed from backend-produced result and output files.
#[derive(Clone, Debug)]
pub struct ToolExecution {
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub outputs: BTreeMap<OutputId, Vec<Hash>>,
}

#[derive(Debug)]
pub struct ToolExecutionError(String);

impl ToolExecutionError {
    pub(super) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ToolExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for ToolExecutionError {}

pub type ToolFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ToolExecution, ToolExecutionError>> + Send + 'a>>;

/// Execute tool plans without using tool process streams or process status as result data.
///
/// Implementations may use backend-specific APIs for dispatch and lifecycle monitoring, but tool
/// stdout, stderr, and exit code must be recorded as files and retrieved after completion.
pub trait ToolExecutor: Send + Sync {
    fn execute<'a>(&'a self, store: &'a Store, plan: ToolExecutionPlan) -> ToolFuture<'a>;
}
