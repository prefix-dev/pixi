use std::collections::HashSet;

use clap::Parser;
use pep508_rs::Requirement;
use pixi_api::{
    WorkspaceContext,
    workspace::{DependencyOptions, GitOptions},
};
use pixi_config::ConfigCli;
use pixi_core::{DependencyType, WorkspaceLocator, workspace::PypiDeps};
use pixi_pypi_spec::{PixiPypiSource, PixiPypiSpec, PypiPackageName};
use url::Url;

use crate::{
    cli_config::{DependencyConfig, LockFileUpdateConfig, NoInstallConfig, WorkspaceConfig},
    cli_interface::CliInterface,
    has_specs::HasSpecs,
};

/// Adds dependencies to the workspace
///
/// The dependencies should be defined as MatchSpec for conda package, a PyPI
/// requirement for the `--pypi` dependencies, or an absolute path to a local
/// `.conda` or `.tar.bz2` package file. If no specific version is provided,
/// the latest version compatible with your workspace will be chosen
/// automatically or a * will be used.
///
/// Example usage:
///
/// - `pixi add python=3.9`: This will select the latest minor version that
///   complies with 3.9.*, i.e., python version 3.9.0, 3.9.1, 3.9.2, etc.
/// - `pixi add python`: In absence of a specified version, the latest version
///   will be chosen. For instance, this could resolve to python version
///   3.11.3.* at the time of writing.
///
/// Adding multiple dependencies at once is also supported:
///
/// - `pixi add python pytest`: This will add both `python` and `pytest` to the
///   workspace's dependencies.
///
/// The `--platform` and `--build/--host` flags make the dependency target
/// specific.
///
/// - `pixi add python --platform linux-64 --platform osx-arm64`: Will add the
///   latest version of python for linux-64 and osx-arm64 platforms.
/// - `pixi add python --build`: Will add the latest version of python for as a
///   build dependency.
///
/// Mixing `--platform` and `--build`/`--host` flags is supported
///
/// The `--pypi` option will add the package as a pypi dependency. This cannot
/// be mixed with the conda dependencies
///
/// - `pixi add --pypi boto3`
/// - `pixi add --pypi "boto3==version"`
///
/// If the workspace manifest is a `pyproject.toml`, adding a pypi dependency will
/// add it to the native pyproject `project.dependencies` array or to the native
/// `dependency-groups` table if a feature is specified:
///
/// - `pixi add --pypi boto3` will add `boto3` to the `project.dependencies`
///   array
/// - `pixi add --pypi boto3 --feature aws` will add `boto3` to the
///   `dependency-groups.aws` array
/// - `pixi add --pypi --editable 'boto3 @ file://absolute/path/to/boto3'` will add
///   the local editable `boto3` to the `pypi-dependencies` array
///
/// Note that if `--platform` or `--editable` are specified, the pypi dependency
/// will be added to the `tool.pixi.pypi-dependencies` table instead as native
/// arrays have no support for platform-specific or editable dependencies.
///
/// These dependencies will then be read by pixi as if they had been added to
/// the pixi `pypi-dependencies` tables of the default or of a named feature.
///
/// The versions will be automatically added with a pinning strategy based on
/// semver or the pinning strategy set in the config. There is a list of
/// packages that are not following the semver versioning scheme but will use
/// the minor version by default:
/// Python, Rust, Julia, GCC, GXX, GFortran, NodeJS, Deno, R, R-Base, Perl
#[derive(Parser, Debug, Default)]
#[clap(arg_required_else_help = true, verbatim_doc_comment)]
pub struct Args {
    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    #[clap(flatten)]
    pub dependency_config: DependencyConfig,

    #[clap(flatten)]
    pub no_install_config: NoInstallConfig,

    #[clap(flatten)]
    pub lock_file_update_config: LockFileUpdateConfig,

    #[clap(flatten)]
    pub config: ConfigCli,

    #[clap(flatten)]
    pub config_source: pixi_config::ConfigSourceCli,

    /// Whether the pypi requirement should be editable
    #[arg(long, requires = "pypi")]
    pub editable: bool,

    /// The PyPI index URL to use for this dependency.
    /// Only applicable when adding pypi dependencies.
    #[clap(long, requires = "pypi", conflicts_with = "git")]
    pub index: Option<Url>,
}

impl TryFrom<&Args> for DependencyOptions {
    type Error = miette::Error;

