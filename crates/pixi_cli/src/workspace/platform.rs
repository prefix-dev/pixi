use std::io::Write;
use std::str::FromStr;

use clap::Parser;
use fancy_display::FancyDisplay;
use miette::IntoDiagnostic;
use pixi_api::WorkspaceContext;
use pixi_core::WorkspaceLocator;
use pixi_manifest::HasWorkspaceManifest;
use rattler_conda_types::Platform;

use crate::{cli_config::WorkspaceConfig, cli_interface::CliInterface};

/// Commands to manage workspace platforms.
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    pub config_source: pixi_config::ConfigSourceCli,

    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Parser, Debug, Default)]
pub struct AddArgs {
    /// The platform name(s) to add.
    #[clap(required = true, num_args=1..)]
    pub platform: Vec<String>,

    /// Don't update the environment, only add changed packages to the
    /// lock file.
    #[clap(long, env = "PIXI_NO_INSTALL")]
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
    /// lock file.
    #[clap(long, env = "PIXI_NO_INSTALL")]
    pub no_install: bool,

    /// The name of the feature to remove the platform from.
    #[clap(long, short)]
    pub feature: Option<String>,
}

#[derive(Parser, Debug)]
pub enum Command {
    /// Adds a platform(s) to the workspace file and updates the lock file.
    #[clap(visible_alias = "a")]
    Add(AddArgs),
    /// List the platforms in the workspace file.
    #[clap(visible_alias = "ls")]
    List,
    /// Remove platform(s) from the workspace file and updates the lock file.
    #[clap(visible_alias = "rm")]
    Remove(RemoveArgs),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_global_config_source(args.config_source.source())
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    let workspace_ctx = WorkspaceContext::new(CliInterface {}, workspace.clone());

    match args.command {
        Command::Add(args) => {
            // `pixi workspace platform add <subdir>` registers a new
            // subdir-bound platform in the workspace. This is the one place
            // where it's legitimate to construct a PixiPlatform from a bare
            // subdir; everywhere else we look up an existing PixiPlatform.
            let platforms = args
                .platform
                .into_iter()
                .map(|platform_str| {
                    Platform::from_str(&platform_str).map(pixi_manifest::PixiPlatform::from_subdir)
                })
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
            // Look up the existing PixiPlatform for each subdir; the workspace
            // already declares them by name + virtual packages, so we must
            // preserve that identity rather than synthesize new ones.
            let workspace_platforms = (&workspace)
                .workspace_manifest()
                .workspace
                .platforms
                .clone();
            let platforms = args
                .platforms
                .iter()
                .map(|subdir| {
                    workspace_platforms
                        .iter()
                        .find(|p| p.subdir() == *subdir)
                        .cloned()
                        .ok_or_else(|| {
                            miette::miette!(
                                "workspace does not define a platform with subdir '{subdir}'"
                            )
                        })
                })
                .collect::<miette::Result<Vec<_>>>()?;
            workspace_ctx
                .remove_platforms(platforms, args.no_install, args.feature)
                .await
        }
    }
}
