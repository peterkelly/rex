use blake3::Hash;
use clap::{Args as ClapArgs, CommandFactory, Parser, ValueEnum};
use rex_workflow::{
    config::Config,
    modules::tools::executor::{
        DockerToolExecutor, DockerToolImages, ExpectedOutput, ToolArgument, ToolBundle,
        ToolExecutionPlan, ToolExecutor, ToolProgram,
    },
    run::{eval_rex, render_result_json},
    state::State,
    storage::{entry::EntryKind, store::Store, transfer},
};
use std::{
    ffi::OsStr,
    io::{ErrorKind, Write},
    path::PathBuf,
    process::{Command, Output, Stdio},
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
    #[command(subcommand)]
    Tools(ToolsSubCommand),
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
    #[arg(long, env = "REX_TOOL_EXECUTOR", value_enum, default_value_t)]
    tool_executor: ToolExecutorChoice,

    #[command(flatten)]
    images: DockerImageOptions,
}

#[derive(ClapArgs, Debug, Default)]
struct DockerImageOptions {
    /// Permit mutable tags in explicit image overrides.
    #[arg(long)]
    allow_image_tags: bool,

    /// Docker image containing FFmpeg and FFprobe.
    #[arg(long, env = "REX_WORKFLOW_DOCKER_FFMPEG_IMAGE")]
    docker_ffmpeg_image: Option<String>,

    /// Docker image containing gnuplot.
    #[arg(long, env = "REX_WORKFLOW_DOCKER_GNUPLOT_IMAGE")]
    docker_gnuplot_image: Option<String>,

    /// Docker image containing Graphviz.
    #[arg(long, env = "REX_WORKFLOW_DOCKER_GRAPHVIZ_IMAGE")]
    docker_graphviz_image: Option<String>,

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
                if self.images.is_configured() {
                    return Err("Docker image options require `--tool-executor docker`".to_owned());
                }
                Ok(State::local(store))
            }
            ToolExecutorChoice::Docker => Ok(State::docker(store, self.images.docker_images()?)),
        }
    }
}

impl DockerImageOptions {
    fn is_configured(&self) -> bool {
        self.allow_image_tags || self.has_image_override()
    }

    fn has_image_override(&self) -> bool {
        self.docker_ffmpeg_image.is_some()
            || self.docker_gnuplot_image.is_some()
            || self.docker_graphviz_image.is_some()
            || self.docker_imagemagick_image.is_some()
            || self.docker_qpdf_image.is_some()
            || self.docker_poppler_image.is_some()
    }

    fn docker_images(self) -> Result<DockerToolImages, String> {
        let mut images = local_tool_images();
        for (bundle, image) in [
            (ToolBundle::Ffmpeg, self.docker_ffmpeg_image),
            (ToolBundle::Gnuplot, self.docker_gnuplot_image),
            (ToolBundle::Graphviz, self.docker_graphviz_image),
            (ToolBundle::ImageMagick, self.docker_imagemagick_image),
            (ToolBundle::Qpdf, self.docker_qpdf_image),
            (ToolBundle::Poppler, self.docker_poppler_image),
        ] {
            if let Some(image) = image {
                if !self.allow_image_tags && !is_digest_qualified(&image) {
                    return Err(format!(
                        "Docker image override for {bundle} must be digest-qualified unless --allow-image-tags is supplied"
                    ));
                }
                images = images.with_image(bundle, image);
            }
        }
        images.validate().map_err(|error| error.to_string())?;
        Ok(images)
    }
}

fn local_tool_images() -> DockerToolImages {
    DockerToolImages::development(
        "rex-tool-ffmpeg:local",
        "rex-tool-gnuplot:local",
        "rex-tool-graphviz:local",
        "rex-tool-imagemagick:local",
        "rex-tool-qpdf:local",
        "rex-tool-poppler:local",
    )
}

fn is_digest_qualified(image: &str) -> bool {
    let Some((name, digest)) = image.rsplit_once("@sha256:") else {
        return false;
    };
    !name.is_empty() && digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Parser, Debug)]
enum ToolsSubCommand {
    /// Build and load all tool images for this machine's native architecture.
    Build,
    /// Diagnose Docker, image availability, architecture, and tool versions.
    Inspect {
        #[command(flatten)]
        images: DockerImageOptions,
    },
    /// Remove stopped Rex tool containers left behind by interrupted workflows.
    Cleanup {
        /// Also remove Rex tool containers that Docker still reports as running.
        #[arg(long)]
        include_running: bool,
    },
}

impl ToolsSubCommand {
    async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            Self::Build => build_tool_images(),
            Self::Inspect { images } => inspect_tool_images(images.docker_images()?).await,
            Self::Cleanup { include_running } => cleanup_tool_containers(include_running),
        }
    }
}

