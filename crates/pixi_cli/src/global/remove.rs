use clap::Parser;
use itertools::Itertools;
use miette::Context;
use pixi_config::{Config, ConfigCli};
use pixi_global::project::ExposedType;
use pixi_global::{EnvironmentName, Project, StateChanges};
use rattler_conda_types::MatchSpec;

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
        // Remove specs from the manifest
        let mut removed_dependencies = vec![];
        for spec in specs {
            let package_name = spec.name.as_exact().expect("package name must be exact");
            project
                .manifest
                .remove_dependency(env_name, package_name)
                .map(|removed_name| removed_dependencies.push(removed_name))?;
        }

        let mut state_changes = project
            .sync_environment_install(env_name, Some(removed_dependencies))
            .await?;
        project
            .sync_exposed_names(env_name, ExposedType::Nothing)
            .await?;
        state_changes |= project.sync_environment_expose(env_name).await?;
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
            state_changes.report(&project).await;
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
