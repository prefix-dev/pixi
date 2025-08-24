use clap::Parser;
use fancy_display::FancyDisplay;
use pixi_config::{Config, ConfigCli};
use pixi_global::list::{list_all_global_environments, list_specific_global_environment};
use pixi_global::{EnvironmentName, Project};
use std::str::FromStr;

/// Lists global environments with their dependencies and exposed commands. Can also display all packages within a specific global environment when using the --environment flag.
///
/// All environments:
///
/// - Yellow: the binaries that are exposed.
/// - Green: the packages that are explicit dependencies of the environment.
/// - Blue: the version of the installed package.
/// - Cyan: the name of the environment.
///
/// Per environment:
///
/// - Green: packages that are explicitly installed.
#[derive(Parser, Debug)]
#[clap(verbatim_doc_comment)]
pub struct Args {
    /// List only packages matching a regular expression.
    /// Without regex syntax it acts like a `contains` filter.
    #[arg()]
    pub regex: Option<String>,

    #[clap(flatten)]
    config: ConfigCli,

    /// Allows listing all the packages installed in a specific environment, with an output similar to `pixi list`.
    #[clap(short, long)]
    environment: Option<String>,

    /// Sorting strategy for the package table of an environment
    #[arg(long, default_value = "name", value_enum, requires = "environment")]
    sort_by: GlobalSortBy,
}

/// Sorting strategy for the package table
#[derive(clap::ValueEnum, Clone, Debug, Default, PartialEq)]
pub enum GlobalSortBy {
    Size,
    #[default]
    Name,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let project = Project::discover_or_create()
        .await?
        .with_cli_config(config.clone());

    if let Some(environment) = args.environment {
        let env_name = EnvironmentName::from_str(environment.as_str())?;
        // Verify that the environment is in sync with the manifest and report to the user otherwise
        if !project.environment_in_sync(&env_name).await? {
            tracing::warn!(
                "The environment {} is not in sync with the manifest, to sync run\n\tpixi global sync",
                env_name.fancy_display()
            );
        }

        list_specific_global_environment(
            &project,
            &env_name,
            args.sort_by == GlobalSortBy::Size,
            args.regex,
        )
        .await?;
    } else {
        // Verify that the environments are in sync with the manifest and report to the user otherwise
        if !project.environments_in_sync().await? {
            tracing::warn!(
                "The environments are not in sync with the manifest, to sync run\n\tpixi global sync"
            );
        }
        list_all_global_environments(&project, None, None, args.regex, true).await?;
    }

    Ok(())
}
