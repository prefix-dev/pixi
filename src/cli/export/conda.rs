use std::path::PathBuf;

use clap::Parser;

use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_conda_types::Platform;

use crate::utils::conda_environment_file::{CondaEnvDep, CondaEnvFile};
use crate::{HasFeatures, Project};

/// Exports a projects dependencies as an environment.yml
///
/// The environment is printed to standard out
#[derive(Debug, Parser)]
#[clap(arg_required_else_help = false)]
pub struct Args {
    /// The platform to list packages for. Defaults to the current platform.
    #[arg(long)]
    pub platform: Option<Platform>,

    /// The path to 'pixi.toml' or 'pyproject.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// The environment to list packages for. Defaults to the default environment.
    #[arg(short, long)]
    pub environment: Option<String>,

    /// Name for environment
    #[arg(short, long)]
    pub name: Option<String>,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(args.manifest_path.as_deref())?;
    let environment = project.environment_from_name_or_env_var(args.environment)?;

    let platform = args.platform.unwrap_or_else(|| environment.best_platform());

    let name = match args.name {
        Some(arg_name) => arg_name,
        None => format!("{}-{}-{}", project.name(), environment.name(), platform),
    };

    let channels = environment
        .channels()
        .into_iter()
        .map(|channel| channel.name().to_string())
        .collect_vec();

    let mut dependencies = environment
        .dependencies(None, Some(platform))
        .into_specs()
        .map(|(name, _spec)| CondaEnvDep::Conda(name.as_source().to_string()))
        .collect_vec();

    let pypi_dependencies = environment
        .pypi_dependencies(Some(platform))
        .into_specs()
        .map(|(name, _spec)| name.as_source().to_string())
        .collect_vec();

    if !pypi_dependencies.is_empty() {
        dependencies.push(CondaEnvDep::Pip {
            pip: pypi_dependencies,
        });
    }

    let env_file = CondaEnvFile {
        name: Some(name),
        channels,
        dependencies,
    };

    let env_string = serde_yaml::to_string(&env_file).into_diagnostic()?;
    println!("{}", env_string);

    Ok(())
}
