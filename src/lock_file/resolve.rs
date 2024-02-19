//! This module contains code to resolve python package from PyPi or Conda packages.
//!
//! See [`resolve_pypi`] and [`resolve_conda`] for more information.

use crate::consts::PROJECT_MANIFEST;
use crate::pypi_marker_env::determine_marker_environment;
use crate::pypi_tags::{get_pypi_tags, is_python_record};
use crate::{
    lock_file::{LockedCondaPackages, LockedPypiPackages, PypiRecord},
    project::manifest::{PyPiRequirement, SystemRequirements},
};
use distribution_types::{BuiltDist, Dist, FileLocation, IndexLocations, Resolution};
use indexmap::IndexMap;
use indicatif::ProgressBar;
use miette::{Context, IntoDiagnostic};
use platform_host::Platform;
use rattler_conda_types::{GenericVirtualPackage, MatchSpec, RepoDataRecord};
use rattler_digest::{parse_digest_from_hex, Md5, Sha256};
use rattler_lock::{PackageHashes, PypiPackageData, PypiPackageEnvironmentData};
use rattler_solve::{resolvo, SolverImpl};
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use url::Url;
use uv_cache::Cache;
use uv_client::{Connectivity, FlatIndex, FlatIndexClient, RegistryClient, RegistryClientBuilder};
use uv_dispatch::BuildDispatch;
use uv_interpreter::Interpreter;
use uv_normalize::PackageName;
use uv_resolver::{InMemoryIndex, Manifest, Options, Resolver};
use uv_traits::{InFlight, NoBinary, NoBuild, SetupPyStrategy};

struct PypiSolveContext {
    interpreter: Interpreter,
    registry_client: Arc<RegistryClient>,
    index_locations: Arc<IndexLocations>,

}

