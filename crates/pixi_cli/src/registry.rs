use std::io::Write;
use std::path::PathBuf;

use clap::Parser;
use miette::IntoDiagnostic;
use pixi_config::Config;
use pixi_core::WorkspaceLocator;

/// Commands to manage the registry of workspaces.
#[derive(Parser, Debug, Clone)]
pub struct Args {
    /// The subcommand to execute
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Parser, Debug, Default, Clone)]
pub struct AddArgs {
    /// Name of the workspace to register.
    #[arg(long, short)]
    pub name: Option<String>,

    /// The path to `pixi.toml`, `pyproject.toml`, or the workspace directory
    #[arg(long, short)]
    pub manifest_path: Option<PathBuf>,
}

#[derive(Parser, Debug, Default, Clone)]
pub struct RemoveArgs {
    /// Name of the workspace to unregister.
    #[clap(required = true)]
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

            let workspace = WorkspaceLocator::for_cli()
                .locate()?;

            let target_name = add_args.name.unwrap_or_else(|| {
                workspace.display_name().to_string()
            });
             let target_path = add_args.manifest_path.unwrap_or_else(|| {
                workspace.root().to_path_buf()
            });
            
            if workspaces.contains_key(&target_name) {
                return Err(miette::diagnostic!(
                    "Workspace with name '{}' is already registered.",
                    target_name,
                )
                .into());
            }
            workspaces.insert(target_name, target_path);
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
