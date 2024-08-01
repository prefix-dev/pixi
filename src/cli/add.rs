use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
};

use clap::Parser;
use indexmap::IndexMap;
use itertools::Itertools;
use pep440_rs::VersionSpecifiers;
use pep508_rs::{Requirement, VersionOrUrl::VersionSpecifier};
use pixi_manifest::{pypi::PyPiPackageName, DependencyOverwriteBehavior, FeatureName, SpecType};
use rattler_conda_types::{MatchSpec, PackageName, Platform, Version};
use rattler_lock::{LockFile, Package};

use super::has_specs::HasSpecs;
use crate::cli::cli_config::{DependencyConfig, PrefixUpdateConfig, ProjectConfig};
use crate::{
    environment::verify_prefix_location_unchanged,
    load_lock_file,
    lock_file::{filter_lock_file, LockFileDerivedData, UpdateContext},
    project::{
        grouped_environment::GroupedEnvironment, has_features::HasFeatures, DependencyType, Project,
    },
};

/// Adds dependencies to the project
///
/// The dependencies should be defined as MatchSpec for conda package, or a PyPI
/// requirement for the --pypi dependencies. If no specific version is provided,
/// the latest version compatible with your project will be chosen automatically
/// or a * will be used.
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
/// The `--pypi` option will add the package as a pypi dependency. This can not
/// be mixed with the conda dependencies
/// - `pixi add --pypi boto3`
/// - `pixi add --pypi "boto3==version"
///
/// If the project manifest is a `pyproject.toml`, adding a pypi dependency will
/// add it to the native pyproject `project.dependencies` array or to the native
/// `project.optional-dependencies` table if a feature is specified:
/// - `pixi add --pypi boto3` will add `boto3` to the `project.dependencies`
///   array
/// - `pixi add --pypi boto3 --feature aws` will add `boto3` to the
///   `project.dependencies.aws` array
/// These dependencies will then be read by pixi as if they had been added to
/// the pixi `pypi-dependencies` tables of the default or of a named feature.
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
        args.dependency_config,
        args.prefix_update_config,
        args.project_config,
    );

    let mut project = Project::load_or_else_discover(project_config.manifest_path.as_deref())?
        .with_cli_config(prefix_update_config.config.clone());

    // Sanity check of prefix location
    verify_prefix_location_unchanged(project.default_environment().dir().as_path()).await?;

    // Load the current lock-file
    let lock_file = load_lock_file(&project).await?;

    // Add the platform if it is not already present
    project
        .manifest
        .add_platforms(dependency_config.platform.iter(), &FeatureName::Default)?;

    // Add the individual specs to the project.
    let mut conda_specs_to_add_constraints_for = IndexMap::new();
    let mut pypi_specs_to_add_constraints_for = IndexMap::new();
    let mut conda_packages = HashSet::new();
    let mut pypi_packages = HashSet::new();
    match dependency_config.dependency_type() {
        DependencyType::CondaDependency(spec_type) => {
            let specs = dependency_config.specs()?;
            let channel_config = project.channel_config();
            for (name, spec) in specs {
                let added = project.manifest.add_dependency(
                    &spec,
                    spec_type,
                    &dependency_config.platform,
                    &dependency_config.feature_name(),
                    DependencyOverwriteBehavior::OverwriteIfExplicit,
                    &channel_config,
                )?;
                if added {
                    if spec.version.is_none() {
                        conda_specs_to_add_constraints_for.insert(name.clone(), (spec_type, spec));
                    }
                    conda_packages.insert(name);
                }
            }
        }
        DependencyType::PypiDependency => {
            let specs = dependency_config.pypi_deps(&project)?;
            for (name, spec) in specs {
                let added = project.manifest.add_pypi_dependency(
                    &spec,
                    &dependency_config.platform,
                    &dependency_config.feature_name(),
                    Some(args.editable),
                    DependencyOverwriteBehavior::OverwriteIfExplicit,
                )?;
                if added {
                    if spec.version_or_url.is_none() {
                        pypi_specs_to_add_constraints_for.insert(name.clone(), spec);
                    }
                    pypi_packages.insert(name.as_normalized().clone());
                }
            }
        }
    }

    // Determine the environments that are affected by the change.
    let feature_name = dependency_config.feature_name();
    let affected_environments = project
        .environments()
        .iter()
        // Filter out any environment that does not contain the feature we modified
        .filter(|e| e.features().any(|f| f.name == feature_name))
        // Expand the selection to also included any environment that shares the same solve
        // group
        .flat_map(|e| {
            GroupedEnvironment::from(e.clone())
                .environments()
                .collect_vec()
        })
        .unique()
        .collect_vec();
    let default_environment_is_affected =
        affected_environments.contains(&project.default_environment());

    tracing::debug!(
        "environments affected by the add command: {}",
        affected_environments.iter().map(|e| e.name()).format(", ")
    );

    // Determine the combination of platforms and environments that are affected by
    // the command
    let affect_environment_and_platforms = affected_environments
        .into_iter()
        // Create an iterator over all environment and platform combinations
        .flat_map(|e| e.platforms().into_iter().map(move |p| (e.clone(), p)))
        // Filter out any platform that is not affected by the changes.
        .filter(|(_, platform)| {
            dependency_config.platform.is_empty() || dependency_config.platform.contains(platform)
        })
        .map(|(e, p)| (e.name().to_string(), p))
        .collect_vec();

    // Create an updated lock-file where the dependencies to be added are removed
    // from the lock-file.
    let unlocked_lock_file = unlock_packages(
        &project,
        &lock_file,
        conda_packages,
        pypi_packages,
        affect_environment_and_platforms
            .iter()
            .map(|(e, p)| (e.as_str(), *p))
            .collect(),
    );

    // Solve the updated project.
    let LockFileDerivedData {
        lock_file,
        package_cache,
        uv_context,
        updated_conda_prefixes,
        updated_pypi_prefixes,
        ..
    } = UpdateContext::builder(&project)
        .with_lock_file(unlocked_lock_file)
        .with_no_install(prefix_update_config.no_install())
        .finish()?
        .update()
        .await?;

    // Update the constraints of specs that didn't have a version constraint based
    // on the contents of the lock-file.
    let implicit_constraints = if !conda_specs_to_add_constraints_for.is_empty() {
        update_conda_specs_from_lock_file(
            &mut project,
            &lock_file,
            conda_specs_to_add_constraints_for,
            affect_environment_and_platforms,
            &feature_name,
            &dependency_config.platform,
        )?
    } else if !pypi_specs_to_add_constraints_for.is_empty() {
        update_pypi_specs_from_lock_file(
            &mut project,
            &lock_file,
            pypi_specs_to_add_constraints_for,
            affect_environment_and_platforms,
            &feature_name,
            &dependency_config.platform,
            args.editable,
        )?
    } else {
        HashMap::new()
    };

    // Write the lock-file and the project to disk
    project.save()?;

    // Reconstruct the lock-file derived data.
    let mut updated_lock_file = LockFileDerivedData {
        project: &project,
        lock_file,
        package_cache,
        updated_conda_prefixes,
        updated_pypi_prefixes,
        uv_context,
    };
    if !prefix_update_config.no_lockfile_update {
        updated_lock_file.write_to_disk()?;
    }

    // Install/update the default environment if:
    // - we are not skipping the installation,
    // - there is only the default environment,
    // - and the default environment is affected by the changes,
    if !prefix_update_config.no_install()
        && project.environments().len() == 1
        && default_environment_is_affected
    {
        updated_lock_file
            .prefix(&project.default_environment())
            .await?;
    }

    // Notify the user we succeeded.
    dependency_config.display_success("Added", implicit_constraints);

    Project::warn_on_discovered_from_env(project_config.manifest_path.as_deref());
    Ok(())
}

