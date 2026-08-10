use crate::{
    modules::tools::executor::OutputKind::{Directory, Numbered, Single, Tree},
    storage::{store::Store, transfer},
};
use blake3::Hash;
use std::{
    collections::BTreeMap,
    error::Error,
    fmt,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    process::Stdio,
    sync::Arc,
};
use tokio::{io::AsyncWriteExt, process::Command};

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

#[derive(Clone, Debug)]
pub struct ToolExecutionPlan {
    pub program: ToolProgram,
    pub arguments: Vec<ToolArgument>,
    pub inputs: Vec<CasInput>,
    pub outputs: Vec<ExpectedOutput>,
    pub stdin: Option<Hash>,
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
}

impl ToolProgram {
    fn command(self) -> (&'static str, Option<&'static str>) {
        match self {
            Self::Ffmpeg => ("ffmpeg", None),
            Self::Ffprobe => ("ffprobe", None),
            Self::ImageMagick => ("magick", None),
            Self::ImageMagickMogrify => ("magick", Some("mogrify")),
            Self::ImageMagickIdentify => ("magick", Some("identify")),
            Self::ImageMagickCompare => ("magick", Some("compare")),
            Self::ImageMagickComposite => ("magick", Some("composite")),
            Self::ImageMagickMontage => ("magick", Some("montage")),
            Self::ImageMagickStream => ("magick", Some("stream")),
            Self::Qpdf => ("qpdf", None),
            Self::PdfInfo => ("pdfinfo", None),
            Self::PdfToText => ("pdftotext", None),
            Self::PdfToCairo => ("pdftocairo", None),
            Self::PdfImages => ("pdfimages", None),
        }
    }
}

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
    fn new(message: impl Into<String>) -> Self {
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

pub trait ToolExecutor: Send + Sync {
    fn execute<'a>(&'a self, store: &'a Store, plan: ToolExecutionPlan) -> ToolFuture<'a>;
}

#[derive(Clone, Default)]
pub struct LocalToolExecutor;

impl ToolExecutor for LocalToolExecutor {
    fn execute<'a>(&'a self, store: &'a Store, plan: ToolExecutionPlan) -> ToolFuture<'a> {
        Box::pin(execute_local(store, plan))
    }
}

pub fn local_executor() -> Arc<dyn ToolExecutor> {
    Arc::new(LocalToolExecutor)
}

async fn execute_local(
    store: &Store,
    plan: ToolExecutionPlan,
) -> Result<ToolExecution, ToolExecutionError> {
    let temporary = tempfile::tempdir()
        .map_err(|error| ToolExecutionError::new(format!("create tool workspace: {error}")))?;
    let input_dir = temporary.path().join("inputs");
    let output_dir = temporary.path().join("outputs");
    let scratch_dir = temporary.path().join("scratch");
    for directory in [&input_dir, &output_dir, &scratch_dir] {
        std::fs::create_dir_all(directory).map_err(|error| {
            ToolExecutionError::new(format!("create `{}`: {error}", directory.display()))
        })?;
    }

    let input_paths = materialize_inputs(store, &input_dir, &plan.inputs).await?;
    let output_paths = prepare_outputs(&output_dir, &plan.outputs)?;
    let arguments = render_arguments(&plan.arguments, &input_paths, &output_paths)?;

    let (executable, subcommand) = plan.program.command();
    let mut command = Command::new(executable);
    if let Some(subcommand) = subcommand {
        command.arg(subcommand);
    }
    command
        .args(arguments)
        .current_dir(temporary.path())
        .env("MAGICK_TEMPORARY_PATH", &scratch_dir)
        .env("TMPDIR", &scratch_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let stdin_data = match plan.stdin {
        Some(hash) => Some(
            store
                .get(hash)
                .await
                .map_err(|error| ToolExecutionError::new(format!("read stdin object: {error}")))?,
        ),
        None => None,
    };
    command.stdin(if stdin_data.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });

    let mut child = command
        .spawn()
        .map_err(|error| ToolExecutionError::new(format!("spawn `{executable}`: {error}")))?;
    let stdin_writer = if let Some(data) = stdin_data {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| ToolExecutionError::new("spawned tool has no stdin pipe"))?;
        Some(tokio::spawn(async move {
            stdin.write_all(&data).await?;
            stdin.shutdown().await
        }))
    } else {
        None
    };

    let process_output = child
        .wait_with_output()
        .await
        .map_err(|error| ToolExecutionError::new(format!("wait for tool: {error}")))?;
    if let Some(writer) = stdin_writer {
        writer
            .await
            .map_err(|error| ToolExecutionError::new(format!("join stdin writer: {error}")))?
            .map_err(|error| ToolExecutionError::new(format!("write tool stdin: {error}")))?;
    }

    let outputs = import_outputs(store, &plan.outputs, &output_paths).await?;
    Ok(ToolExecution {
        exit_code: process_output.status.code(),
        stdout: process_output.stdout,
        stderr: process_output.stderr,
        outputs,
    })
}

