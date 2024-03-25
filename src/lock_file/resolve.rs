//! This module contains code to resolve python package from PyPi or Conda packages.
//!
//! See [`resolve_pypi`] and [`resolve_conda`] for more information.

use crate::config::get_cache_dir;
use crate::consts::PROJECT_MANIFEST;
use crate::project::manifest::python::RequirementOrEditable;
use crate::uv_reporter::{UvReporter, UvReporterOptions};
use std::collections::{BTreeMap, HashMap};
use std::future::{ready, Future};
use std::iter::once;

use crate::lock_file::{package_identifier, PypiPackageIdentifier};
use crate::pypi_marker_env::determine_marker_environment;
use crate::pypi_tags::{get_pypi_tags, is_python_record};
use crate::{
    lock_file::{LockedCondaPackages, LockedPypiPackages},
    project::manifest::{PyPiRequirement, SystemRequirements},
    Project,
};

use distribution_types::{
    BuiltDist, DirectUrlSourceDist, Dist, IndexLocations, Name, PrioritizedDist, Resolution,
    SourceDist,
};
use distribution_types::{FileLocation, SourceDistCompatibility};
use futures::FutureExt;
use indexmap::IndexMap;
use indicatif::ProgressBar;
use itertools::{Either, Itertools};
use miette::{Context, IntoDiagnostic};
use pep508_rs::{Requirement, VerbatimUrl};
use pypi_types::Metadata23;
use rattler_conda_types::{GenericVirtualPackage, MatchSpec, RepoDataRecord};
use rattler_digest::{parse_digest_from_hex, Md5, Sha256};
use rattler_lock::{PackageHashes, PypiPackageData, PypiPackageEnvironmentData, UrlOrPath};
use rattler_solve::{resolvo, SolverImpl};
use std::path::{Path, PathBuf};
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
    AllowedYanks, DefaultResolverProvider, DistFinder, InMemoryIndex, Manifest, Options,
    PythonRequirement, Resolver, ResolverProvider, VersionMap, VersionsResponse,
};
use uv_traits::{BuildContext, ConfigSettings, InFlight, NoBinary, NoBuild, SetupPyStrategy};

/// Objects that are needed for resolutions which can be shared between different resolutions.
#[derive(Clone)]
pub struct UvResolutionContext {
    pub cache: Cache,
    pub registry_client: Arc<RegistryClient>,
    pub in_flight: Arc<InFlight>,
    pub index_locations: Arc<IndexLocations>,
    pub no_build: NoBuild,
    pub no_binary: NoBinary,
}

impl UvResolutionContext {
    pub fn from_project(project: &Project) -> miette::Result<Self> {
        let cache = Cache::from_path(
            get_cache_dir()
                .expect("missing caching directory")
                .join("uv-cache"),
        )
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
        Ok(Self {
            cache,
            registry_client,
            in_flight,
            index_locations,
            no_build: NoBuild::None,
            no_binary: NoBinary::None,
        })
    }
}

/// This function takes as input a set of dependencies and system requirements and returns a set of
/// locked packages.

fn parse_hashes_from_hex(
    sha256: &Option<Box<str>>,
    md5: &Option<Box<str>>,
) -> Option<PackageHashes> {
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

struct CondaResolverProvider<'a, Context: BuildContext + Send + Sync> {
    fallback: DefaultResolverProvider<'a, Context>,
    conda_python_identifiers: &'a HashMap<PackageName, (RepoDataRecord, PypiPackageIdentifier)>,
}

