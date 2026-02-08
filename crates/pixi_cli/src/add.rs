use crate::{
    cli_config::{DependencyConfig, LockFileUpdateConfig, NoInstallConfig, WorkspaceConfig},
    cli_interface::CliInterface,
    has_specs::HasSpecs,
};
use clap::Parser;
use miette::{IntoDiagnostic, WrapErr};
use pixi_api::{
    WorkspaceContext,
    workspace::{DependencyOptions, GitOptions},
};
use pixi_config::ConfigCli;
use pixi_core::{DependencyType, WorkspaceLocator};
use pixi_pypi_spec::{PixiPypiSource, PixiPypiSpec, VersionOrStar};
use url::Url;

/// Adds dependencies to the workspace
///
/// The dependencies should be defined as MatchSpec for conda package, or a PyPI
/// requirement for the `--pypi` dependencies. If no specific version is
/// provided, the latest version compatible with your workspace will be chosen
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

    /// Whether the pypi requirement should be editable
    #[arg(long, requires = "pypi")]
    pub editable: bool,

    /// The URL of the PyPI index to use for this dependency
    #[arg(long, requires = "pypi")]
    pub index: Option<String>,
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

pub async fn execute(args: Args) -> miette::Result<()> {
    let mut workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?
        .with_cli_config(args.config.clone());

    // Apply backend override if provided (primarily for testing)
    if let Some(backend_override) = args.workspace_config.backend_override.clone() {
        workspace = workspace.with_backend_override(backend_override);
    }

    let workspace_ctx = WorkspaceContext::new(CliInterface {}, workspace.clone());

    let update_deps = match args.dependency_config.dependency_type() {
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

            workspace_ctx
                .add_conda_deps(
                    args.dependency_config.specs()?,
                    spec_type,
                    (&args).try_into()?,
                    git_options,
                )
                .await?
        }
        DependencyType::PypiDependency => {
            let index_url = args
                .index
                .as_ref()
                .map(|url_str| Url::parse(url_str))
                .transpose()
                .into_diagnostic()
                .wrap_err("Failed to parse index URL")?;

            let pixi_spec = index_url.map(|url| {
                PixiPypiSpec::new(PixiPypiSource::Registry {
                    version: VersionOrStar::Star,
                    index: Some(url),
                })
            });
            let pypi_deps = match args
                .dependency_config
                .vcs_pep508_requirements(&workspace)
                .transpose()?
            {
                Some(vcs_reqs) => vcs_reqs
                    .into_iter()
                    .map(|(name, req)| (name, (req, pixi_spec.clone(), None)))
                    .collect(),
                None => args
                    .dependency_config
                    .pypi_deps(&workspace)?
                    .into_iter()
                    .map(|(name, req)| (name, (req, pixi_spec.clone(), None)))
                    .collect(),
            };

            workspace_ctx
                .add_pypi_deps(pypi_deps, args.editable, (&args).try_into()?)
                .await?
        }
    };

    if let Some(update_deps) = update_deps {
        // Notify the user we succeeded
        args.dependency_config
            .display_success("Added", update_deps.implicit_constraints);
    }

    Ok(())
}