async fn materialize_inputs(
    store: &Store,
    input_dir: &Path,
    inputs: &[CasInput],
) -> Result<Vec<PathBuf>, ToolExecutionError> {
    let mut paths = Vec::with_capacity(inputs.len());
    for (id, input) in inputs.iter().enumerate() {
        let path = match input.kind {
            InputKind::Blob => input_dir.join(format!(
                "input-{id:04}.{}",
                clean_extension(&input.extension)
            )),
            InputKind::Tree => input_dir.join(format!("input-{id:04}")),
        };
        match input.kind {
            InputKind::Blob => transfer::export_blob(store, input.hash, &path).await,
            InputKind::Tree => {
                std::fs::create_dir_all(&path).map_err(|error| {
                    ToolExecutionError::new(format!("create input directory: {error}"))
                })?;
                transfer::export_tree(store, input.hash, &path).await
            }
        }
        .map_err(|error| ToolExecutionError::new(format!("materialize input {id}: {error}")))?;
        paths.push(path);
    }
    Ok(paths)
}

fn prepare_outputs(
    output_dir: &Path,
    outputs: &[ExpectedOutput],
) -> Result<Vec<PathBuf>, ToolExecutionError> {
    let mut paths = Vec::with_capacity(outputs.len());
    for (id, output) in outputs.iter().enumerate() {
        let extension = clean_extension(&output.extension);
        let path = match output.kind {
            Single => output_dir.join(format!("output-{id:04}.{extension}")),
            Numbered => output_dir.join(format!("output-{id:04}-%06d.{extension}")),
            Directory | Tree => {
                let path = output_dir.join(format!("output-{id:04}"));
                std::fs::create_dir_all(&path).map_err(|error| {
                    ToolExecutionError::new(format!("create output directory: {error}"))
                })?;
                path
            }
        };
        paths.push(path);
    }
    Ok(paths)
}

fn render_arguments(
    arguments: &[ToolArgument],
    inputs: &[PathBuf],
    outputs: &[PathBuf],
) -> Result<Vec<String>, ToolExecutionError> {
    arguments
        .iter()
        .map(|argument| render_argument(argument, inputs, outputs))
        .collect()
}

fn render_argument(
    argument: &ToolArgument,
    inputs: &[PathBuf],
    outputs: &[PathBuf],
) -> Result<String, ToolExecutionError> {
    match argument {
        ToolArgument::Literal(value) => Ok(value.clone()),
        ToolArgument::Path {
            slot,
            prefix,
            suffix,
        } => {
            let path: &Path = match slot {
                PathSlot::Input(id) => inputs.get(*id).map(PathBuf::as_path),
                PathSlot::InputParent(id) => inputs.get(*id).and_then(|path| path.parent()),
                PathSlot::Output(id) => outputs.get(*id).map(PathBuf::as_path),
            }
            .ok_or_else(|| ToolExecutionError::new(format!("unknown path slot {slot:?}")))?;
            Ok(format!("{prefix}{}{suffix}", path.display()))
        }
        ToolArgument::Joined(parts) => {
            let mut rendered = String::new();
            for part in parts {
                rendered.push_str(&render_argument(part, inputs, outputs)?);
            }
            Ok(rendered)
        }
    }
}

