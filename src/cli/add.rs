use clap::Parser;
use indexmap::IndexMap;
use miette::IntoDiagnostic;
use pixi_manifest::{FeatureName, SpecType};
use pixi_spec::{GitSpec, SourceSpec};
use rattler_conda_types::{MatchSpec, PackageName};

use super::has_specs::HasSpecs;
use crate::{
    cli::cli_config::{DependencyConfig, PrefixUpdateConfig, WorkspaceConfig},
    environment::sanity_check_project,
    workspace::DependencyType,
    WorkspaceLocator,
};

/// Adds dependencies to the project
///
/// The dependencies should be defined as MatchSpec for conda package, or a PyPI
/// requirement for the `--pypi` dependencies. If no specific version is
/// provided, the latest version compatible with your project will be chosen
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
/// - `pixi add python pytest`: This will add both `python` and `pytest` to the
///   project's dependencies.
///
/// The `--platform` and `--build/--host` flags make the dependency target
/// specific.
/// - `pixi add python --platform linux-64 --platform osx-arm64`: Will add the
///   latest version of python for linux-64 and osx-arm64 platforms.
/// - `pixi add python --build`: Will add the latest version of python for as a
///   build dependency.
///
/// Mixing `--platform` and `--build`/`--host` flags is supported
///
/// The `--pypi` option will add the package as a pypi dependency. This cannot
/// be mixed with the conda dependencies
/// - `pixi add --pypi boto3`
/// - `pixi add --pypi "boto3==version"
///
/// If the project manifest is a `pyproject.toml`, adding a pypi dependency will
/// add it to the native pyproject `project.dependencies` array or to the native
/// `dependency-groups` table if a feature is specified:
/// - `pixi add --pypi boto3` will add `boto3` to the `project.dependencies`
///   array
/// - `pixi add --pypi boto3 --feature aws` will add `boto3` to the
///   `dependency-groups.aws` array
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
    pub prefix_update_config: PrefixUpdateConfig,

    /// Whether the pypi requirement should be editable
    #[arg(long, requires = "pypi")]
    pub editable: bool,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let (dependency_config, prefix_update_config, project_config) = (
        args.dependency_config,
        args.prefix_update_config,
        args.workspace_config,
    );

    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(project_config.workspace_locator_start())
        .locate()?
        .with_cli_config(prefix_update_config.config.clone());

    sanity_check_project(&workspace).await?;

    let mut workspace = workspace.modify()?;

    // Add the platform if it is not already present
    workspace
        .manifest()
        .add_platforms(dependency_config.platforms.iter(), &FeatureName::Default)?;

    let (match_specs, source_specs, pypi_deps) = match dependency_config.dependency_type() {
        DependencyType::CondaDependency(spec_type) => {
            // if user passed some git configuration
            // we will use it to create pixi source specs
            let passed_specs: IndexMap<PackageName, (MatchSpec, SpecType)> = dependency_config
                .specs()?
                .into_iter()
                .map(|(name, spec)| (name, (spec, spec_type)))
                .collect();

            if let Some(git) = &dependency_config.git {
                let source_specs = passed_specs
                    .iter()
                    .map(|(name, (_spec, spec_type))| {
                        let git_reference =
                            dependency_config.rev.clone().unwrap_or_default().into();

                        let git_spec = GitSpec {
                            git: git.clone(),
                            rev: Some(git_reference),
                            subdirectory: dependency_config.subdir.clone(),
                        };
                        (name.clone(), (SourceSpec::Git(git_spec), *spec_type))
                    })
                    .collect();
                (IndexMap::default(), source_specs, IndexMap::default())
            } else {
                (passed_specs, IndexMap::default(), IndexMap::default())
            }
        }
        DependencyType::PypiDependency => {
            let match_specs = IndexMap::default();
            let source_specs = IndexMap::default();
            let pypi_deps = match dependency_config
                .vcs_pep508_requirements(workspace.workspace())
                .transpose()?
            {
                Some(vcs_reqs) => vcs_reqs
                    .into_iter()
                    .map(|(name, req)| (name, (req, None)))
                    .collect(),
                None => dependency_config
                    .pypi_deps(workspace.workspace())?
                    .into_iter()
                    .map(|(name, req)| (name, (req, None)))
                    .collect(),
            };

            (match_specs, source_specs, pypi_deps)
        }
    };
    // TODO: add dry_run logic to add
    let dry_run = false;

    let update_deps = match Box::pin(workspace.update_dependencies(
        match_specs,
        pypi_deps,
        source_specs,
        &prefix_update_config,
        &dependency_config.feature,
        &dependency_config.platforms,
        args.editable,
        dry_run,
    ))
    .await
    {
        Ok(update_deps) => {
            // Write the updated manifest
            workspace.save().await.into_diagnostic()?;
            update_deps
        }
        Err(e) => {
            workspace.revert().await.into_diagnostic()?;
            return Err(e);
        }
    };

    if let Some(update_deps) = update_deps {
        // Notify the user we succeeded
        dependency_config.display_success("Added", update_deps.implicit_constraints);
    }

    Ok(())
}
