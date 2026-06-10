use clap::Parser;
use itertools::Itertools;
use miette::Context;
use pixi_config::{Config, ConfigCli};
use pixi_global::{EnvironmentName, ExposedName, Project, StateChange, StateChanges};
use pixi_pypi_spec::PypiPackageName;
use rattler_conda_types::MatchSpec;
use std::str::FromStr;

use crate::global::revert_environment_after_error;
use crate::has_specs::HasSpecs;

/// Removes dependencies from an environment
///
/// Use `pixi global uninstall` to remove the whole environment
///
/// Example: `pixi global remove --environment python numpy`
#[derive(Parser, Debug)]
#[clap(arg_required_else_help = true, verbatim_doc_comment)]
pub struct Args {
    /// Specifies the package that should be removed.
    #[arg(num_args = 1.., required = true, value_name = "PACKAGE")]
    packages: Vec<String>,

    /// Specifies the environment that the dependencies need to be removed from.
    #[clap(short, long)]
    environment: Option<EnvironmentName>,

    #[clap(flatten)]
    config: ConfigCli,
}

impl HasSpecs for Args {
    fn packages(&self) -> Vec<&str> {
        self.packages.iter().map(AsRef::as_ref).collect()
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let Some(env_name) = &args.environment else {
        miette::bail!(
            "`--environment` is required. Try `pixi global uninstall {}` if you want to delete whole environments",
            args.packages.join(" ")
        );
    };
    let config = Config::with_cli_config(&args.config);
    let project_original = Project::discover_or_create()
        .await?
        .with_cli_config(config.clone());

    if project_original.environment(env_name).is_none() {
        miette::bail!(
            "Environment {} doesn't exist. You can create a new environment with `pixi global install`.",
            env_name
        );
    }

    async fn apply_changes(
        env_name: &EnvironmentName,
        specs: &[MatchSpec],
        project: &mut Project,
    ) -> miette::Result<StateChanges> {
        // Snapshot the executables of the declared PyPI dependencies while
        // they are still installed, so the exposed mappings of removed
        // packages can be cleaned up below.
        let pypi_executables = project.executables_of_pypi_dependencies(env_name).await?;

        // Remove specs from the manifest. A name that is not a conda
        // dependency may refer to a PyPI dependency instead.
        let mut removed_dependencies = vec![];
        let mut removed_pypi_dependencies = vec![];
        for spec in specs {
            let package_name = spec.name.as_exact().expect("package name must be exact");
            let pypi_name = (!project
                .environment(env_name)
                .is_some_and(|env| env.dependencies.specs.contains_key(package_name)))
            .then(|| PypiPackageName::from_str(package_name.as_source()).ok())
            .flatten()
            .filter(|name| {
                project.environment(env_name).is_some_and(|env| {
                    env.pypi_dependencies
                        .keys()
                        .any(|key| key.as_normalized() == name.as_normalized())
                })
            });

            match pypi_name {
                Some(name) => {
                    project.manifest.remove_pypi_dependency(env_name, &name)?;
                    removed_pypi_dependencies.push(name);
                }
                None => project
                    .manifest
                    .remove_dependency(env_name, package_name)
                    .map(|removed_name| removed_dependencies.push(removed_name))?,
            }
        }

        // Figure out which package the exposed binaries belong to
        let prefix = project.environment_prefix(env_name).await?;

        for spec in specs {
            {
                let name = spec.name.as_exact().expect("package name must be exact");
                // If the package is not existent, don't try to remove executables
                if let Ok(record) = prefix.find_designated_package(name).await {
                    prefix
                        .find_executables(&[record])
                        .into_iter()
                        .filter_map(|executable| {
                            ExposedName::from_str(executable.name.as_str()).ok()
                        })
                        .for_each(|exposed_name| {
                            project
                                .manifest
                                .remove_exposed_name(env_name, &exposed_name)
                                .ok();
                        });
                }
            }
        }

        // Remove the exposed mappings of removed PyPI dependencies.
        for name in &removed_pypi_dependencies {
            let Some(executables) = pypi_executables.get(name) else {
                continue;
            };
            for executable in executables {
                if let Ok(exposed_name) = ExposedName::from_str(executable.name.as_str()) {
                    project
                        .manifest
                        .remove_exposed_name(env_name, &exposed_name)
                        .ok();
                }
            }
        }

        // Sync environment. A removed PyPI dependency cannot be detected by
        // the sync check (its distribution lingers in site-packages), so the
        // environment is reinstalled explicitly; the PyPI installer then
        // removes the now-extraneous distributions.
        let state_changes = if removed_pypi_dependencies.is_empty() {
            project
                .sync_environment(env_name, Some(removed_dependencies))
                .await?
        } else {
            let mut state_changes = StateChanges::new_with_env(env_name.clone());
            let mut environment_update = project.install_environment(env_name).await?;
            environment_update.add_removed_packages(removed_dependencies);
            state_changes.insert_change(
                env_name,
                StateChange::UpdatedEnvironment(environment_update),
            );
            state_changes |= project.sync_environment_expose(env_name).await?;
            state_changes
        };
        project.clear_progress();

        project.manifest.save().await?;
        Ok(state_changes)
    }

    let mut project = project_original.clone();
    let specs = args
        .specs()?
        .into_iter()
        .map(|(_, specs)| specs)
        .collect_vec();

    match apply_changes(env_name, specs.as_slice(), &mut project)
        .await
        .wrap_err(format!("Couldn't remove packages from {env_name}"))
    {
        Ok(state_changes) => {
            state_changes.report();
        }
        Err(err) => {
            if let Err(revert_err) =
                revert_environment_after_error(env_name, &project_original).await
            {
                tracing::warn!("Reverting of the operation failed");
                tracing::info!("Reversion error: {:?}", revert_err);
            }
            return Err(err);
        }
    }

    Ok(())
}