/// Update the pypi specs of newly added packages based on the contents of the
/// updated lock-file.
fn update_pypi_specs_from_lock_file(
    project: &mut Project,
    updated_lock_file: &LockFile,
    pypi_specs_to_add_constraints_for: IndexMap<PyPiPackageName, Requirement>,
    affect_environment_and_platforms: Vec<(String, Platform)>,
    feature_name: &FeatureName,
    platforms: &[Platform],
    editable: bool,
) -> miette::Result<HashMap<String, String>> {
    let mut implicit_constraints = HashMap::new();

    let pypi_records = affect_environment_and_platforms
        .into_iter()
        // Get all the conda and pypi records for the combination of environments and
        // platforms
        .filter_map(|(env, platform)| {
            let locked_env = updated_lock_file.environment(&env)?;
            locked_env.pypi_packages_for_platform(platform)
        })
        .flatten()
        .collect_vec();

    let pinning_strategy = project.config().pinning_strategy.unwrap_or_default();

    // Determine the versions of the packages in the lock-file
    for (name, req) in pypi_specs_to_add_constraints_for {
        let version_constraint = pinning_strategy.determine_version_constraint(
            pypi_records
                .iter()
                .filter_map(|(data, _)| {
                    if &data.name == name.as_normalized() {
                        Version::from_str(&data.version.to_string()).ok()
                    } else {
                        None
                    }
                })
                .collect_vec()
                .iter(),
        );

        let version_spec =
            version_constraint.and_then(|spec| VersionSpecifiers::from_str(&spec.to_string()).ok());
        if let Some(version_spec) = version_spec {
            implicit_constraints.insert(name.as_source().to_string(), version_spec.to_string());
            let req = Requirement {
                version_or_url: Some(VersionSpecifier(version_spec)),
                ..req
            };
            project.manifest.add_pypi_dependency(
                &req,
                platforms,
                feature_name,
                Some(editable),
                DependencyOverwriteBehavior::Overwrite,
            )?;
        }
    }

    Ok(implicit_constraints)
}

