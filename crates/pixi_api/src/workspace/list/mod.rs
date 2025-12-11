use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_core::{
    UpdateLockFileOptions, Workspace, environment::LockFileUsage, lock_file::UvResolutionContext,
};
use pixi_manifest::FeaturesExt;
use pixi_uv_conversions::{ConversionError, pypi_options_to_index_locations, to_uv_normalize};
use pypi_modifiers::pypi_tags::{get_pypi_tags, is_python_record};
use rattler_conda_types::Platform;
use rattler_lock::LockedPackageRef;
use uv_distribution::RegistryWheelIndex;
use uv_distribution_types::{
    ConfigSettings, ExtraBuildRequires, ExtraBuildVariables, PackageConfigSettings,
};

mod package;

use package::PackageExt;
pub use package::{Package, PackageKind};

pub async fn list(
    workspace: &Workspace,
    regex: Option<String>,
    platform: Option<Platform>,
    environment: Option<String>,
    explicit: bool,
    no_install: bool,
    lock_file_usage: LockFileUsage,
) -> miette::Result<Vec<Package>> {
    let environment = workspace.environment_from_name_or_env_var(environment)?;

    let lock_file = workspace
        .update_lock_file(UpdateLockFileOptions {
            lock_file_usage,
            no_install,
            max_concurrent_solves: workspace.config().max_concurrent_solves(),
        })
        .await?
        .0
        .into_lock_file();

    // Load the platform
    let platform = platform.unwrap_or_else(|| environment.best_platform());

    // Get all the packages in the environment.
    let locked_deps = lock_file
        .environment(environment.name().as_str())
        .and_then(|env| env.packages(platform).map(Vec::from_iter))
        .unwrap_or_default();

    let locked_deps_ext = locked_deps
        .into_iter()
        .map(|p| match p {
            LockedPackageRef::Pypi(pypi_data, _) => {
                let name = to_uv_normalize(&pypi_data.name)?;
                Ok(PackageExt::PyPI(pypi_data.clone(), name))
            }
            LockedPackageRef::Conda(c) => Ok(PackageExt::Conda(c.clone())),
        })
        .collect::<Result<Vec<_>, ConversionError>>()
        .into_diagnostic()?;

    // Get the python record from the lock file
    let mut conda_records = locked_deps_ext.iter().filter_map(|d| d.as_conda());

    // Construct the registry index if we have a python record
    let python_record = conda_records.find(|r| is_python_record(r));
    let tags;
    let uv_context;
    let index_locations;
    let config_settings = ConfigSettings::default();
    let package_config_settings = PackageConfigSettings::default();
    let extra_build_requires = ExtraBuildRequires::default();
    let extra_build_variables = ExtraBuildVariables::default();

    let mut registry_index = if let Some(python_record) = python_record {
        if environment.has_pypi_dependencies() {
            uv_context = UvResolutionContext::from_config(workspace.config())?;
            index_locations =
                pypi_options_to_index_locations(&environment.pypi_options(), workspace.root())
                    .into_diagnostic()?;
            tags = get_pypi_tags(
                platform,
                &environment.system_requirements(),
                python_record.record(),
            )?;
            Some(RegistryWheelIndex::new(
                &uv_context.cache,
                &tags,
                &index_locations,
                &uv_types::HashStrategy::None,
                &config_settings,
                &package_config_settings,
                &extra_build_requires,
                &extra_build_variables,
            ))
        } else {
            None
        }
    } else {
        None
    };

    // Get the explicit project dependencies
    let mut project_dependency_names = environment
        .combined_dependencies(Some(platform))
        .names()
        .map(|p| p.as_source().to_string())
        .collect_vec();
    project_dependency_names.extend(
        environment
            .pypi_dependencies(Some(platform))
            .into_iter()
            .map(|(name, _)| name.as_normalized().as_dist_info_name().into_owned()),
    );

    let mut packages_to_output = locked_deps_ext
        .iter()
        .map(|p| Package::new(p, &project_dependency_names, registry_index.as_mut()))
        .collect::<Vec<Package>>();

    // Filter packages by regex if needed
    if let Some(regex) = regex {
        let regex = regex::Regex::new(&regex).map_err(|_| miette::miette!("Invalid regex"))?;
        packages_to_output = packages_to_output
            .into_iter()
            .filter(|p| regex.is_match(&p.name))
            .collect::<Vec<_>>();
    }

    // Filter packages by explicit if needed
    if explicit {
        packages_to_output = packages_to_output
            .into_iter()
            .filter(|p| p.is_explicit)
            .collect::<Vec<_>>();
    }

    Ok(packages_to_output)
}
