use crate::cli::global::revert_environment_after_error;
use crate::cli::has_specs::HasSpecs;
use crate::global::{EnvironmentName, Mapping, Project};
use clap::Parser;
use itertools::Itertools;
use miette::Context;
use pixi_config::{Config, ConfigCli};
use rattler_conda_types::{MatchSpec, Matches};

/// Adds dependencies to an environment
///
/// Example:
/// - pixi global add --environment python numpy
/// - pixi global add --environment my_env pytest pytest-cov --expose pytest=pytest
#[derive(Parser, Debug, Clone)]
#[clap(arg_required_else_help = true, verbatim_doc_comment)]
pub struct Args {
    /// Specifies the packages that are to be added to the environment.
    #[arg(num_args = 1..)]
    packages: Vec<String>,

    /// Specifies the environment that the dependencies need to be added to.
    #[clap(short, long, required = true)]
    environment: EnvironmentName,

    /// Add one or more mapping which describe which executables are exposed.
    /// The syntax is `exposed_name=executable_name`, so for example `python3.10=python`.
    /// Alternatively, you can input only an executable_name and `executable_name=executable_name` is assumed.
    #[arg(long)]
    expose: Vec<Mapping>,

    #[clap(flatten)]
    config: ConfigCli,
}

impl HasSpecs for Args {
    fn packages(&self) -> Vec<&str> {
        self.packages.iter().map(AsRef::as_ref).collect()
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let project_original = Project::discover_or_create()
        .await?
        .with_cli_config(config.clone());

    if project_original.environment(&args.environment).is_none() {
        miette::bail!("Environment {} doesn't exist. You can create a new environment with `pixi global install`.", &args.environment);
    }

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
            eprintln!(
                "{}Added package '{}'",
                console::style(console::Emoji("✔ ", "")).green(),
                record
            );
        }

        project.manifest.save().await?;
        Ok(())
    }

    let mut project = project_original.clone();
    let specs = args
        .specs()?
        .into_iter()
        .map(|(_, specs)| specs)
        .collect_vec();

    if let Err(err) = apply_changes(
        &args.environment,
        specs.as_slice(),
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
