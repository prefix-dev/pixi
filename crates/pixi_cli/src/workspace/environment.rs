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
    pub config_source: pixi_config::ConfigSourceCli,

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

#[derive(Parser, Debug, Default)]
pub struct ListArgs {
    /// Output the environment names in machine readable format (space
    /// delimited). This output is used for autocomplete.
    #[arg(long, hide(true))]
    pub machine_readable: bool,
}

#[derive(Parser, Debug)]
pub enum Command {
    /// Adds an environment to the manifest file.
    #[clap(visible_alias = "a")]
    Add(AddArgs),
    /// List the environments in the manifest file.
    #[clap(visible_alias = "ls")]
    List(ListArgs),
    /// Remove an environment from the manifest file.
    #[clap(visible_alias = "rm")]
    Remove(RemoveArgs),
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_global_config_source(args.config_source.source())
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    let workspace_ctx = WorkspaceContext::new(CliInterface {}, workspace);

    match args.command {
        Command::List(list_args) => {
            let envs = workspace_ctx.list_environments().await;
            if list_args.machine_readable {
                let names = envs.iter().map(|e| e.name().as_str()).join(" ");
                writeln!(std::io::stdout(), "{names}")
                    .inspect_err(|e| {
                        if e.kind() == std::io::ErrorKind::BrokenPipe {
                            std::process::exit(0);
                        }
                    })
                    .into_diagnostic()?;
                return Ok(());
            }
            writeln!(std::io::stdout(), "{}", format_environment_list(&envs))
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

/// Renders the `Environments:` block shown by
/// `pixi workspace environment list`.
fn format_environment_list(envs: &[pixi_core::workspace::Environment<'_>]) -> String {
    format!(
        "Environments:\n{}",
        envs.iter().format_with("\n", |e, f| {
            // Content the environment defines inline lives on its
            // synthesized feature; render it under the environment.
            let inline_details = e
                .features()
                .find(|feature| feature.name.is_environment())
                .map(super::feature_detail_lines)
                .unwrap_or_default();

            f(&format_args!(
                "- {}:\n    features: {}{}{}",
                e.name().fancy_display(),
                e.features()
                    .filter(|f| !f.name.is_environment())
                    .map(|f| f.name.fancy_display())
                    .format(", "),
                if let Some(solve_group) = e.solve_group() {
                    format!(
                        "\n    solve_group: {}",
                        consts::SOLVE_GROUP_STYLE.apply_to(solve_group.name())
                    )
                } else {
                    "".to_string()
                },
                if inline_details.is_empty() {
                    "".to_string()
                } else {
                    format!("\n{}", inline_details.join("\n"))
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

    #[test]
    fn environment_list_shows_inline_content() {
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

            [environments.dev.tasks]
            greet = "echo hello"
            "#,
        )
        .unwrap();

        insta::assert_snapshot!(format_environment_list(&workspace.environments()), @r"
        Environments:
        - default:
            features: default
        - lint:
            features: lint, default
        - dev:
            features: default
            dependencies: git
            tasks: greet
        ");
    }
}
