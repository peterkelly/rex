use super::{
    CasInput, ExpectedOutput, InputKind, OutputId,
    OutputKind::{Directory, Numbered, Single, Tree},
    PathSlot, ToolArgument, ToolExecutionError,
};
use crate::storage::{store::Store, transfer};
use blake3::Hash;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

const INPUT_DIRECTORY: &str = "inputs";
const OUTPUT_DIRECTORY: &str = "outputs";
const SCRATCH_DIRECTORY: &str = "scratch";

pub(super) struct ToolWorkspace {
    temporary: tempfile::TempDir,
    input_paths: Vec<PathBuf>,
    output_paths: Vec<PathBuf>,
}

impl ToolWorkspace {
    pub(super) async fn prepare(
        store: &Store,
        inputs: &[CasInput],
        outputs: &[ExpectedOutput],
    ) -> Result<Self, ToolExecutionError> {
        let temporary = tempfile::tempdir()
            .map_err(|error| ToolExecutionError::new(format!("create tool workspace: {error}")))?;
        for directory in [INPUT_DIRECTORY, OUTPUT_DIRECTORY, SCRATCH_DIRECTORY] {
            let path = temporary.path().join(directory);
            std::fs::create_dir_all(&path).map_err(|error| {
                ToolExecutionError::new(format!("create `{}`: {error}", path.display()))
            })?;
        }

        let input_paths = materialize_inputs(store, temporary.path(), inputs).await?;
        let output_paths = prepare_outputs(temporary.path(), outputs)?;
        Ok(Self {
            temporary,
            input_paths,
            output_paths,
        })
    }

    pub(super) fn root(&self) -> &Path {
        self.temporary.path()
    }

    pub(super) fn scratch_dir(&self) -> PathBuf {
        self.root().join(SCRATCH_DIRECTORY)
    }

    pub(super) fn render_arguments(
        &self,
        arguments: &[ToolArgument],
        execution_root: &Path,
    ) -> Result<Vec<String>, ToolExecutionError> {
        let input_paths = resolve_paths(execution_root, &self.input_paths);
        let output_paths = resolve_paths(execution_root, &self.output_paths);
        render_arguments(arguments, &input_paths, &output_paths)
    }

    pub(super) async fn import_outputs(
        &self,
        store: &Store,
        outputs: &[ExpectedOutput],
    ) -> Result<BTreeMap<OutputId, Vec<Hash>>, ToolExecutionError> {
        let output_paths = resolve_paths(self.root(), &self.output_paths);
        import_outputs(store, outputs, &output_paths).await
    }
}

fn resolve_paths(root: &Path, paths: &[PathBuf]) -> Vec<PathBuf> {
    paths.iter().map(|path| root.join(path)).collect()
}

async fn materialize_inputs(
    store: &Store,
    workspace_root: &Path,
    inputs: &[CasInput],
) -> Result<Vec<PathBuf>, ToolExecutionError> {
    let mut paths = Vec::with_capacity(inputs.len());
    for (id, input) in inputs.iter().enumerate() {
        let relative_path = match input.kind {
            InputKind::Blob => PathBuf::from(INPUT_DIRECTORY).join(format!(
                "input-{id:04}.{}",
                clean_extension(&input.extension)
            )),
            InputKind::Tree => PathBuf::from(INPUT_DIRECTORY).join(format!("input-{id:04}")),
        };
        let path = workspace_root.join(&relative_path);
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
        paths.push(relative_path);
    }
    Ok(paths)
}

fn prepare_outputs(
    workspace_root: &Path,
    outputs: &[ExpectedOutput],
) -> Result<Vec<PathBuf>, ToolExecutionError> {
    let mut paths = Vec::with_capacity(outputs.len());
    for (id, output) in outputs.iter().enumerate() {
        let extension = clean_extension(&output.extension);
        let relative_path = match output.kind {
            Single => PathBuf::from(OUTPUT_DIRECTORY).join(format!("output-{id:04}.{extension}")),
            Numbered => {
                PathBuf::from(OUTPUT_DIRECTORY).join(format!("output-{id:04}-%06d.{extension}"))
            }
            Directory | Tree => {
                let path = PathBuf::from(OUTPUT_DIRECTORY).join(format!("output-{id:04}"));
                std::fs::create_dir_all(workspace_root.join(&path)).map_err(|error| {
                    ToolExecutionError::new(format!("create output directory: {error}"))
                })?;
                path
            }
        };
        paths.push(relative_path);
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
            Single => match std::fs::symlink_metadata(path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(ToolExecutionError::new(format!(
                        "tool output is a symbolic link: `{}`",
                        path.display()
                    )));
                }
                Ok(metadata) if metadata.file_type().is_file() => vec![path.clone()],
                Ok(_) => {
                    return Err(ToolExecutionError::new(format!(
                        "tool output is not a regular file: `{}`",
                        path.display()
                    )));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(error) => {
                    return Err(ToolExecutionError::new(format!(
                        "inspect tool output `{}`: {error}",
                        path.display()
                    )));
                }
            },
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
    use super::super::OutputKind;
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

    #[test]
    fn workspace_paths_can_be_rendered_under_an_execution_root() {
        assert_eq!(
            resolve_paths(
                Path::new("/work"),
                &[PathBuf::from("inputs/input-0000.mp4")]
            ),
            [PathBuf::from("/work/inputs/input-0000.mp4")]
        );
        assert_eq!(
            resolve_paths(
                Path::new("/work"),
                &[PathBuf::from("outputs/output-0000.mp3")]
            ),
            [PathBuf::from("/work/outputs/output-0000.mp3")]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn output_roots_must_not_be_symbolic_links() {
        use std::os::unix::fs::symlink;

        for kind in [
            OutputKind::Single,
            OutputKind::Numbered,
            OutputKind::Directory,
            OutputKind::Tree,
        ] {
            let temporary = tempfile::tempdir().unwrap();
            let target = temporary.path().join("target");
            let output = match kind {
                OutputKind::Single => {
                    std::fs::write(&target, b"outside").unwrap();
                    let output = temporary.path().join("output");
                    symlink(&target, &output).unwrap();
                    output
                }
                OutputKind::Numbered => {
                    std::fs::create_dir(&target).unwrap();
                    let output_root = temporary.path().join("output-root");
                    symlink(&target, &output_root).unwrap();
                    output_root.join("output-0000-%06d.bin")
                }
                OutputKind::Directory | OutputKind::Tree => {
                    std::fs::create_dir(&target).unwrap();
                    std::fs::write(target.join("outside.txt"), b"outside").unwrap();
                    let output = temporary.path().join("output");
                    symlink(&target, &output).unwrap();
                    output
                }
            };

            let error = import_outputs(
                &Store::new_in_memory(),
                &[ExpectedOutput {
                    kind,
                    extension: "bin".to_string(),
                }],
                &[output],
            )
            .await
            .unwrap_err();

            assert!(
                error.to_string().contains("symbolic link"),
                "unexpected error for {kind:?}: {error}"
            );
        }
    }
}
