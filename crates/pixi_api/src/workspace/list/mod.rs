use std::collections::HashMap;

use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_core::{
    UpdateLockFileOptions, Workspace, environment::LockFileUsage, lock_file::UvResolutionContext,
};
use pixi_manifest::FeaturesExt;
use pixi_uv_conversions::{ConversionError, pypi_options_to_index_locations, to_uv_normalize};
use pypi_modifiers::pypi_tags::{get_pypi_tags, is_python_package_name};
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
    let locked_platform = lock_file.platform(platform.as_str());
    let locked_environment = lock_file.environment(environment.name().as_str());

    // Get all the packages in the environment.
    let locked_deps = match (locked_platform, locked_environment) {
        (Some(locked_platform), Some(locked_environment)) => locked_environment
            .packages(locked_platform)
            .map(Vec::from_iter)
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    let locked_deps_ext = locked_deps
        .into_iter()
        .map(|p| match p {
            LockedPackageRef::Pypi(pypi_data) => {
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
    let python_record = conda_records.find(|r| is_python_package_name(r.name()));
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
            let record = python_record
                .record()
                .expect("python record should have full metadata");
            tags = get_pypi_tags(platform, &environment.system_requirements(), record)?;
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

    // Get the explicit project dependencies with their requested specs
    let mut requested_specs: HashMap<String, String> = environment
        .combined_dependencies(Some(platform))
        .iter()
        .map(|(name, specs)| {
            let spec_str = specs.iter().map(|s| s.to_string()).join(", ");
            (name.as_source().to_string(), spec_str)
        })
        .collect();
    requested_specs.extend(
        environment
            .pypi_dependencies(Some(platform))
            .into_iter()
            .map(|(name, reqs)| {
                let spec = reqs
                    .first()
                    .map(|r| r.to_string())
                    .unwrap_or_else(|| "*".to_string());
                (name.as_normalized().as_dist_info_name().into_owned(), spec)
            }),
    );

    let mut packages_to_output = locked_deps_ext
        .iter()
        .map(|p| Package::new(p, &requested_specs, registry_index.as_mut()))
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
