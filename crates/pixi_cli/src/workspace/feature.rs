use std::io::Write;

use clap::Parser;
use fancy_display::FancyDisplay;
use indexmap::IndexMap;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_core::WorkspaceLocator;
use pixi_manifest::{Feature, FeatureName};
use serde::Serialize;

use crate::{cli_config::WorkspaceConfig, cli_interface::cli_context};

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
pub struct ListArgs {
    /// Output the feature names in machine readable format (space delimited).
    /// This output is used for autocomplete.
    #[arg(long, hide(true))]
    pub machine_readable: bool,

    /// Output the features in JSON format.
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser, Debug)]
pub enum Command {
    /// List the features in the manifest file.
    #[clap(visible_alias = "ls")]
    List(ListArgs),
    /// Remove a feature from the manifest file.
    #[clap(visible_alias = "rm")]
    Remove(RemoveArgs),
}

#[derive(Serialize)]
struct FeatureInfo<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    dependencies: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pypi_dependencies: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tasks: Option<Vec<String>>,
}

impl<'a> FeatureInfo<'a> {
    fn from_feature(name: &'a FeatureName, feature: &'a Feature) -> Self {
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

        Self {
            name: name.as_str(),
            dependencies: if deps.is_empty() { None } else { Some(deps) },
            pypi_dependencies: if pypi_deps.is_empty() { None } else { Some(pypi_deps) },
            tasks: if tasks.is_empty() { None } else { Some(tasks) },
        }
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_global_config_source(args.config_source.source())
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    let workspace_ctx = cli_context(workspace);

    match args.command {
        Command::List(list_args) => {
            let features = workspace_ctx.list_features().await;
            if list_args.machine_readable {
                let output = features.keys().map(FeatureName::as_str).join(" ");
                pixi_utils::io::ignore_broken_pipe(writeln!(std::io::stdout(), "{output}"))
                    .into_diagnostic()?;
                return Ok(());
            }

            if list_args.json {
                let feature_infos: Vec<_> = features
                    .iter()
                    .map(|(name, feature)| FeatureInfo::from_feature(name, feature))
                    .collect();
                pixi_utils::io::ignore_broken_pipe(writeln!(
                    std::io::stdout(),
                    "{}",
                    serde_json::to_string_pretty(&feature_infos).into_diagnostic()?
                ))
                .into_diagnostic()?;
                return Ok(());
            }

            let output = format_feature_list(&features);
            pixi_utils::io::ignore_broken_pipe(writeln!(std::io::stdout(), "{output}"))
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
        let workspace_ctx = cli_context(workspace);

        let features = workspace_ctx.list_features().await;

        insta::assert_snapshot!(format_feature_list(&features), @r"
        Features:
        - default
        - lint:
            dependencies: ruff
        ");
    }

    #[tokio::test]
    async fn feature_list_json() {
        let workspace = Workspace::from_str(
            Path::new("pixi.toml"),
            r#"
            [workspace]
            name = "test"
            channels = []
            platforms = ["linux-64"]

            [feature.lint.dependencies]
            ruff = "*"
            
            [feature.test.pypi-dependencies]
            pytest = "*"
            "#,
        )
        .unwrap();
        let workspace_ctx = cli_context(workspace);

        let features = workspace_ctx.list_features().await;
        
        let feature_infos: Vec<_> = features
            .iter()
            .map(|(name, feature)| FeatureInfo::from_feature(name, feature))
            .collect();
            
        let json = serde_json::to_string_pretty(&feature_infos).unwrap();

        insta::assert_snapshot!(json, @r###"
        [
          {
            "name": "default"
          },
          {
            "name": "lint",
            "dependencies": [
              "ruff"
            ]
          },
          {
            "name": "test",
            "pypi_dependencies": [
              "pytest"
            ]
          }
        ]
        "###);
    }
}