async fn import_outputs(
    store: &Store,
    outputs: &[ExpectedOutput],
    paths: &[PathBuf],
) -> Result<BTreeMap<OutputId, Vec<Hash>>, ToolExecutionError> {
    let mut imported = BTreeMap::new();
    for (id, (output, path)) in outputs.iter().zip(paths).enumerate() {
        let files = match output.kind {
            Single => {
                if path.is_file() {
                    vec![path.clone()]
                } else {
                    Vec::new()
                }
            }
            Numbered => {
                let prefix = format!("output-{id:04}-");
                let mut files = transfer::regular_files(path.parent().ok_or_else(|| {
                    ToolExecutionError::new("numbered output has no parent directory")
                })?)
                .map_err(|error| ToolExecutionError::new(format!("scan output: {error}")))?;
                files.retain(|candidate| {
                    candidate
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with(&prefix))
                });
                files
            }
            Directory => transfer::regular_files(path)
                .map_err(|error| ToolExecutionError::new(format!("scan output: {error}")))?,
            Tree => {
                let (_, hash) = transfer::import_path(store, path).await.map_err(|error| {
                    ToolExecutionError::new(format!("import output tree: {error}"))
                })?;
                imported.insert(id, vec![hash]);
                continue;
            }
        };

        let mut hashes = Vec::with_capacity(files.len());
        for file in files {
            hashes.push(
                store
                    .put(std::fs::read(&file).map_err(|error| {
                        ToolExecutionError::new(format!(
                            "read output `{}`: {error}",
                            file.display()
                        ))
                    })?)
                    .await
                    .map_err(|error| {
                        ToolExecutionError::new(format!(
                            "store output `{}`: {error}",
                            file.display()
                        ))
                    })?,
            );
        }
        imported.insert(id, hashes);
    }
    Ok(imported)
}

fn clean_extension(extension: &str) -> String {
    let cleaned: String = extension
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect();
    if cleaned.is_empty() {
        "bin".to_string()
    } else {
        cleaned.to_ascii_lowercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbolic_paths_are_rendered_only_by_executor() {
        let args = vec![
            ToolArgument::literal("-resize"),
            ToolArgument::literal("10x10"),
            ToolArgument::input_decorated(0, "png:", "[0]"),
            ToolArgument::output_decorated(0, "jpeg:"),
        ];
        let rendered = render_arguments(
            &args,
            &[PathBuf::from("/tmp/in")],
            &[PathBuf::from("/tmp/out")],
        )
        .unwrap();
        assert_eq!(
            rendered,
            ["-resize", "10x10", "png:/tmp/in[0]", "jpeg:/tmp/out"]
        );
    }

    #[test]
    fn joined_arguments_can_embed_paths_and_parent_directories() {
        let args = vec![ToolArgument::joined(vec![
            ToolArgument::literal("subtitles=filename='"),
            ToolArgument::input(0),
            ToolArgument::literal("':fontsdir='"),
            ToolArgument::input_parent_decorated(0, "", ""),
            ToolArgument::literal("'"),
        ])];
        let rendered = render_arguments(
            &args,
            &[PathBuf::from("/private/work/inputs/subtitles.srt")],
            &[],
        )
        .unwrap();
        assert_eq!(
            rendered,
            [
                "subtitles=filename='/private/work/inputs/subtitles.srt':fontsdir='/private/work/inputs'"
            ]
        );
    }
}