fn build_tool_images() -> Result<(), Box<dyn std::error::Error>> {
    let build_context = tempfile::tempdir()?;
    for (relative_path, contents) in TOOL_IMAGE_SOURCES {
        let destination = build_context.path().join(relative_path);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(destination, contents)?;
    }

    println!("Building native Rex tool images with Docker Buildx...");
    let status = Command::new("docker")
        .args(["buildx", "bake", "--load"])
        .current_dir(build_context.path())
        .stdin(Stdio::null())
        .status()?;
    if !status.success() {
        return Err(format!("build Rex tool images failed with status {status}").into());
    }
    println!("Built and loaded all native tool images.");
    Ok(())
}

const TOOL_IMAGE_SOURCES: &[(&str, &[u8])] = &[
    (
        "docker-bake.hcl",
        include_bytes!("../../tool-images/docker-bake.hcl"),
    ),
    (
        "ffmpeg/Dockerfile",
        include_bytes!("../../tool-images/ffmpeg/Dockerfile"),
    ),
    (
        "gnuplot/Dockerfile",
        include_bytes!("../../tool-images/gnuplot/Dockerfile"),
    ),
    (
        "graphviz/Dockerfile",
        include_bytes!("../../tool-images/graphviz/Dockerfile"),
    ),
    (
        "imagemagick/Dockerfile",
        include_bytes!("../../tool-images/imagemagick/Dockerfile"),
    ),
    (
        "imagemagick/policy.xml",
        include_bytes!("../../tool-images/imagemagick/policy.xml"),
    ),
    (
        "qpdf/Dockerfile",
        include_bytes!("../../tool-images/qpdf/Dockerfile"),
    ),
    (
        "poppler/Dockerfile",
        include_bytes!("../../tool-images/poppler/Dockerfile"),
    ),
];

async fn inspect_tool_images(images: DockerToolImages) -> Result<(), Box<dyn std::error::Error>> {
    let version = docker_command([
        OsStr::new("version"),
        OsStr::new("--format"),
        OsStr::new("{{.Server.Version}} {{.Server.Os}}/{{.Server.Arch}}"),
    ])?;
    let version = ensure_docker_success("query Docker server", version)?;
    println!(
        "Docker server: {}",
        String::from_utf8_lossy(&version.stdout).trim()
    );
    println!("Host architecture: {}", std::env::consts::ARCH);

    let mut missing = Vec::new();
    for (bundle, image) in images.iter() {
        let output = docker_command([
            OsStr::new("image"),
            OsStr::new("inspect"),
            OsStr::new("--format"),
            OsStr::new("{{.Id}} {{.Os}}/{{.Architecture}}"),
            OsStr::new(image),
        ])?;
        if output.status.success() {
            println!(
                "{bundle}: {} ({})",
                image,
                String::from_utf8_lossy(&output.stdout).trim()
            );
        } else {
            println!("{bundle}: MISSING ({image})");
            missing.push(bundle);
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "{} tool image(s) are missing; run `rex tools build` first",
            missing.len()
        )
        .into());
    }

    let store = Store::new_in_memory();
    let executor = DockerToolExecutor::new(images);
    for bundle in ToolBundle::ALL {
        let execution = executor.execute(&store, version_plan(bundle)).await?;
        if execution.exit_code != Some(0) {
            return Err(format!(
                "{bundle} version command failed with status {:?}: {}",
                execution.exit_code,
                String::from_utf8_lossy(&execution.stderr).trim()
            )
            .into());
        }
        let version = if execution.stdout.is_empty() {
            &execution.stderr
        } else {
            &execution.stdout
        };
        let first_line = String::from_utf8_lossy(version);
        println!(
            "{bundle} version: {}",
            first_line.lines().next().unwrap_or("<no output>")
        );
    }
    Ok(())
}

fn version_plan(bundle: ToolBundle) -> ToolExecutionPlan {
    let (program, arguments) = match bundle {
        ToolBundle::Ffmpeg => (ToolProgram::Ffmpeg, vec!["-version"]),
        ToolBundle::Gnuplot => (ToolProgram::Gnuplot, vec!["--version"]),
        ToolBundle::Graphviz => (ToolProgram::Graphviz, vec!["-V"]),
        ToolBundle::ImageMagick => (ToolProgram::ImageMagick, vec!["-version"]),
        ToolBundle::Qpdf => (ToolProgram::Qpdf, vec!["--version"]),
        ToolBundle::Poppler => (ToolProgram::PdfInfo, vec!["-v"]),
    };
    ToolExecutionPlan {
        program,
        arguments: arguments.into_iter().map(ToolArgument::literal).collect(),
        inputs: Vec::new(),
        outputs: Vec::<ExpectedOutput>::new(),
        stdin: None,
    }
}

