use blake3::Hash;
use clap::{Args as ClapArgs, CommandFactory, Parser, ValueEnum};
use rex_workflow::{
    config::Config,
    modules::tools::executor::DockerToolImages,
    run::{eval_rex, render_result_json},
    state::State,
    storage::{entry::EntryKind, store::Store, transfer},
};
use std::{
    io::{ErrorKind, Write},
    path::PathBuf,
};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(long, env = "REX_STORE", default_value = "./store")]
    store_path: PathBuf,

    #[command(subcommand)]
    subcommand: SubCommand,
}

#[derive(Parser, Debug)]
enum SubCommand {
    #[command(subcommand)]
    Store(StoreSubCommand),
    #[command(subcommand)]
    Server(ServerSubCommand),
    // #[command(subcommand)]
    Run {
        /// Path to a `.rex` file to run.
        #[arg(value_name = "FILE")]
        path: PathBuf,

        /// JSON file containing inputs for a `main` function.
        #[arg(long = "inputs", value_name = "JSON")]
        inputs: Option<String>,

        /// Print string results directly instead of as JSON string literals.
        #[arg(long = "raw-output")]
        raw_output: bool,

        #[command(flatten)]
        tools: ToolOptions,
    },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
enum ToolExecutorChoice {
    /// Run tools as processes on the host operating system.
    #[default]
    Local,
    /// Run tools in isolated containers through the local Docker daemon.
    Docker,
}

#[derive(ClapArgs, Debug)]
struct ToolOptions {
    /// Execution backend used for external workflow tools.
    #[arg(long, env = "REX_WORKFLOW_TOOL_EXECUTOR", value_enum, default_value_t)]
    tool_executor: ToolExecutorChoice,

    /// Docker image containing FFmpeg and FFprobe.
    #[arg(long, env = "REX_WORKFLOW_DOCKER_FFMPEG_IMAGE")]
    docker_ffmpeg_image: Option<String>,

    /// Docker image containing ImageMagick.
    #[arg(long, env = "REX_WORKFLOW_DOCKER_IMAGEMAGICK_IMAGE")]
    docker_imagemagick_image: Option<String>,

    /// Docker image containing QPDF.
    #[arg(long, env = "REX_WORKFLOW_DOCKER_QPDF_IMAGE")]
    docker_qpdf_image: Option<String>,

    /// Docker image containing the Poppler command suite.
    #[arg(long, env = "REX_WORKFLOW_DOCKER_POPPLER_IMAGE")]
    docker_poppler_image: Option<String>,
}

impl ToolOptions {
    fn state(self, store: Store) -> Result<State, String> {
        match self.tool_executor {
            ToolExecutorChoice::Local => {
                if self.has_docker_image_override() {
                    return Err("Docker image options require `--tool-executor docker`".to_owned());
                }
                Ok(State::local(store))
            }
            ToolExecutorChoice::Docker => Ok(State::docker(store, self.docker_images())),
        }
    }

    fn has_docker_image_override(&self) -> bool {
        self.docker_ffmpeg_image.is_some()
            || self.docker_imagemagick_image.is_some()
            || self.docker_qpdf_image.is_some()
            || self.docker_poppler_image.is_some()
    }

    fn docker_images(self) -> DockerToolImages {
        DockerToolImages::new(
            self.docker_ffmpeg_image
                .unwrap_or_else(|| "rex-tool-ffmpeg:local".to_owned()),
            self.docker_imagemagick_image
                .unwrap_or_else(|| "rex-tool-imagemagick:local".to_owned()),
            self.docker_qpdf_image
                .unwrap_or_else(|| "rex-tool-qpdf:local".to_owned()),
            self.docker_poppler_image
                .unwrap_or_else(|| "rex-tool-poppler:local".to_owned()),
        )
    }
}

#[derive(Parser, Debug)]
enum StoreSubCommand {
    Cat { path: String },
    Ls { path: String },
    ResolvePath { path: String },
    Import { path: PathBuf },
    Export { hash: Hash, path: PathBuf },
}

impl StoreSubCommand {
    async fn run(self, config: Config) -> Result<(), Box<dyn std::error::Error>> {
        let store = Store::new_with_filesystem(config.store_path);
        match self {
            StoreSubCommand::Cat { path } => {
                let hash = store.resolve_path(path).await?;
                let data = store.get(hash).await?;
                std::io::stdout().write_all(&data)?;
            }
            StoreSubCommand::Ls { path } => {
                let hash = store.resolve_path(&path).await?;
                let entries = match store.get_tree(hash).await {
                    Ok(entries) => entries,
                    Err(error) if error.kind() == ErrorKind::NotADirectory => {
                        return Err(format!("Not a tree: {path}").into());
                    }
                    Err(error) => return Err(Box::new(error)),
                };

                let mut sw = 0;
                for entry in entries.values() {
                    sw = std::cmp::max(sw, entry.size.to_string().len());
                }

                for (name, entry) in entries.iter() {
                    print!(
                        "{} {:<4} {:>sw$} {}",
                        entry.hash, entry.kind, entry.size, name
                    );
                    if entry.kind == EntryKind::Tree {
                        print!("/");
                    }
                    println!();
                }
            }
            StoreSubCommand::ResolvePath { path } => {
                let hash = store.resolve_path(path).await?;
                println!("{}", hash);
            }
            StoreSubCommand::Import { path } => {
                let (_kind, hash) = transfer::import_path(&store, path.as_path())
                    .await
                    .map_err(|error| error.to_string())?;
                println!("{}", hash);
            }
            StoreSubCommand::Export { hash, path } => {
                let result = match store.get_tree(hash).await {
                    Ok(_) => transfer::export_tree(&store, hash, &path).await,
                    Err(error) if error.kind() == ErrorKind::NotADirectory => {
                        transfer::export_blob(&store, hash, &path).await
                    }
                    Err(error) => return Err(Box::new(error)),
                };
                result.map_err(|error| error.to_string())?;
                println!("Export done");
            }
        }
        Ok(())
    }
}

#[derive(Parser, Debug)]
enum ServerSubCommand {
    Start {
        #[clap(long, env = "REX_HOST")]
        host: String,
        #[clap(long, env = "REX_PORT")]
        port: u16,
    },
}

impl ServerSubCommand {
    async fn run(self, _config: Config) -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = match Args::try_parse() {
        Ok(args) => args,
        Err(err) => {
            if err.kind() == clap::error::ErrorKind::MissingSubcommand {
                Args::command().print_help().unwrap();
                println!();
                std::process::exit(1);
            } else {
                err.exit();
            }
        }
    };