/// Update the conda specs of newly added packages based on the contents of the
/// updated lock-file.
fn update_conda_specs_from_lock_file(
    project: &mut Project,
    updated_lock_file: &LockFile,
    conda_specs_to_add_constraints_for: IndexMap<PackageName, (SpecType, MatchSpec)>,
    affect_environment_and_platforms: Vec<(String, Platform)>,
    feature_name: &FeatureName,
    platforms: &[Platform],
) -> miette::Result<HashMap<String, String>> {
    let mut implicit_constraints = HashMap::new();

    // Determine the conda records that were affected by the add.
    let conda_records = affect_environment_and_platforms
        .into_iter()
        // Get all the conda and pypi records for the combination of environments and
        // platforms
        .filter_map(|(env, platform)| {
            let locked_env = updated_lock_file.environment(&env)?;
            locked_env
                .conda_repodata_records_for_platform(platform)
                .ok()?
        })
        .flatten()
        .collect_vec();

    let pinning_strategy = project.config().pinning_strategy.unwrap_or_default();
    let channel_config = project.channel_config();
    for (name, (spec_type, spec)) in conda_specs_to_add_constraints_for {
        let version_constraint = pinning_strategy.determine_version_constraint(
            conda_records.iter().filter_map(|record| {
                if record.package_record.name == name {
                    Some(record.package_record.version.version())
                } else {
                    None
                }
            }),
        );

        if let Some(version_constraint) = version_constraint {
            implicit_constraints
                .insert(name.as_source().to_string(), version_constraint.to_string());
            let spec = MatchSpec {
                version: Some(version_constraint),
                ..spec
            };
            project.manifest.add_dependency(
                &spec,
                spec_type,
                platforms,
                feature_name,
                DependencyOverwriteBehavior::Overwrite,
                &channel_config,
            )?;
        }
    }

    Ok(implicit_constraints)
}

/// Constructs a new lock-file where some of the constraints have been removed.
fn unlock_packages(
    project: &Project,
    lock_file: &LockFile,
    conda_packages: HashSet<PackageName>,
    pypi_packages: HashSet<uv_normalize::PackageName>,
    affected_environments: HashSet<(&str, Platform)>,
) -> LockFile {
    filter_lock_file(project, lock_file, |env, platform, package| {
        if affected_environments.contains(&(env.name().as_str(), platform)) {
            match package {
                Package::Conda(package) => !conda_packages.contains(&package.package_record().name),
                Package::Pypi(package) => !pypi_packages.contains(&package.data().package.name),
            }
        } else {
            true
        }
    })
}
