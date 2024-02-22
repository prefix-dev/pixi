//! This module contains code to resolve python package from PyPi or Conda packages.
//!
//! See [`resolve_pypi`] and [`resolve_conda`] for more information.

use crate::config::get_cache_dir;
use crate::consts::PROJECT_MANIFEST;
use std::collections::HashMap;
use std::future::Future;

use crate::lock_file::{package_identifier, PypiPackageIdentifier};
use crate::pypi_marker_env::determine_marker_environment;
use crate::pypi_tags::{get_pypi_tags, is_python_record};
use crate::{
    lock_file::{LockedCondaPackages, LockedPypiPackages, PypiRecord},
    project::manifest::{PyPiRequirement, SystemRequirements},
    Project,
};
use distribution_types::{
    BuiltDist, Dist, FileLocation, IndexLocations, Name, Resolution, SourceDist, VersionOrUrl,
};
use indexmap::IndexMap;
use indicatif::ProgressBar;
use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use pep440_rs::{Operator, Version, VersionPattern, VersionSpecifier};
use pep508_rs::Requirement;
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
use uv_distribution::{DistributionDatabase, Reporter};
use uv_interpreter::Interpreter;
use uv_normalize::PackageName;
use uv_resolver::{
    DefaultResolverProvider, InMemoryIndex, Manifest, Options, Resolver, ResolverProvider,
};
use uv_traits::{BuildContext, InFlight, NoBinary, NoBuild, SetupPyStrategy};

/// Objects that are needed for resolutions which can be shared between different resolutions.
#[derive(Clone)]
pub struct UvResolutionContext {
    pub cache: Cache,
    pub registry_client: Arc<RegistryClient>,
    pub in_flight: Arc<InFlight>,
    pub index_locations: Arc<IndexLocations>,
    pub in_memory_index: Arc<InMemoryIndex>,
}

impl UvResolutionContext {
    pub fn from_project(project: &Project) -> miette::Result<Self> {
        let cache = Cache::from_path(get_cache_dir().expect("missing caching directory"))
            .into_diagnostic()
            .context("failed to create uv cache")?;
        let registry_client = Arc::new(
            RegistryClientBuilder::new(cache.clone())
                .client(project.client().clone())
                .connectivity(Connectivity::Online)
                .build(),
        );
        let in_flight = Arc::new(InFlight::default());
        let index_locations = Arc::new(project.pypi_index_locations());
        let in_memory_index = Arc::new(InMemoryIndex::default());
        Ok(Self {
            cache,
            registry_client,
            in_flight,
            index_locations,
            in_memory_index,
        })
    }
}

/// This function takes as input a set of dependencies and system requirements and returns a set of
/// locked packages.

fn parse_hashes_from_hex(sha256: &Option<String>, md5: &Option<String>) -> Option<PackageHashes> {
    match (sha256, md5) {
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
    }
}

struct ResolveReporter(ProgressBar);

impl uv_resolver::ResolverReporter for ResolveReporter {
    fn on_progress(&self, name: &PackageName, version: VersionOrUrl) {
        self.0.set_message(format!("resolving {}{}", name, version));
    }

    fn on_complete(&self) {}

    fn on_build_start(&self, _dist: &SourceDist) -> usize {
        0
    }

    fn on_build_complete(&self, _dist: &SourceDist, _id: usize) {}

    fn on_checkout_start(&self, _url: &Url, _rev: &str) -> usize {
        0
    }

    fn on_checkout_complete(&self, _url: &Url, _rev: &str, _index: usize) {}
}

// struct CondaResolverProvider<'a, Context: BuildContext + Send + Sync> {
//     fallback: DefaultResolverProvider<'a, Context>,
//     conda_python_identifiers: Vec<PypiPackageIdentifier>,
// }
//
// type PackageVersionsResult = Result<VersionsResponse, uv_client::Error>;
// type WheelMetadataResult = Result<(Metadata21, Option<Url>), uv_distribution::Error>;
//
// impl<'a, Context: BuildContext + Send + Sync> ResolverProvider for CondaResolverProvider<'a, Context> {
//     fn get_package_versions<'io>(&'io self, package_name: &'io PackageName) -> impl Future<Output=uv_resolver::resolver::provider::PackageVersionsResult> + Send + 'io {
//         todo!()
//     }
//
//     fn get_or_build_wheel_metadata<'io>(&'io self, dist: &'io Dist) -> impl Future<Output=uv_resolver::resolver::provider::WheelMetadataResult> + Send + 'io {
//         self.fallback.get_or_build_wheel_metadata(dist)
//     }
//
//     fn index_locations(&self) -> &IndexLocations {
//         todo!()
//     }
//
//     fn with_reporter(self, reporter: impl Reporter + 'static) -> Self {
//         todo!()
//     }
// }

