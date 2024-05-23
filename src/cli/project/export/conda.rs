use std::path::PathBuf;

use clap::Parser;

use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_conda_types::{MatchSpec, Platform};

use crate::cli::LockFileUsageArgs;
use crate::lock_file::UpdateLockFileOptions;
use crate::utils::conda_environment_file::{CondaEnvDep, CondaEnvFile};
use crate::{project, HasFeatures, Project};

// enum to select version spec formatting
#[derive(clap::ValueEnum, Clone, Debug)]
pub enum VersionSpec {
    Manifest,
    Locked,
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

    #[clap(flatten)]
    pub lock_file_usage: LockFileUsageArgs,

    /// Don't install the environment for pypi solving, only update the lock-file if it can solve without installing.
    #[arg(long)]
    pub no_install: bool,
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

    if let VersionSpec::Locked = args.version_spec {
        let lock_file = project
            .up_to_date_lock_file(UpdateLockFileOptions {
                lock_file_usage: args.lock_file_usage.into(),
                no_install: args.no_install,
                ..UpdateLockFileOptions::default()
            })
            .await?;

        let locked_deps = lock_file
            .lock_file
            .environment(environment.name().as_str())
            .and_then(|env| env.packages(platform).map(Vec::from_iter))
            .unwrap_or_default();

        let mut dependencies = locked_deps
            .iter()
            .filter_map(|d| d.as_conda())
            .map(|d| CondaEnvDep::Conda(d.package_record().to_string()))
            .collect_vec();

        let mut pypi_dependencies = locked_deps
            .iter()
            .filter_map(|d| d.as_pypi())
            .filter(|d| !d.is_editable())
            .map(|d| format!("{}={}", d.data().package.name, d.data().package.version))
            .collect_vec();

        let editable_dependencies = environment
            .pypi_dependencies(Some(platform))
            .into_specs()
            .filter_map(|(name, spec)| {
                let requirement = spec
                    .as_pep508(name.as_normalized(), project.root())
                    .into_diagnostic()
                    .unwrap();
                if let project::manifest::python::RequirementOrEditable::Editable(
                    _package_name,
                    requirements_txt,
                ) = &requirement
                {
                    let relative_path = requirements_txt
                        .path
                        .as_path()
                        .strip_prefix(project.manifest_path().parent().unwrap());
                    return Some(format!("-e ./{}", relative_path.unwrap().to_string_lossy()));
                }
                None
            })
            .collect_vec();

        pypi_dependencies.extend(editable_dependencies);

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

        return Ok(());
    }

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
