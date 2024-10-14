use crate::cli::global::revert_environment_after_error;
use crate::cli::has_specs::HasSpecs;
use crate::global::{EnvironmentName, ExposedName, Project, StateChanges};
use clap::Parser;
use itertools::Itertools;
use miette::Context;
use pixi_config::{Config, ConfigCli};
use rattler_conda_types::MatchSpec;
use std::str::FromStr;

/// Removes dependencies from an environment
///
/// Example:
/// - pixi global remove --environment python numpy
#[derive(Parser, Debug)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    /// Specifies the packages that are to be removed.
    #[arg(num_args = 1..)]
    packages: Vec<String>,

    /// Specifies the environment that the dependencies need to be removed from.
    #[clap(short, long, required = true)]
    environment: EnvironmentName,

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
        project: &mut Project,
    ) -> miette::Result<StateChanges> {
        // Remove specs from the manifest
        for spec in specs {
            project.manifest.remove_dependency(env_name, spec)?;
        }

        // Figure out which package the exposed binaries belong to
        let prefix = project.environment_prefix(env_name).await?;

        for spec in specs {
            if let Some(name) = spec.clone().name {
                // If the package is not existent, don't try to remove executables
                if let Ok(record) = prefix.find_designated_package(&name).await {
                    prefix
                        .find_executables(&[record])
                        .into_iter()
                        .filter_map(|(name, _path)| ExposedName::from_str(name.as_str()).ok())
                        .for_each(|exposed_name| {
                            project
                                .manifest
                                .remove_exposed_name(env_name, &exposed_name)
                                .ok();
                        });
                }
            }
        }

        // Sync environment
        let state_changes = project.sync_environment(env_name).await?;

        project.manifest.save().await?;
        Ok(state_changes)
    }

    let mut project = project_original.clone();
    let specs = args
        .specs()?
        .into_iter()
        .map(|(_, specs)| specs)
        .collect_vec();

    match apply_changes(&args.environment, specs.as_slice(), &mut project)
        .await
        .wrap_err(format!(
            "Couldn't remove packages from {}",
            &args.environment
        )) {
        Ok(state_changes) => {
            state_changes.report();
        }
        Err(err) => {
            revert_environment_after_error(&args.environment, &project_original)
                .await
                .wrap_err(format!(
                    "Could not remove {:?}. Reverting also failed.",
                    args.packages
                ))?;
            return Err(err);
        }
    }

    Ok(())
}
