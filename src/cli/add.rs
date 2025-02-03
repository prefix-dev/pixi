use std::future::IntoFuture;

use clap::Parser;
use indexmap::IndexMap;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_config::Config;
use pixi_manifest::{FeatureName, SpecType};
use pixi_progress::await_in_progress;
use pixi_spec::{GitSpec, SourceSpec};
use rattler_conda_types::{MatchSpec, PackageName, Platform};
use regex::Regex;
use tokio::time::timeout;

use super::has_specs::HasSpecs;
use crate::{
    cli::{
        cli_config::{ChannelsConfig, DependencyConfig, PrefixUpdateConfig, ProjectConfig},
        search::search_package_by_wildcard,
    },
    environment::verify_prefix_location_unchanged,
    project::{DependencyType, Project},
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
/// The versions will be automatically added with a pinning strategy based on semver
/// or the pinning strategy set in the config. There is a list of packages
/// that are not following the semver versioning scheme but will use
/// the minor version by default:
/// Python, Rust, Julia, GCC, GXX, GFortran, NodeJS, Deno, R, R-Base, Perl
#[derive(Parser, Debug, Default)]
#[clap(arg_required_else_help = true, verbatim_doc_comment)]
pub struct Args {
    #[clap(flatten)]
    pub project_config: ProjectConfig,

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
        &args.dependency_config,
        &args.prefix_update_config,
        &args.project_config,
    );

    let mut project = Project::load_or_else_discover(project_config.manifest_path.as_deref())?
        .with_cli_config(prefix_update_config.config.clone());

    // Sanity check of prefix location
    verify_prefix_location_unchanged(project.default_environment().dir().as_path()).await?;

    // Add the platform if it is not already present
    project
        .manifest
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
                .vcs_pep508_requirements(&project)
                .transpose()?
            {
                Some(vcs_reqs) => vcs_reqs
                    .into_iter()
                    .map(|(name, req)| (name, (req, None)))
                    .collect(),
                None => dependency_config
                    .pypi_deps(&project)?
                    .into_iter()
                    .map(|(name, req)| (name, (req, None)))
                    .collect(),
            };

            (match_specs, source_specs, pypi_deps)
        }
    };
    // TODO: add dry_run logic to add
    let dry_run = false;

    // Save original manifest
    let original_manifest_content =
        fs_err::read_to_string(project.manifest_path()).into_diagnostic()?;

    let update_deps = match project
        .update_dependencies(
            match_specs,
            pypi_deps,
            source_specs,
            prefix_update_config,
            &args.dependency_config.feature,
            &args.dependency_config.platforms,
            args.editable,
            dry_run,
        )
        .await
    {
        Ok(update_deps) => {
            // Write the updated manifest
            project.save()?;
            update_deps
        }
        Err(e) => {
            // Restore original manifest
            fs_err::write(project.manifest_path(), original_manifest_content).into_diagnostic()?;

            if let Some(package_name) = is_package_not_found(&e) {
                let timeout_duration = std::time::Duration::from_secs(1);
                if let Ok(Ok(Some(similar_packages))) = timeout(
                    timeout_duration,
                    search_similar_packages(&package_name, &project),
                )
                .await
                {
                    let formatted_suggestions =
                        format!("{}{}", " - ", similar_packages.join("\n - "));

                    return Err(miette::miette!(
                        help = format!("Did you mean one of these?\n{}\nTip: Run `pixi search` to explore available packages.", formatted_suggestions),
                        "{}", e
                    ).wrap_err(format!("No candidates were found for {}", package_name)));
                } else {
                    return Err(e);
                }
            }
            return Err(e);
        }
    };

    if let Some(update_deps) = update_deps {
        // Notify the user we succeeded
        dependency_config.display_success("Added", update_deps.implicit_constraints);
    }

    Project::warn_on_discovered_from_env(project_config.manifest_path.as_deref());
    Ok(())
}

// Parses the underlying error message and returns the package name
fn is_package_not_found(err: &miette::Report) -> Option<String> {
    let cause = err.root_cause().to_string();

    let pattern = r"No candidates were found for (\w+)";
    let re = Regex::new(pattern).expect("Should be able to compile the regex");

    re.captures(&cause).map(|captures| captures[1].to_string())
}

async fn search_similar_packages(
    search_term: &str,
    project: &Project,
) -> miette::Result<Option<Vec<String>>> {
    let channels = ChannelsConfig::default().resolve_from_project(Some(project))?;
    let package_name = PackageName::try_from(search_term).into_diagnostic()?;

    let client = project.authenticated_client().clone();

    let gateway = Config::load_global().gateway(client);
    let all_names = await_in_progress("loading all package names", |_| async {
        gateway
            .names(channels.clone(), [Platform::current(), Platform::NoArch])
            .await
    })
    .await
    .into_diagnostic()?;

    let repodata_query_func = |some_specs: Vec<MatchSpec>| {
        gateway
            .query(
                channels.clone(),
                [Platform::current(), Platform::NoArch],
                some_specs.clone(),
            )
            .into_future()
    };

    let search_result = search_package_by_wildcard(
        package_name,
        &format!("*{}*", search_term),
        all_names,
        repodata_query_func,
        None, // this limit only applies to whatever will be printed during the process, but not to what will be returned
        &mut std::io::empty(), // we don't want to print anything
    )
    .await;

    search_result.map(|option| {
        option.map(|records| {
            records
                .into_iter()
                .map(|record| record.package_record.name.as_normalized().to_owned())
                .take(5)
                .unique()
                .collect()
        })
    })
}
