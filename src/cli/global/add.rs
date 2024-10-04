use crate::cli::global::revert_environment_after_error;
use crate::global::{EnvironmentName, Mapping, Project};
use clap::Parser;
use itertools::Itertools;
use miette::Context;
use pixi_config::{Config, ConfigCli};
use rattler_conda_types::{MatchSpec, Matches};

/// Adds dependencies to an environment
///
/// Example:
/// pixi global add -e python numpy
/// pixi global add -e my_env pytest pytest-cov --expose pytest=pytest
#[derive(Parser, Debug, Clone)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    /// Packages match specs to install
    packages: Vec<MatchSpec>,

    /// Specifies the environment that the dependencies need to be added to.
    #[clap(short = 'e', long = "environment", required = true)]
    environment: EnvironmentName,

    /// Add one or more `MAPPING` for environment `ENV` which describe which executables are exposed.
    /// The syntax for `MAPPING` is `exposed_name=executable_name`, so for example `python3.10=python`.
    #[arg(long)]
    expose: Vec<Mapping>,

    /// Answer yes to all questions.
    #[clap(short = 'y', long = "yes", long = "assume-yes")]
    assume_yes: bool,

    #[clap(flatten)]
    config: ConfigCli,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let project_original = Project::discover_or_create(args.assume_yes)
        .await?
        .with_cli_config(config.clone());

    async fn apply_changes(
        env_name: &EnvironmentName,
        specs: &[MatchSpec],
        expose: &[Mapping],
        project: &mut Project,
    ) -> miette::Result<()> {
        // Add specs to the manifest
        for spec in specs {
            project.manifest.add_dependency(
                env_name,
                spec,
                project.clone().config().global_channel_config(),
            )?;
        }

        // Add expose mappings to the manifest
        for mapping in expose {
            project.manifest.add_exposed_mapping(env_name, mapping)?;
        }

        // Sync environment
        project.sync_environment(env_name).await?;

        // Figure out version of the added packages
        let added_package_records = project
            .environment_prefix(env_name)
            .await?
            .find_installed_packages(None)
            .await?
            .into_iter()
            .filter(|r| specs.iter().any(|s| s.matches(&r.repodata_record)))
            .map(|r| r.repodata_record.package_record)
            .collect_vec();

        for record in added_package_records {
            println!(
                "{}Added package {}",
                console::style(console::Emoji("✔ ", "")).green(),
                record
            );
        }

        project.manifest.save().await?;
        Ok(())
    }

    let mut project = project_original.clone();

    if let Err(err) = apply_changes(
        &args.environment,
        args.packages.as_slice(),
        args.expose.as_slice(),
        &mut project,
    )
    .await
    {
        revert_environment_after_error(&args.environment, &project_original)
            .await
            .wrap_err(format!(
                "Could not add {:?}. Reverting also failed.",
                args.packages
            ))?;
        return Err(err);
    }

    Ok(())
}
