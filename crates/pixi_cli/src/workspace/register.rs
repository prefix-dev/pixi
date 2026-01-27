use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

use crate::cli_config::WorkspaceConfig;
use clap::Parser;
use miette::IntoDiagnostic;
use pixi_config::pixi_home;
use pixi_consts::consts;
use pixi_core::WorkspaceLocator;

/// Commands to manage the registry of workspaces. Default command will add a new workspace
#[derive(Parser, Debug, Clone)]
pub struct Args {
    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    /// The subcommand to execute
    #[clap(subcommand)]
    pub command: Option<Command>,

    /// Name of the workspace to register.
    #[arg(long, short)]
    pub name: Option<String>,

    /// Path to register
    #[arg(long, short)]
    pub path: Option<PathBuf>,
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

#[derive(Parser, Debug, Default, Clone)]
pub struct PruneArgs {}

#[derive(Parser, Debug, Clone)]
pub enum Command {
    /// List the registered workspaces.
    #[clap(visible_alias = "ls")]
    List(ListArgs),
    /// Remove a workspace from registry.
    #[clap(visible_alias = "rm")]
    Remove(RemoveArgs),
    // Prune disassociated workspaces from registry.
    // #[clap(visible_alias = "pr")]
    // Prune(PruneArgs),
}

pub fn global_config_write_path() -> miette::Result<PathBuf> {
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

pub fn get_global_workspaces_map() -> miette::Result<HashMap<String, PathBuf>> {
    let global_workspaces_dir = pixi_home()
        .ok_or_else(|| miette::miette!("Could not determine PIXI_HOME"))?
        .join(consts::DEFAULT_GLOBAL_WORKSPACE_DIR);

    let mut workspaces = HashMap::new();

    if global_workspaces_dir.exists() {
        for entry in std::fs::read_dir(&global_workspaces_dir).into_diagnostic()? {
            let entry = entry.into_diagnostic()?;
            let path = entry.path().join("pixi.toml");
            if path.is_file() {
                let space = entry.file_name().into_string().unwrap();
                let full_path = std::fs::canonicalize(path).into_diagnostic()?;
                workspaces.insert(space, full_path);
            }
        }
    }

    Ok(workspaces)
}

pub async fn execute(args: Args) -> miette::Result<()> {
    match args.command {
        Some(Command::List(args)) => {
            let workspaces = get_global_workspaces_map()?;
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
        Some(Command::Remove(remove_args)) => {
            let workspace_dir = pixi_home()
                .ok_or_else(|| miette::miette!("Could not determine PIXI_HOME"))?
                .join(consts::DEFAULT_GLOBAL_WORKSPACE_DIR)
                .join(&remove_args.name);

            if workspace_dir.exists() {
                std::fs::remove_dir_all(workspace_dir).into_diagnostic()?;
                eprintln!(
                    "{} Workspace '{}' has been removed from the registry successfully.",
                    console::style(console::Emoji("✔ ", "")).green(),
                    &remove_args.name
                );
            } else {
                return Err(
                    miette::diagnostic!("Workspace '{}' is not found.", remove_args.name,).into(),
                );
            }
        }
        // Some(Command::Prune(_)) => {
        //     let mut workspaces = config.named_workspaces.clone();
        //     workspaces.retain(|key, val| {
        //         if val.exists() {
        //             true
        //         } else {
        //             eprintln!("{} {}", console::style("removed workspace").green(), key);
        //             false
        //         }
        //     });
        //     config.named_workspaces = workspaces;
        //     config.save(&to)?;
        //     eprintln!(
        //         "{} Workspace registry cleaned",
        //         console::style(console::Emoji("✔ ", "")).green(),
        //     );
        // }
        None => {
            let workspace = WorkspaceLocator::for_cli()
                .with_closest_package(false)
                .with_search_start(args.workspace_config.workspace_locator_start())
                .locate()?;

            let target_name = args
                .name
                .unwrap_or_else(|| workspace.display_name().to_string());
            let target_path = args.path.unwrap_or_else(|| workspace.root().to_path_buf());

            let workspace_dir = pixi_home()
                .ok_or_else(|| miette::miette!("Could not determine PIXI_HOME"))?
                .join(consts::DEFAULT_GLOBAL_WORKSPACE_DIR)
                .join(&target_name);

            std::os::unix::fs::symlink(&target_path, &workspace_dir).into_diagnostic()?;

            eprintln!(
                "{} {}",
                console::style(console::Emoji("✔ ", "")).green(),
                console::style("Workspace registered successfully.").bold()
            );
        }
    };
    Ok(())
}
