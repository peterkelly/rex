use blake3::Hash;
use clap::{CommandFactory, Parser};
use rex::{
    storage::{EntryKind, Store, export_blob, export_tree, import_path},
    workflow::{
        config::Config,
        run::{eval_rex, render_result_json},
        state::State,
    },
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

    /// Directory containing installed tools (defaults to the `rex` binary's directory).
    #[clap(long, env = "REX_TOOL_DIR")]
    tool_dir: Option<PathBuf>,

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
    },
}

#[derive(Parser, Debug)]
enum ToolsSubCommand {
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
            Self::Cleanup { include_running } => cleanup_tool_containers(include_running),
        }
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
                let (_kind, hash) = import_path(&store, path.as_path())
                    .await
                    .map_err(|error| error.to_string())?;
                println!("{}", hash);
            }
            StoreSubCommand::Export { hash, path } => {
                let result = match store.get_tree(hash).await {
                    Ok(_) => export_tree(&store, hash, &path).await,
                    Err(error) if error.kind() == ErrorKind::NotADirectory => {
                        export_blob(&store, hash, &path).await
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
    let tool_dir = args.tool_dir.clone().or_else(|| {
        std::env::current_exe()
            .ok()
            .and_then(|executable| executable.parent().map(std::path::Path::to_path_buf))
    });

    match args.subcommand {
        SubCommand::Store(sub) => sub.run(config).await?,
        SubCommand::Server(sub) => sub.run(config).await?,
        SubCommand::Tools(sub) => sub.run().await?,
        SubCommand::Run {
            path,
            inputs,
            raw_output,
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
            let mut state = State::without_tools(store);
            if let Some(directory) = tool_dir {
                state = state
                    .with_tool_directory(directory)
                    .with_tool_environment("REX_STORE", config.store_path.to_string_lossy());
            }

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

    #[test]
    fn run_command_has_no_tool_specific_options() {
        let args = Args::try_parse_from(["rex", "run", "workflow.rex"]).unwrap();
        assert!(matches!(args.subcommand, SubCommand::Run { .. }));
    }

    #[test]
    fn run_rejects_the_removed_local_executor_option() {
        let error =
            Args::try_parse_from(["rex", "run", "workflow.rex", "--tool-executor", "local"])
                .unwrap_err();
        assert!(error.to_string().contains("unexpected argument"));
    }

    #[test]
    fn tools_cleanup_command_parses() {
        let cleanup = Args::try_parse_from(["rex", "tools", "cleanup"]).unwrap();
        assert!(matches!(
            cleanup.subcommand,
            SubCommand::Tools(ToolsSubCommand::Cleanup {
                include_running: false
            })
        ));
    }
}