fn single_version_requirement(name: PackageName, version: Version) -> Requirement {
    Requirement {
        name,
        version_or_url: Some(pep508_rs::VersionOrUrl::VersionSpecifier(
            [
                VersionSpecifier::new(Operator::Equal, VersionPattern::verbatim(version))
                    .expect("this should always work"),
            ]
            .into_iter()
            .collect(),
        )),
        extras: Vec::default(),
        marker: None,
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn resolve_pypi(
    context: UvResolutionContext,
    dependencies: IndexMap<PackageName, Vec<PyPiRequirement>>,
    system_requirements: SystemRequirements,
    locked_conda_records: &[RepoDataRecord],
    locked_pypi_records: &[PypiRecord],
    platform: rattler_conda_types::Platform,
    pb: &ProgressBar,
    python_location: &Path,
    _venv_root: &Path,
) -> miette::Result<LockedPypiPackages> {
    // Solve python packages
    pb.set_message("resolving pypi dependencies");

    // Determine which pypi packages are already installed as conda package.
    // Determine the python packages that are installed by the conda packages
    let conda_python_packages =
        package_identifier::PypiPackageIdentifier::from_records(locked_conda_records)
            .into_diagnostic()
            .context("failed to extract python packages from conda metadata")?
            .into_iter()
            .map(|p| (p.name.clone(), p))
            .collect::<HashMap<_, _>>();

    if !conda_python_packages.is_empty() {
        tracing::info!(
            "the following python packages are assumed to be installed by conda: {conda_python_packages}",
            conda_python_packages =
                conda_python_packages
                    .values()
                    .format_with(", ", |p, f| f(&format_args!(
                        "{name} {version}",
                        name = &p.name,
                        version = &p.version
                    )))
        );
    } else {
        tracing::info!("there are no python packages installed by conda");
    }

    // Get the Pypi requirements
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

    // Determine the tags for this particular solve.
    let tags = get_pypi_tags(platform, &system_requirements, python_record.as_ref())?;

    // Construct a fake interpreter from the conda environment.
    // TODO: Should we look into using the actual interpreter here?
    let platform = Platform::current().expect("unsupported platform");
    let interpreter =
        Interpreter::query(python_location, &platform, &context.cache).into_diagnostic()?;

    // Resolve the flat indexes from `--find-links`.
    let flat_index = {
        let client = FlatIndexClient::new(&context.registry_client, &context.cache);
        let entries = client
            .fetch(context.index_locations.flat_index())
            .await
            .into_diagnostic()?;
        FlatIndex::from_entries(entries, &tags)
    };

    // Create a shared in-memory index.
    let options = Options::default();
    let build_dispatch = BuildDispatch::new(
        &context.registry_client,
        &context.cache,
        &interpreter,
        &context.index_locations,
        &flat_index,
        &context.in_memory_index,
        &context.in_flight,
        interpreter.sys_executable().to_path_buf(),
        SetupPyStrategy::default(),
        &NoBuild::None,
        &NoBinary::None,
    )
    .with_options(options.clone());

    let constraints = conda_python_packages
        .values()
        .map(|p| single_version_requirement(p.name.clone(), p.version.clone()))
        .collect();

    let preferences = locked_pypi_records
        .iter()
        .map(|p| single_version_requirement(p.0.name.clone(), p.0.version.clone()))
        .collect();

    let resolution = Resolver::new(
        Manifest::new(
            requirements,
            constraints,
            Vec::new(),
            preferences,
            None,
            Vec::new(),
        ),
        options.clone(),
        &marker_environment,
        &interpreter,
        &tags,
        &context.registry_client,
        &flat_index,
        &context.in_memory_index,
        &build_dispatch,
    )
    .into_diagnostic()
    .context("failed to resolve pypi dependencies")?
    .with_reporter(ResolveReporter(pb.clone()))
    .resolve()
    .await
    .into_diagnostic()
    .context("failed to resolve pypi dependencies")?;

    let resolution = Resolution::from(resolution);

    // Clear message
    pb.set_message("");

    let database = DistributionDatabase::new(
        &context.cache,
        &tags,
        &context.registry_client,
        &build_dispatch,
    );
    let mut locked_packages = LockedPypiPackages::with_capacity(resolution.len());
    for dist in resolution.into_distributions() {
        // If this refers to a conda package we can skip it
        if conda_python_packages.contains_key(dist.name()) {
            continue;
        }

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

                        let hash =
                            parse_hashes_from_hex(&dist.file.hashes.sha256, &dist.file.hashes.md5);

                        (url, hash)
                    }
                    BuiltDist::DirectUrl(dist) => (dist.url.to_url(), None),
                    BuiltDist::Path(dist) => (dist.url.to_url(), None),
                };

                let metadata = context
                    .registry_client
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
            Dist::Source(source) => {
                let hash = source
                    .file()
                    .and_then(|file| parse_hashes_from_hex(&file.hashes.sha256, &file.hashes.md5));

                let (metadata, url) = database
                    .get_or_build_wheel_metadata(&Dist::Source(source.clone()))
                    .await
                    .into_diagnostic()?;

                // Use the precise url if we got it back
                // otherwise try to construct it from the source
                let url = if let Some(url) = url {
                    url
                } else {
                    match source {
                        SourceDist::Registry(reg) => match &reg.file.url {
                            FileLocation::AbsoluteUrl(url) => {
                                Url::from_str(url).expect("invalid absolute url")
                            }
                            FileLocation::Path(path) => {
                                Url::from_file_path(path).expect("invalid path")
                            }
                            _ => todo!("unsupported URL"),
                        },
                        SourceDist::DirectUrl(direct) => direct.url.to_url(),
                        SourceDist::Git(git) => git.url.to_url(),
                        SourceDist::Path(path) => path.url.to_url(),
                    }
                };

                PypiPackageData {
                    name: metadata.name,
                    version: metadata.version,
                    requires_dist: metadata.requires_dist,
                    requires_python: metadata.requires_python,
                    url,
                    hash,
                }
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
