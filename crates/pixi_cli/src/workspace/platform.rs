use std::io::Write;
use std::str::FromStr;

use clap::Parser;
use fancy_display::FancyDisplay;
use miette::IntoDiagnostic;
use pixi_api::WorkspaceContext;
use pixi_core::WorkspaceLocator;
use rattler_conda_types::Platform;

use crate::{cli_config::WorkspaceConfig, cli_interface::CliInterface};

/// Commands to manage workspace platforms.
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    /// The subcommand to execute
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Parser, Debug, Default)]
pub struct AddArgs {
    /// The platform name(s) to add.
    #[clap(required = true, num_args=1..)]
    pub platform: Vec<String>,

    /// Don't update the environment, only add changed packages to the
    /// lock-file.
    #[clap(long)]
    pub no_install: bool,

    /// The name of the feature to add the platform to.
    #[clap(long, short)]
    pub feature: Option<String>,
}

#[derive(Parser, Debug, Default)]
pub struct RemoveArgs {
    /// The platform name to remove.
    #[clap(required = true, num_args=1.., value_name = "PLATFORM")]
    pub platforms: Vec<Platform>,

    /// Don't update the environment, only remove the platform(s) from the
    /// lock-file.
    #[clap(long)]
    pub no_install: bool,

    /// The name of the feature to remove the platform from.
    #[clap(long, short)]
    pub feature: Option<String>,
}

#[derive(Parser, Debug)]
pub enum Command {
    /// Adds a platform(s) to the workspace file and updates the lockfile.
    #[clap(visible_alias = "a")]
    Add(AddArgs),
    /// List the platforms in the workspace file.
    #[clap(visible_alias = "ls")]
    List,
    /// Remove platform(s) from the workspace file and updates the lockfile.
    #[clap(visible_alias = "rm")]
    Remove(RemoveArgs),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    let workspace_ctx = WorkspaceContext::new(CliInterface {}, workspace);

    match args.command {
        Command::Add(args) => {
            let platforms = args
                .platform
                .into_iter()
                .map(|platform_str| Platform::from_str(&platform_str))
                .collect::<Result<Vec<_>, _>>()
                .into_diagnostic()?;

            workspace_ctx
                .add_platforms(platforms, args.no_install, args.feature)
                .await
        }
        Command::List => {
            let platforms = workspace_ctx.list_platforms().await;

            for (env_name, env_platforms) in platforms {
                let _ = writeln!(
                    std::io::stdout(),
                    "{} {}",
                    console::style("Environment:").bold().bright(),
                    env_name.fancy_display()
                )
                .inspect_err(|e| {
                    if e.kind() == std::io::ErrorKind::BrokenPipe {
                        std::process::exit(0);
                    }
                });

                for platform in env_platforms {
                    let _ =
                        writeln!(std::io::stdout(), "- {}", platform.as_str()).inspect_err(|e| {
                            if e.kind() == std::io::ErrorKind::BrokenPipe {
                                std::process::exit(0);
                            }
                        });
                }
            }

            Ok(())
        }
        Command::Remove(args) => {
            workspace_ctx
                .remove_platforms(args.platforms, args.no_install, args.feature)
                .await
        }
    }
}
