use std::path::PathBuf;

use clap::Parser;

use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_conda_types::{MatchSpec, Platform};

use crate::utils::conda_environment_file::{CondaEnvDep, CondaEnvFile};
use crate::{project, HasFeatures, Project};

// enum to select version spec formatting
#[derive(clap::ValueEnum, Clone, Debug)]
pub enum VersionSpec {
    Manifest,
    // Locked,
    None,
}

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

    /// Dependency spec output method
    #[arg(long, default_value = "manifest", value_enum)]
    pub version_spec: VersionSpec,
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
        .map(|(name, spec)| match args.version_spec {
            VersionSpec::Manifest => {
                CondaEnvDep::Conda(MatchSpec::from_nameless(spec, Some(name)).to_string())
            }
            _ => CondaEnvDep::Conda(name.as_source().to_string()),
        })
        .collect_vec();

    let pypi_dependencies = environment
        .pypi_dependencies(Some(platform))
        .into_specs()
        .map(|(name, spec)| match args.version_spec {
            VersionSpec::Manifest => {
                let requirement = spec
                    .as_pep508(name.as_normalized(), project.root())
                    .into_diagnostic()
                    .unwrap();
                return match &requirement {
                    project::manifest::python::RequirementOrEditable::Editable(
                        _package_name,
                        requirements_txt,
                    ) => {
                        let relative_path = requirements_txt
                            .path
                            .as_path()
                            .strip_prefix(project.manifest_path().parent().unwrap());
                        format!("-e ./{}", relative_path.unwrap().to_string_lossy())
                    }
                    _ => requirement.to_string(),
                };
            }
            _ => name.as_source().to_string(),
        })
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
