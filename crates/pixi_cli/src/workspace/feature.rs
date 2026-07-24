use std::io::Write;

use clap::Parser;
use fancy_display::FancyDisplay;
use indexmap::IndexMap;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_api::WorkspaceContext;
use pixi_core::WorkspaceLocator;
use pixi_manifest::{Feature, FeatureName};

use crate::{cli_config::WorkspaceConfig, cli_interface::CliInterface};

/// Commands to manage workspace features.
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    pub config_source: pixi_config::ConfigSourceCli,

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
        .with_deprecation_warnings(true)
        .with_global_config_source(args.config_source.source())
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    let workspace_ctx = WorkspaceContext::new(CliInterface {}, workspace);

    match args.command {
        Command::List => {
            let features = workspace_ctx.list_features().await;
            writeln!(std::io::stdout(), "{}", format_feature_list(&features))
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

/// Renders the `Features:` block shown by `pixi workspace feature list`.
fn format_feature_list(features: &IndexMap<FeatureName, Feature>) -> String {
    format!(
        "Features:\n{}",
        features.iter().format_with("\n", |(name, feature), f| {
            let details = super::feature_detail_lines(feature);

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
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use pixi_core::Workspace;

    use super::*;

    #[tokio::test]
    async fn feature_list_hides_inline_environments() {
        let workspace = Workspace::from_str(
            Path::new("pixi.toml"),
            r#"
            [workspace]
            name = "test"
            channels = []
            platforms = ["linux-64"]

            [feature.lint.dependencies]
            ruff = "*"

            [environments]
            lint = ["lint"]

            [environments.dev.dependencies]
            git = "*"
            "#,
        )
        .unwrap();
        let workspace_ctx = WorkspaceContext::new(CliInterface {}, workspace);

        let features = workspace_ctx.list_features().await;

        insta::assert_snapshot!(format_feature_list(&features), @r"
        Features:
        - default
        - lint:
            dependencies: ruff
        ");
    }
}
