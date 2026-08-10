use blake3::Hash;
use clap::{CommandFactory, Parser};
use rex_workflow::{
    config::Config,
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
    },
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
            let state = State::local(store);

            let result_json = eval_rex(&source, inputs, state).await?;
            let rendered = render_result_json(&result_json, raw_output)?;
            println!("{rendered}");
        }
    }

    Ok(())
}
