use crate::consts::PROJECT_MANIFEST;
use crate::lock_file::{package_identifier, pypi_name_mapping};
use crate::project::manifest::{PyPiRequirement, SystemRequirements};
use crate::pypi_marker_env::determine_marker_environment;
use crate::pypi_tags::{is_python_record, project_platform_tags};
use indexmap::IndexMap;
use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{Platform, RepoDataRecord};
use rip::index::PackageDb;
use rip::python_env::PythonLocation;
use rip::resolve::solve_options::{ResolveOptions, SDistResolution};
use rip::resolve::{resolve, PinnedPackage};
use rip::types::PackageName;
use rip::wheel_builder::WheelBuilder;
use std::path::Path;
use std::sync::Arc;
use std::vec;

/// Resolve python packages for the specified project.
pub async fn resolve_dependencies<'db>(
    package_db: Arc<PackageDb>,
    dependencies: IndexMap<PackageName, Vec<PyPiRequirement>>,
    system_requirements: SystemRequirements,
    platform: Platform,
    conda_packages: &[RepoDataRecord],
    python_location: Option<&Path>,
    sdist_resolution: SDistResolution,
) -> miette::Result<Vec<PinnedPackage>> {
    if dependencies.is_empty() {
        return Ok(vec![]);
    }

    // Determine the python packages that are installed by the conda packages
    let conda_python_packages =
        package_identifier::PypiPackageIdentifier::from_records(conda_packages)
            .into_diagnostic()
            .context("failed to extract python packages from conda metadata")?
            .into_iter()
            .map(PinnedPackage::from)
            .collect_vec();

    if !conda_python_packages.is_empty() {
        tracing::info!(
            "the following python packages are assumed to be installed by conda: {conda_python_packages}",
            conda_python_packages =
                conda_python_packages
                    .iter()
                    .format_with(", ", |p, f| f(&format_args!(
                        "{name} {version}",
                        name = &p.name,
                        version = &p.version
                    )))
        );
    } else {
        tracing::info!("there are no python packages installed by conda");
    }

    // Determine the python interpreter that is installed as part of the conda packages.
    let python_record = conda_packages
        .iter()
        .find(|r| is_python_record(r))
        .ok_or_else(|| miette::miette!("could not resolve pypi dependencies because no python interpreter is added to the dependencies of the project.\nMake sure to add a python interpreter to the [dependencies] section of the {PROJECT_MANIFEST}, or run:\n\n\tpixi add python"))?;

    // Determine the environment markers
    let marker_environment = determine_marker_environment(platform, python_record.as_ref())?;

    // Determine the compatible tags
    let compatible_tags =
        project_platform_tags(platform, &system_requirements, python_record.as_ref());

    let requirements = dependencies
        .iter()
        .flat_map(|(name, req)| req.iter().map(move |req| (name, req)))
        .map(|(name, req)| req.as_pep508(name))
        .collect::<Vec<pep508_rs::Requirement>>();

    // If we only have a system python
    // we cannot resolve correctly, because we might not be able to
    // build source dists correctly. Let's skip them for now
    let (sdist_resolution, python_location) = match python_location {
        Some(path) => (sdist_resolution, PythonLocation::Custom(path.to_path_buf())),
        // Use the resolution we have been passed in
        None => (sdist_resolution, PythonLocation::System),
    };

    // Resolve the PyPi dependencies
    let marker_environment = Arc::new(marker_environment);
    let compatible_tags = Arc::new(compatible_tags);
    let resolve_options = ResolveOptions {
        sdist_resolution,
        python_location,
        locked_packages: conda_python_packages
            .into_iter()
            .map(|p| (p.name.clone(), p))
            .collect(),
        ..Default::default()
    };
    let mut result = resolve(
        package_db.clone(),
        &requirements,
        marker_environment.clone(),
        Some(compatible_tags.clone()),
        WheelBuilder::new(
            package_db,
            marker_environment.clone(),
            Some(compatible_tags.clone()),
            resolve_options.clone(),
        )
        .expect("failed to create wheel builder"),
        resolve_options,
    )
    .await
    .wrap_err("failed to resolve `pypi-dependencies`, due to underlying error")?;

    // Remove any conda package from the result
    result.retain(|p| !p.artifacts.is_empty());

    Ok(result)
}

/// Amend the records with pypi purls if they are not present yet.
pub async fn amend_pypi_purls(conda_packages: &mut [RepoDataRecord]) -> miette::Result<()> {
    let conda_forge_mapping = pypi_name_mapping::conda_pypi_name_mapping().await?;
    for record in conda_packages.iter_mut() {
        pypi_name_mapping::amend_pypi_purls(record, conda_forge_mapping)?;
    }
    Ok(())
}