impl<'a, Context: BuildContext + Send + Sync> ResolverProvider
    for CondaResolverProvider<'a, Context>
{
    fn get_package_versions<'io>(
        &'io self,
        package_name: &'io PackageName,
    ) -> impl Future<Output = uv_resolver::PackageVersionsResult> + Send + 'io {
        if let Some((repodata_record, identifier)) = self.conda_python_identifiers.get(package_name)
        {
            // If we encounter a package that was installed by conda we simply return a single
            // available version in the form of a source distribution with the URL of the
            // conda package.
            //
            // Obviously this is not a valid source distribution but it easies debugging.
            let dist = Dist::Source(SourceDist::DirectUrl(DirectUrlSourceDist {
                name: identifier.name.as_normalized().clone(),
                url: VerbatimUrl::unknown(repodata_record.url.clone()),
            }));

            let prioritized_dist =
                PrioritizedDist::from_source(dist, None, SourceDistCompatibility::Compatible);

            return ready(Ok(VersionsResponse::Found(VersionMap::from(
                BTreeMap::from_iter([(identifier.version.clone(), prioritized_dist)]),
            ))))
            .right_future();
        }

        // Otherwise use the default implementation
        self.fallback
            .get_package_versions(package_name)
            .left_future()
    }

    fn get_or_build_wheel_metadata<'io>(
        &'io self,
        dist: &'io Dist,
    ) -> impl Future<Output = uv_resolver::WheelMetadataResult> + Send + 'io {
        if let Dist::Source(SourceDist::DirectUrl(DirectUrlSourceDist { url, name })) = dist {
            if let Some((_, iden)) = self.conda_python_identifiers.get(name) {
                // If this is a Source dist and the package is actually installed by conda we
                // create fake metadata with no dependencies. We assume that all conda installed
                // packages are properly installed including its dependencies.
                return ready(Ok((
                    Metadata23 {
                        metadata_version: "1.0".to_string(),
                        name: name.clone(),
                        version: iden.version.clone(),
                        requires_dist: vec![],
                        requires_python: None,
                        // TODO: This field is not actually properly used.
                        provides_extras: iden.extras.iter().cloned().collect(),
                    },
                    Some(url.to_url()),
                )))
                .left_future();
            }
        }

        // Otherwise just call the default implementation
        self.fallback
            .get_or_build_wheel_metadata(dist)
            .right_future()
    }

    fn index_locations(&self) -> &IndexLocations {
        self.fallback.index_locations()
    }

    fn with_reporter(self, reporter: impl Reporter + 'static) -> Self {
        Self {
            fallback: self.fallback.with_reporter(reporter),
            ..self
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn resolve_pypi(
    context: UvResolutionContext,
    dependencies: IndexMap<PackageName, Vec<PyPiRequirement>>,
    system_requirements: SystemRequirements,
    locked_conda_records: &[RepoDataRecord],
    platform: rattler_conda_types::Platform,
    pb: &ProgressBar,
    python_location: &Path,
    env_variables: &HashMap<String, String>,
    project_root: &Path,
) -> miette::Result<LockedPypiPackages> {
    // Solve python packages
    pb.set_message("resolving pypi dependencies");

    // Determine which pypi packages are already installed as conda package.
    let conda_python_packages = locked_conda_records
        .iter()
        .flat_map(|record| {
            package_identifier::PypiPackageIdentifier::from_record(record).map_or_else(
                |err| Either::Right(once(Err(err))),
                |identifiers| {
                    Either::Left(identifiers.into_iter().map(|i| Ok((record.clone(), i))))
                },
            )
        })
        .map_ok(|(record, p)| (p.name.as_normalized().clone(), (record.clone(), p)))
        .collect::<Result<HashMap<_, _>, _>>()
        .into_diagnostic()
        .context("failed to extract python packages from conda metadata")?;

    if !conda_python_packages.is_empty() {
        tracing::info!(
            "the following python packages are assumed to be installed by conda: {conda_python_packages}",
            conda_python_packages =
                conda_python_packages
                    .values()
                    .format_with(", ", |(_, p), f| f(&format_args!(
                        "{name} {version}",
                        name = &p.name.as_source(),
                        version = &p.version
                    )))
        );
    } else {
        tracing::info!("there are no python packages installed by conda");
    }

    // Get the Pypi requirements
    // partion the requirements into editable and non-editable requirements
    let (editables, requirements): (Vec<_>, Vec<_>) = dependencies
        .iter()
        .flat_map(|(name, req)| req.iter().map(move |req| (name, req)))
        .map(|(name, req)| {
            req.as_pep508(name, project_root)
                .into_diagnostic()
                .wrap_err(format!(
                    "error while converting {} to pep508 requirement",
                    name
                ))
        })
        .collect::<miette::Result<Vec<_>>>()?
        .into_iter()
        .partition(|req| matches!(req, RequirementOrEditable::Editable(_)));

    let _editables = editables
        .into_iter()
        .map(|req| {
            req.into_editable()
                .expect("wrong partitioning of editable and non-editable requirements")
        })
        .collect::<Vec<_>>();

    let requirements = requirements
        .into_iter()
        .map(|req| {
            req.into_requirement()
                .expect("wrong partitioning of editable and non-editable requirements")
        })
        .collect::<Vec<_>>();

    // Determine the python interpreter that is installed as part of the conda packages.
    let python_record = locked_conda_records
        .iter()
        .find(|r| is_python_record(r))
        .ok_or_else(|| miette::miette!("could not resolve pypi dependencies because no python interpreter is added to the dependencies of the project.\nMake sure to add a python interpreter to the [dependencies] section of the {PROJECT_MANIFEST}, or run:\n\n\tpixi add python"))?;

    // Construct the marker environment for the target platform
    let marker_environment = determine_marker_environment(platform, python_record.as_ref())?;

    // Determine the tags for this particular solve.
    let tags = get_pypi_tags(platform, &system_requirements, python_record.as_ref())?;

    // Construct an interpreter from the conda environment.
    let interpreter = Interpreter::query(python_location, &context.cache).into_diagnostic()?;

    tracing::debug!("[Resolve] Using Python Interpreter: {:?}", interpreter);

    // Resolve the flat indexes from `--find-links`.
    let flat_index = {
        let client = FlatIndexClient::new(&context.registry_client, &context.cache);
        let entries = client
            .fetch(context.index_locations.flat_index())
            .await
            .into_diagnostic()?;
        FlatIndex::from_entries(entries, &tags)
    };

    let in_memory_index = InMemoryIndex::default();
    let config_settings = ConfigSettings::default();

    // Create a shared in-memory index.
    let options = Options::default();
    let build_dispatch = BuildDispatch::new(
        &context.registry_client,
        &context.cache,
        &interpreter,
        &context.index_locations,
        &flat_index,
        &in_memory_index,
        &context.in_flight,
        SetupPyStrategy::default(),
        &config_settings,
        uv_traits::BuildIsolation::Isolated,
        &context.no_build,
        &context.no_binary,
    )
    .with_options(options)
    .with_build_extra_env_vars(env_variables.iter());

    let constraints = conda_python_packages
        .values()
        .map(|(repo, p)| Requirement {
            name: p.name.as_normalized().clone(),
            extras: vec![],
            version_or_url: Some(pep508_rs::VersionOrUrl::Url(VerbatimUrl::unknown(
                repo.url.clone(),
            ))),
            marker: None,
        })
        .collect();

    let manifest = Manifest::new(
        requirements,
        // Vec::new(),
        constraints,
        Vec::new(),
        Vec::new(),
        None,
        Vec::new(),
    );

    let fallback_provider = DefaultResolverProvider::new(
        &context.registry_client,
        DistributionDatabase::new(
            &context.cache,
            &tags,
            &context.registry_client,
            &build_dispatch,
        ),
        &flat_index,
        &tags,
        PythonRequirement::new(&interpreter, &marker_environment),
        AllowedYanks::default(),
        options.exclude_newer,
        build_dispatch.no_binary(),
        &NoBuild::None,
    );
    let provider = CondaResolverProvider {
        fallback: fallback_provider,
        conda_python_identifiers: &conda_python_packages,
    };

    let resolution = Resolver::new_custom_io(
        manifest,
        options,
        &marker_environment,
        PythonRequirement::new(&interpreter, &marker_environment),
        &in_memory_index,
        provider,
    )
    .into_diagnostic()
    .context("failed to resolve pypi dependencies")?
    .with_reporter(UvReporter::new(
        UvReporterOptions::new().with_existing(pb.clone()),
    ))
    .resolve()
    .await
    .into_diagnostic()
    .context("failed to resolve pypi dependencies")?;

    let resolution = Resolution::from(resolution);

    let database = DistributionDatabase::new(
        &context.cache,
        &tags,
        &context.registry_client,
        &build_dispatch,
    );

    let resolution = DistFinder::new(
        &tags,
        &context.registry_client,
        &interpreter,
        &flat_index,
        build_dispatch.no_binary(),
        &NoBuild::None,
    )
    .resolve(&resolution.requirements())
    .await
    .into_diagnostic()
    .context("failed to find matching pypi distributions for the resolution")?;

    let mut locked_packages = LockedPypiPackages::with_capacity(resolution.len());
    for dist in resolution.into_distributions() {
        // If this refers to a conda package we can skip it
        if conda_python_packages.contains_key(dist.name()) {
            continue;
        }

        let pypi_package_data = match dist {
            Dist::Built(dist) => {
                let (url_or_path, hash) = match &dist {
                    BuiltDist::Registry(dist) => {
                        let url = match &dist.file.url {
                            FileLocation::AbsoluteUrl(url) => {
                                UrlOrPath::Url(Url::from_str(url).expect("invalid absolute url"))
                            }
                            FileLocation::Path(path) => UrlOrPath::Path(path.clone()),
                            _ => todo!("unsupported URL"),
                        };

                        let hash =
                            parse_hashes_from_hex(&dist.file.hashes.sha256, &dist.file.hashes.md5);

                        (url, hash)
                    }
                    BuiltDist::DirectUrl(dist) => {
                        let url = dist.url.to_url();
                        let direct_url = Url::parse(&format!("direct+{url}"))
                            .expect("could not create direct-url");
                        (UrlOrPath::Url(direct_url), None)
                    }
                    BuiltDist::Path(dist) => (
                        dist.url
                            .given()
                            .map(|path| UrlOrPath::Path(PathBuf::from(path)))
                            // When using a direct url reference like https://foo/bla.whl we do not have a given
                            .unwrap_or_else(|| UrlOrPath::Url(dist.url.to_url())),
                        None,
                    ),
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
                    url_or_path,
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
                let url_or_path = match source {
                    SourceDist::Registry(reg) => {
                        if let Some(url) = url {
                            UrlOrPath::Url(url)
                        } else {
                            match &reg.file.url {
                                FileLocation::AbsoluteUrl(url) => UrlOrPath::Url(
                                    Url::from_str(url).expect("invalid absolute url"),
                                ),
                                FileLocation::Path(path) => UrlOrPath::Path(path.clone()),
                                _ => todo!("unsupported URL"),
                            }
                        }
                    }
                    SourceDist::DirectUrl(direct) => {
                        let url = direct.url.to_url();
                        Url::parse(&format!("direct+{url}"))
                            .expect("could not create direct-url")
                            .into()
                    }
                    SourceDist::Git(git) => git.url.to_url().into(),
                    SourceDist::Path(path) => {
                        // Create the url for the lock file
                        path.url
                            .given()
                            .map(|path| UrlOrPath::Path(PathBuf::from(path)))
                            // When using a direct url reference like https://foo/bla.whl we do not have a given
                            .unwrap_or_else(|| path.url.to_url().into())
                    }
                };

                PypiPackageData {
                    name: metadata.name,
                    version: metadata.version,
                    requires_dist: metadata.requires_dist,
                    requires_python: metadata.requires_python,
                    url_or_path,
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