fn cleanup_tool_containers(include_running: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = vec![
        "container",
        "ls",
        "--all",
        "--quiet",
        "--filter",
        "label=rex.workflow=true",
    ];
    if !include_running {
        for status in ["created", "exited", "dead"] {
            arguments.extend(["--filter", status_filter(status)]);
        }
    }
    let output = docker_command(arguments.iter().map(OsStr::new))?;
    let output = ensure_docker_success("list Rex tool containers", output)?;
    let ids: Vec<_> = String::from_utf8(output.stdout)?
        .lines()
        .map(str::to_owned)
        .collect();
    if ids.is_empty() {
        println!("No matching Rex tool containers found.");
        return Ok(());
    }

    let mut remove = Command::new("docker");
    remove.args(["container", "rm", "--force"]).args(&ids);
    let output = remove.stdin(Stdio::null()).output()?;
    ensure_docker_success("remove Rex tool containers", output)?;
    println!("Removed {} Rex tool container(s).", ids.len());
    Ok(())
}

fn status_filter(status: &str) -> &'static str {
    match status {
        "created" => "status=created",
        "exited" => "status=exited",
        "dead" => "status=dead",
        _ => unreachable!("fixed Docker status filter"),
    }
}

fn docker_command<I, S>(arguments: I) -> Result<Output, std::io::Error>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new("docker")
        .args(arguments)
        .stdin(Stdio::null())
        .output()
}

fn ensure_docker_success(action: &str, output: Output) -> Result<Output, String> {
    if output.status.success() {
        return Ok(output);
    }
    Err(format!(
        "{action} failed with status {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    ))
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
        SubCommand::Tools(sub) => sub.run().await?,
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
        assert!(!tools.images.is_configured());
        assert!(tools.state(Store::new_in_memory()).is_ok());
    }

    #[test]
    fn run_configures_locally_built_images_by_default() {
        let tools = parse_run(&["rex", "run", "workflow.rex", "--tool-executor", "docker"]);

        assert_eq!(tools.tool_executor, ToolExecutorChoice::Docker);
        let images = tools.images.docker_images().unwrap();
        assert!(images.validate().is_ok());
        assert!(images.allows_tags());
        assert_eq!(images.image(ToolBundle::Ffmpeg), "rex-tool-ffmpeg:local");
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
            "registry.example/ffmpeg@sha256:1111111111111111111111111111111111111111111111111111111111111111",
            "--docker-gnuplot-image",
            "registry.example/gnuplot@sha256:6666666666666666666666666666666666666666666666666666666666666666",
            "--docker-graphviz-image",
            "registry.example/graphviz@sha256:5555555555555555555555555555555555555555555555555555555555555555",
            "--docker-imagemagick-image",
            "registry.example/imagemagick@sha256:2222222222222222222222222222222222222222222222222222222222222222",
            "--docker-qpdf-image",
            "registry.example/qpdf@sha256:3333333333333333333333333333333333333333333333333333333333333333",
            "--docker-poppler-image",
            "registry.example/poppler@sha256:4444444444444444444444444444444444444444444444444444444444444444",
        ]);

        let images = tools.images.docker_images().unwrap();
        assert_eq!(
            images.image(ToolBundle::Ffmpeg),
            "registry.example/ffmpeg@sha256:1111111111111111111111111111111111111111111111111111111111111111"
        );
        assert_eq!(
            images.image(ToolBundle::Gnuplot),
            "registry.example/gnuplot@sha256:6666666666666666666666666666666666666666666666666666666666666666"
        );
        assert_eq!(
            images.image(ToolBundle::Graphviz),
            "registry.example/graphviz@sha256:5555555555555555555555555555555555555555555555555555555555555555"
        );
        assert_eq!(
            images.image(ToolBundle::ImageMagick),
            "registry.example/imagemagick@sha256:2222222222222222222222222222222222222222222222222222222222222222"
        );
        assert_eq!(
            images.image(ToolBundle::Qpdf),
            "registry.example/qpdf@sha256:3333333333333333333333333333333333333333333333333333333333333333"
        );
        assert_eq!(
            images.image(ToolBundle::Poppler),
            "registry.example/poppler@sha256:4444444444444444444444444444444444444444444444444444444444444444"
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

    #[test]
    fn tools_build_and_inspect_commands_parse() {
        let build = Args::try_parse_from(["rex", "tools", "build"]).unwrap();
        assert!(matches!(
            build.subcommand,
            SubCommand::Tools(ToolsSubCommand::Build)
        ));

        let inspect = Args::try_parse_from(["rex", "tools", "inspect"]).unwrap();
        assert!(matches!(
            inspect.subcommand,
            SubCommand::Tools(ToolsSubCommand::Inspect { .. })
        ));
    }
}