/// This function takes as input a set of dependencies and system requirements and returns a set of
/// locked packages.
#[allow(clippy::too_many_arguments)]
pub async fn resolve_pypi(
    // package_db: Arc<PackageDb>,
    dependencies: IndexMap<PackageName, Vec<PyPiRequirement>>,
    system_requirements: SystemRequirements,
    locked_conda_records: &[RepoDataRecord],
    _locked_pypi_records: &[PypiRecord],
    platform: rattler_conda_types::Platform,
    pb: &ProgressBar,
    python_location: &Path,
    venv_root: &Path,
) -> miette::Result<LockedPypiPackages> {
    // Solve python packages
    pb.set_message("resolving pypi dependencies");

    let requirements = dependencies
        .iter()
        .flat_map(|(name, req)| req.iter().map(move |req| (name, req)))
        .map(|(name, req)| req.as_pep508(name))
        .collect::<Vec<pep508_rs::Requirement>>();

    // Determine the python interpreter that is installed as part of the conda packages.
    let python_record = locked_conda_records
        .iter()
        .find(|r| is_python_record(r))
        .ok_or_else(|| miette::miette!("could not resolve pypi dependencies because no python interpreter is added to the dependencies of the project.\nMake sure to add a python interpreter to the [dependencies] section of the {PROJECT_MANIFEST}, or run:\n\n\tpixi add python"))?;

    // Construct the marker environment for the target platform
    let marker_environment = determine_marker_environment(platform, python_record.as_ref())?;

    // Determine the tags
    let tags = get_pypi_tags(platform, system_requirements, python_record.as_ref())?;

    // Construct a fake interpreter from the conda environment.
    // TODO: Should we look into using the actual interpreter here?
    let interpreter = Interpreter::artificial(
        Platform::current().expect("unsupported platform"),
        marker_environment.clone(),
        venv_root.to_path_buf(),
        venv_root.to_path_buf(),
        python_location.to_path_buf(),
        Path::new("invalid").to_path_buf(),
    );

    // Construct a cache
    // TODO: Figure out the right location
    let cache = Cache::temp()
        .into_diagnostic()
        .context("failed to create cache")?;

    // Define where to get packages from
    let index_locations = Arc::new(IndexLocations::default());

    // Construct a registry client
    let registry_client = Arc::new(
        RegistryClientBuilder::new(cache.clone())
            .index_urls(index_locations.index_urls())
            .connectivity(Connectivity::Online)
            .build(),
    );

    // Resolve the flat indexes from `--find-links`.
    let flat_index = {
        let client = FlatIndexClient::new(&registry_client, &cache);
        let entries = client
            .fetch(index_locations.flat_index())
            .await
            .into_diagnostic()?;
        FlatIndex::from_entries(entries, &tags)
    };

    // Create a shared in-memory index.
    let index = InMemoryIndex::default();

    // Track in-flight downloads, builds, etc., across resolutions.
    let in_flight = InFlight::default();

    let options = Options::default();

    let build_dispatch = BuildDispatch::new(
        &registry_client,
        &cache,
        &interpreter,
        &index_locations,
        &flat_index,
        &index,
        &in_flight,
        interpreter.sys_executable().to_path_buf(),
        SetupPyStrategy::Pep517,
        &NoBuild::None,
        &NoBinary::None,
    )
        .with_options(options.clone());

    let resolution = Resolver::new(
        Manifest::simple(requirements),
        Options::default(),
        &marker_environment,
        &interpreter,
        &tags,
        &registry_client,
        &flat_index,
        &index,
        &build_dispatch,
    )
        .resolve()
        .await
        .into_diagnostic()
        .context("failed to resolve pypi dependencies")?;
    let resolution = Resolution::from(resolution);

    // Clear message
    pb.set_message("");

    let mut locked_packages = LockedPypiPackages::with_capacity(resolution.len());
    for dist in resolution.into_distributions() {
        let pypi_package_data = match dist {
            Dist::Built(dist) => {
                let (url, hash) = match &dist {
                    BuiltDist::Registry(dist) => {
                        let url = match &dist.file.url {
                            FileLocation::AbsoluteUrl(url) => {
                                Url::from_str(url).expect("invalid absolute url")
                            }
                            FileLocation::Path(path) => {
                                Url::from_file_path(path).expect("invalid path")
                            }
                            _ => todo!("unsupported URL"),
                        };

                        let hash = match (&dist.file.hashes.sha256, &dist.file.hashes.md5) {
                            (Some(sha256), None) => Some(PackageHashes::Sha256(
                                parse_digest_from_hex::<Sha256>(sha256).expect("invalid sha256"),
                            )),
                            (None, Some(md5)) => Some(PackageHashes::Md5(
                                parse_digest_from_hex::<Md5>(md5).expect("invalid md5"),
                            )),
                            (Some(sha256), Some(md5)) => Some(PackageHashes::Md5Sha256(
                                parse_digest_from_hex::<Md5>(md5).expect("invalid md5"),
                                parse_digest_from_hex::<Sha256>(sha256).expect("invalid sha256"),
                            )),
                            (None, None) => None,
                        };

                        (url, hash)
                    }
                    BuiltDist::DirectUrl(dist) => (dist.url.to_url(), None),
                    BuiltDist::Path(dist) => (dist.url.to_url(), None),
                };

                let metadata = registry_client
                    .wheel_metadata(&dist)
                    .await
                    .expect("failed to get wheel metadata");
                PypiPackageData {
                    name: metadata.name,
                    version: metadata.version,
                    requires_dist: metadata.requires_dist,
                    requires_python: metadata.requires_python,
                    url,
                    hash,
                }
            }
            Dist::Source(_) => {
                todo!("source dists not yet supported");
            }
        };

        // TODO: Store extras in the lock-file
        locked_packages.push((pypi_package_data, PypiPackageEnvironmentData::default()));
    }

    Ok(locked_packages)
}

/// Solves the conda package environment for the given input. This function is async because it
/// spawns a background task for the solver. Since solving is a CPU intensive task we do not want to
/// block the main task.
pub async fn resolve_conda(
    specs: Vec<MatchSpec>,
    virtual_packages: Vec<GenericVirtualPackage>,
    locked_packages: Vec<RepoDataRecord>,
    available_packages: Vec<Vec<RepoDataRecord>>,
) -> miette::Result<LockedCondaPackages> {
    tokio::task::spawn_blocking(move || {
        // Construct a solver task that we can start solving.
        let task = rattler_solve::SolverTask {
            specs,
            available_packages: &available_packages,
            locked_packages,
            pinned_packages: vec![],
            virtual_packages,
            timeout: None,
        };

        // Solve the task
        resolvo::Solver.solve(task).into_diagnostic()
    })
        .await
        .unwrap_or_else(|e| match e.try_into_panic() {
            Ok(e) => std::panic::resume_unwind(e),
            Err(_err) => Err(miette::miette!("cancelled")),
        })
}
