use std::io::Write;

use clap::Parser;
use fancy_display::FancyDisplay;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_api::WorkspaceContext;
use pixi_consts::consts;
use pixi_core::WorkspaceLocator;
use pixi_manifest::EnvironmentName;
use pixi_manifest::HasFeaturesIter;

use crate::{cli_config::WorkspaceConfig, cli_interface::CliInterface};

/// Commands to manage workspace environments.
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    /// The subcommand to execute
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Parser, Debug)]
pub struct AddArgs {
    /// The name of the environment to add.
    pub name: EnvironmentName,

    /// Features to add to the environment.
    #[arg(short, long = "feature")]
    pub features: Option<Vec<String>>,

    /// The solve-group to add the environment to.
    #[clap(long)]
    pub solve_group: Option<String>,

    /// Don't include the default feature in the environment.
    #[clap(default_value = "false", long)]
    pub no_default_feature: bool,

    /// Update the manifest even if the environment already exists.
    #[clap(default_value = "false", long)]
    pub force: bool,
}

#[derive(Parser, Debug, Default)]
pub struct RemoveArgs {
    /// The name of the environment to remove
    pub name: String,
}

#[derive(Parser, Debug)]
pub enum Command {
    /// Adds an environment to the manifest file.
    #[clap(visible_alias = "a")]
    Add(AddArgs),
    /// List the environments in the manifest file.
    #[clap(visible_alias = "ls")]
    List,
    /// Remove an environment from the manifest file.
    #[clap(visible_alias = "rm")]
    Remove(RemoveArgs),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    let workspace_ctx = WorkspaceContext::new(CliInterface {}, workspace);

    match args.command {
        Command::List => {
            let envs = workspace_ctx.list_environments().await;
            writeln!(
                std::io::stdout(),
                "Environments:\n{}",
                envs.iter().format_with("\n", |e, f| f(&format_args!(
                    "- {}: \n    features: {}{}",
                    e.name().fancy_display(),
                    e.features().map(|f| f.name.fancy_display()).format(", "),
                    if let Some(solve_group) = e.solve_group() {
                        format!(
                            "\n    solve_group: {}",
                            consts::SOLVE_GROUP_STYLE.apply_to(solve_group.name())
                        )
                    } else {
                        "".to_string()
                    }
                )))
            )
            .inspect_err(|e| {
                if e.kind() == std::io::ErrorKind::BrokenPipe {
                    std::process::exit(0);
                }
            })
            .into_diagnostic()?;
        }
        Command::Add(args) => {
            workspace_ctx
                .add_environment(
                    args.name,
                    args.features,
                    args.solve_group,
                    args.no_default_feature,
                    args.force,
                )
                .await?
        }
        Command::Remove(args) => workspace_ctx.remove_environment(&args.name).await?,
    }

    Ok(())
}
