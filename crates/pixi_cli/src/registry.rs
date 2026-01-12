use std::io::Write;
use std::path::PathBuf;

use clap::Parser;
use miette::IntoDiagnostic;
use pixi_config::Config;

use crate::cli_config::WorkspaceConfig;

/// Commands to manage the registry of workspaces.
#[derive(Parser, Debug, Clone)]
pub struct Args {
    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    /// The subcommand to execute
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Parser, Debug, Default, Clone)]
pub struct AddArgs {
    /// Name of the workspace to register.
    #[clap(required = true, num_args=1..)]
    pub name: String,

    /// The path to `pixi.toml`, `pyproject.toml`, or the workspace directory
    #[clap(required = true, num_args=1..)]
    pub manifest_path: PathBuf,
}

#[derive(Parser, Debug, Default, Clone)]
pub struct RemoveArgs {
    /// Name of the workspace to unregister.
    #[clap(required = true, num_args=1..)]
    pub name: String,
}

#[derive(Parser, Debug, Default, Clone)]
pub struct ListArgs {
    /// Output in JSON format
    #[arg(long)]
    json: bool,
}

#[derive(Parser, Debug, Clone)]
pub enum Command {
    /// Adds a workspace to registry.
    #[clap(visible_alias = "a")]
    Add(AddArgs),
    /// List the registered workspaces.
    #[clap(visible_alias = "ls")]
    List(ListArgs),
    /// Remove a workspace from registry.
    #[clap(visible_alias = "rm")]
    Remove(RemoveArgs),
}

fn global_config_write_path() -> miette::Result<PathBuf> {
    let mut global_locations = pixi_config::config_path_global();
    let mut to = global_locations
        .pop()
        .expect("should have at least one global config path");

    for p in global_locations {
        if p.exists() {
            to = p;
            break;
        }
    }
    Ok(to)
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let mut config = Config::load_global();
    let to = global_config_write_path()?;

    match args.command {
        Command::Add(add_args) => {
            let mut workspaces = config.named_workspaces.clone();
            if workspaces.contains_key(&add_args.name) {
                return Err(miette::diagnostic!(
                    "Workspace with name '{}' is already registered.",
                    add_args.name,
                )
                .into());
            }
            workspaces.insert(add_args.name, add_args.manifest_path);
            config.named_workspaces = workspaces;
            config.save(&to)?;
            eprintln!(
                "{} {}",
                console::style(console::Emoji("✔ ", "")).green(),
                console::style("Workspace registered successfully.").bold()
            );
        }
        Command::List(args) => {
            let workspaces = config.named_workspaces;
            let out = if args.json {
                serde_json::to_string_pretty(&workspaces).into_diagnostic()?
            } else {
                toml_edit::ser::to_string_pretty(&workspaces).into_diagnostic()?
            };
            writeln!(std::io::stdout(), "{out}")
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::BrokenPipe {
                        std::process::exit(0);
                    }
                    e
                })
                .into_diagnostic()?;
        }
        Command::Remove(remove_args) => {
            let mut workspaces = config.named_workspaces.clone();
            if workspaces.contains_key(&remove_args.name) {
                workspaces.remove(&remove_args.name);
                config.named_workspaces = workspaces;
                config.save(&to)?;
                eprintln!(
                    "{} {}",
                    console::style(console::Emoji("✔ ", "")).green(),
                    format!(
                        "Workspace '{}' has been removed from the registry successfully.",
                        &remove_args.name
                    )
                );
            } else {
                return Err(
                    miette::diagnostic!("Workspace '{}' is not found.", remove_args.name,).into(),
                );
            }
        }
    };
    Ok(())
}
