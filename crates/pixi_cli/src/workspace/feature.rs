use std::io::Write;

use clap::Parser;
use fancy_display::FancyDisplay;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_api::WorkspaceContext;
use pixi_core::WorkspaceLocator;
use pixi_manifest::FeatureName;

use crate::{cli_config::WorkspaceConfig, cli_interface::CliInterface};

/// Commands to manage workspace features.
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    /// The subcommand to execute
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Parser, Debug)]
pub struct RemoveArgs {
    /// The name of the feature to remove
    pub feature: FeatureName,
}

#[derive(Parser, Debug)]
pub enum Command {
    /// List the features in the manifest file.
    #[clap(visible_alias = "ls")]
    List,
    /// Remove a feature from the manifest file.
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
            let features = workspace_ctx.list_features().await;
            writeln!(
                std::io::stdout(),
                "Features:\n{}",
                features.iter().format_with("\n", |(name, feature), f| {
                    let deps: Vec<_> = feature
                        .dependencies(pixi_manifest::SpecType::Run, None)
                        .map(|d| d.names().map(|n| n.as_normalized().to_string()).collect())
                        .unwrap_or_default();

                    let pypi_deps: Vec<_> = feature
                        .pypi_dependencies(None)
                        .map(|d| d.names().map(|n| n.as_source().to_string()).collect())
                        .unwrap_or_default();

                    let tasks: Vec<_> = feature
                        .targets
                        .default()
                        .tasks
                        .keys()
                        .map(|k| k.as_str().to_string())
                        .collect();

                    let mut details = Vec::new();

                    if !deps.is_empty() {
                        details.push(format!("    dependencies: {}", deps.join(", ")));
                    }
                    if !pypi_deps.is_empty() {
                        details.push(format!("    pypi-dependencies: {}", pypi_deps.join(", ")));
                    }
                    if !tasks.is_empty() {
                        details.push(format!("    tasks: {}", tasks.join(", ")));
                    }

                    f(&format_args!(
                        "- {}{}",
                        name.fancy_display(),
                        if !details.is_empty() {
                            format!(":\n{}", details.join("\n"))
                        } else {
                            String::new()
                        }
                    ))
                })
            )
            .inspect_err(|e| {
                if e.kind() == std::io::ErrorKind::BrokenPipe {
                    std::process::exit(0);
                }
            })
            .into_diagnostic()?;
        }
        Command::Remove(args) => {
            workspace_ctx.remove_feature(&args.feature).await?;
        }
    }

    Ok(())
}
