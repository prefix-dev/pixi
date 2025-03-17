use crate::cli::global::revert_environment_after_error;
use crate::cli::has_specs::HasSpecs;
use crate::global::{EnvironmentName, Mapping, Project, StateChanges};
use clap::Parser;
use itertools::Itertools;
use pixi_config::{Config, ConfigCli};
use rattler_conda_types::MatchSpec;

/// Adds dependencies to an environment
///
/// Example:
///
/// - `pixi global add --environment python numpy`
/// - `pixi global add --environment my_env pytest pytest-cov --expose pytest=pytest`
#[derive(Parser, Debug, Clone)]
#[clap(arg_required_else_help = true, verbatim_doc_comment)]
pub struct Args {
    /// Specifies the package that should be added to the environment.
    #[arg(num_args = 1.., required = true, value_name = "PACKAGE")]
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
        specs: Vec<MatchSpec>,
        expose: &[Mapping],
        project: &mut Project,
    ) -> miette::Result<StateChanges> {
        let mut state_changes = StateChanges::new_with_env(env_name.clone());

        // Add specs to the manifest
        for spec in &specs {
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
        state_changes |= project.sync_environment(env_name, None).await?;

        // Figure out added packages and their corresponding versions
        state_changes |= project.added_packages(specs, env_name).await?;

        project.manifest.save().await?;

        Ok(state_changes)
    }

    let mut project_modified = project_original.clone();

    let specs = args
        .specs()?
        .into_iter()
        .map(|(_, specs)| specs)
        .collect_vec();

    match apply_changes(
        &args.environment,
        specs,
        args.expose.as_slice(),
        &mut project_modified,
    )
    .await
    {
        Ok(state_changes) => {
            state_changes.report();
            Ok(())
        }
        Err(err) => {
            if let Err(revert_err) =
                revert_environment_after_error(&args.environment, &project_original).await
            {
                tracing::warn!("Reverting of the operation failed");
                tracing::info!("Reversion error: {:?}", revert_err);
            }
            Err(err)
        }
    }
}
