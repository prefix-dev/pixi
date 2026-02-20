use std::io::Write;
use std::path::PathBuf;

use crate::cli_config::WorkspaceConfig;
use clap::Parser;
use miette::IntoDiagnostic;
use pixi_api::workspace::WorkspaceRegistry;
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

pub async fn execute(args: Args) -> miette::Result<()> {
    match args.command {
        Some(Command::List(args)) => {
            let workspace_registry = WorkspaceRegistry::load()?;
            let workspaces = workspace_registry.named_workspaces_map();
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
            let mut workspace_registry = WorkspaceRegistry::load()?;
            workspace_registry
                .remove_workspace(&remove_args.name)
                .await?;

            eprintln!(
                "{} Workspace '{}' has been removed from the registry successfully.",
                console::style(console::Emoji("✔ ", "")).green(),
                &remove_args.name
            );
        }
        // Some(Command::Prune(_)) => {
        //     let workspace_registry = WorkspaceRegistry::load().await?;
        //     let workspaces = workspace_registry.named_workspaces_map();
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

            let mut workspace_registry = WorkspaceRegistry::load()?;
            workspace_registry
                .add_workspace(target_name, target_path)
                .await?;

            eprintln!(
                "{} {}",
                console::style(console::Emoji("✔ ", "")).green(),
                console::style("Workspace registered successfully.").bold()
            );
        }
    };
    Ok(())
}