    let config = Config {
        store_path: args.store_path.clone(),
    };

    match args.subcommand {
        SubCommand::Store(sub) => sub.run(config).await?,
        SubCommand::Server(sub) => sub.run(config).await?,
        SubCommand::Run {
            path,
            inputs,
            raw_output,
            tools,
        } => {
            let source = std::fs::read_to_string(path)?;

            let inputs: Option<serde_json::Value> = match inputs {
                Some(path) => {
                    let raw = std::fs::read_to_string(path.clone())
                        .map_err(|e| format!("failed to read `{path}`: {e}"))?;
                    serde_json::from_str(&raw)
                        .map_err(|e| format!("failed to parse input JSON `{path}`: {e}"))?
                }
                None => None,
            };

            let store = Store::new_with_filesystem(config.store_path.clone());
            let state = tools.state(store)?;

            let result_json = eval_rex(&source, inputs, state).await?;
            let rendered = render_result_json(&result_json, raw_output)?;
            println!("{rendered}");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rex_workflow::modules::tools::executor::ToolBundle;

    fn parse_run(arguments: &[&str]) -> ToolOptions {
        let args = Args::try_parse_from(arguments).expect("parse rex-workflow arguments");
        let SubCommand::Run { tools, .. } = args.subcommand else {
            panic!("expected run subcommand");
        };
        tools
    }

    #[test]
    fn run_defaults_to_the_local_tool_executor() {
        let tools = parse_run(&["rex", "run", "workflow.rex"]);

        assert_eq!(tools.tool_executor, ToolExecutorChoice::Local);
        assert!(!tools.has_docker_image_override());
        assert!(tools.state(Store::new_in_memory()).is_ok());
    }

    #[test]
    fn run_configures_default_local_docker_images() {
        let tools = parse_run(&["rex", "run", "workflow.rex", "--tool-executor", "docker"]);

        assert_eq!(tools.tool_executor, ToolExecutorChoice::Docker);
        let images = tools.docker_images();
        assert_eq!(images.image(ToolBundle::Ffmpeg), "rex-tool-ffmpeg:local");
        assert_eq!(
            images.image(ToolBundle::ImageMagick),
            "rex-tool-imagemagick:local"
        );
        assert_eq!(images.image(ToolBundle::Qpdf), "rex-tool-qpdf:local");
        assert_eq!(images.image(ToolBundle::Poppler), "rex-tool-poppler:local");
    }

    #[test]
    fn run_accepts_a_docker_image_override_for_every_bundle() {
        let tools = parse_run(&[
            "rex",
            "run",
            "workflow.rex",
            "--tool-executor",
            "docker",
            "--docker-ffmpeg-image",
            "registry.example/ffmpeg@sha256:111",
            "--docker-imagemagick-image",
            "registry.example/imagemagick@sha256:222",
            "--docker-qpdf-image",
            "registry.example/qpdf@sha256:333",
            "--docker-poppler-image",
            "registry.example/poppler@sha256:444",
        ]);

        let images = tools.docker_images();
        assert_eq!(
            images.image(ToolBundle::Ffmpeg),
            "registry.example/ffmpeg@sha256:111"
        );
        assert_eq!(
            images.image(ToolBundle::ImageMagick),
            "registry.example/imagemagick@sha256:222"
        );
        assert_eq!(
            images.image(ToolBundle::Qpdf),
            "registry.example/qpdf@sha256:333"
        );
        assert_eq!(
            images.image(ToolBundle::Poppler),
            "registry.example/poppler@sha256:444"
        );
    }

    #[test]
    fn run_rejects_docker_image_options_with_the_local_executor() {
        let tools = parse_run(&[
            "rex",
            "run",
            "workflow.rex",
            "--docker-qpdf-image",
            "registry.example/qpdf:latest",
        ]);

        let error = tools
            .state(Store::new_in_memory())
            .err()
            .expect("local execution should reject Docker image options");
        assert_eq!(
            error,
            "Docker image options require `--tool-executor docker`"
        );
    }
}