    fn try_from(args: &Args) -> miette::Result<Self> {
        Ok(DependencyOptions {
            feature: args.dependency_config.feature.clone(),
            platforms: args.dependency_config.platforms.clone(),
            no_install: args.no_install_config.no_install,
            lock_file_usage: args.lock_file_update_config.lock_file_usage()?,
        })
    }
}

impl From<&Args> for GitOptions {
    fn from(args: &Args) -> Self {
        GitOptions {
            git: args.dependency_config.git.clone(),
            reference: args
                .dependency_config
                .rev
                .clone()
                .unwrap_or_default()
                .into(),
            subdir: args.dependency_config.subdir.clone(),
        }
    }
}

fn map_pypi_requirements_with_index(
    requirements: impl Iterator<Item = (PypiPackageName, Requirement)>,
    index: Option<&Url>,
) -> miette::Result<PypiDeps> {
    requirements
        .map(|(name, req)| {
            let pixi_spec = if let Some(index_url) = index {
                // Create spec from requirement
                let mut spec = PixiPypiSpec::try_from(req.clone())
                    .map_err(|e| miette::miette!("failed to convert requirement: {}", e))?;

                // Only apply index if this is a Registry source
                match spec.source_mut() {
                    PixiPypiSource::Registry { index, .. } => {
                        *index = Some(index_url.clone());
                        Some(spec) // Return spec only when index was actually applied
                    }
                    _ => None, // For Git, Path, etc. - index doesn't apply
                }
            } else {
                None // No index provided
            };

            Ok((name, (req, pixi_spec, None)))
        })
        .collect()
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let mut workspace = WorkspaceLocator::for_cli()
        .with_global_config_source(args.config_source.source())
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?
        .with_cli_config(args.config.clone());

    // Apply backend override if provided (primarily for testing)
    if let Some(backend_override) = args.workspace_config.backend_override.clone() {
        workspace = workspace.with_backend_override(backend_override);
    }

    let workspace_ctx = WorkspaceContext::new(CliInterface {}, workspace.clone());

    let (update_deps, skipped, parsed_names): (_, Vec<String>, Vec<String>) =
        match args.dependency_config.dependency_type() {
            DependencyType::CondaDependency(spec_type) => {
                let git_options = GitOptions {
                    git: args.dependency_config.git.clone(),
                    reference: args
                        .dependency_config
                        .rev
                        .clone()
                        .unwrap_or_default()
                        .into(),
                    subdir: args.dependency_config.subdir.clone(),
                };

                let specs = args.dependency_config.specs()?;
                let names: Vec<String> = specs
                    .keys()
                    .map(|n| n.as_normalized().to_string())
                    .collect();
                let result = workspace_ctx
                    .add_conda_deps(specs, spec_type, (&args).try_into()?, git_options)
                    .await?;
                (result.0, result.1, names)
            }
            DependencyType::PypiDependency => {
                let requirements_iter = match args
                    .dependency_config
                    .vcs_pep508_requirements(&workspace)
                    .transpose()?
                {
                    Some(vcs_reqs) => vcs_reqs.into_iter(),
                    None => args.dependency_config.pypi_deps(&workspace)?.into_iter(),
                };

                let pypi_deps =
                    map_pypi_requirements_with_index(requirements_iter, args.index.as_ref())?;

                let names: Vec<String> = pypi_deps
                    .keys()
                    .map(|n| n.as_normalized().to_string())
                    .collect();
                let result = workspace_ctx
                    .add_pypi_deps(pypi_deps, args.editable, (&args).try_into()?)
                    .await?;
                (result.0, result.1, names)
            }
        };

    let skipped_set: HashSet<&str> = skipped.iter().map(|s| s.as_str()).collect();

    for package in &skipped {
        eprintln!(
            "{}{} is already a dependency",
            console::style(console::Emoji("✔ ", "")).green(),
            console::style(package).bold(),
        );
        eprintln!(
            "  Run `{}` to get the newest compatible version",
            console::style(format!("pixi upgrade {package}"))
                .green()
                .bold(),
        );
    }

    if let Some(update_deps) = update_deps {
        let added_specs: Vec<String> = args
            .dependency_config
            .specs
            .iter()
            .zip(parsed_names.iter())
            .filter(|(_, name)| !skipped_set.contains(name.as_str()))
            .map(|(raw, _)| raw.clone())
            .collect();
        let display_config = DependencyConfig {
            specs: added_specs,
            ..args.dependency_config
        };
        display_config.display_success("Added", update_deps.implicit_constraints);
    }

    Ok(())
}
