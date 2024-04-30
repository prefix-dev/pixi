//! This module contains code to resolve python package from PyPi or Conda packages.
//!
//! See [`resolve_pypi`] and [`resolve_conda`] for more information.

use crate::config::get_cache_dir;
use crate::consts::PROJECT_MANIFEST;
use crate::lock_file::pypi_editables::build_editables;
use crate::project::manifest::pypi_options::PypiOptions;
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
    BuiltDist, DirectUrlSourceDist, Dist, FlatIndexLocation, HashPolicy, IndexLocations, IndexUrl,
    Name, PrioritizedDist, Resolution, ResolvedDist, SourceDist,
};
use distribution_types::{FileLocation, SourceDistCompatibility};
use futures::FutureExt;
use indexmap::{IndexMap, IndexSet};
use indicatif::ProgressBar;
use install_wheel_rs::linker::LinkMode;
use itertools::{Either, Itertools};
use miette::{Context, IntoDiagnostic};
use pep508_rs::{Requirement, VerbatimUrl};
use pypi_types::{HashAlgorithm, HashDigest, Metadata23};
use rattler_conda_types::{GenericVirtualPackage, MatchSpec, RepoDataRecord};
use rattler_digest::{parse_digest_from_hex, Md5, Sha256};
use rattler_lock::{
    PackageHashes, PypiPackageData, PypiPackageEnvironmentData, PypiSourceTreeHashable, UrlOrPath,
};
use rattler_solve::{resolvo, ChannelPriority, SolverImpl};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use uv_configuration::{
    ConfigSettings, Constraints, NoBinary, NoBuild, Overrides, SetupPyStrategy,
};

use url::Url;
use uv_cache::Cache;
use uv_client::{Connectivity, FlatIndexClient, RegistryClientBuilder};
use uv_dispatch::BuildDispatch;
use uv_distribution::{ArchiveMetadata, DistributionDatabase, Reporter};
use uv_interpreter::Interpreter;
use uv_normalize::PackageName;
use uv_resolver::{
    AllowedYanks, DefaultResolverProvider, FlatIndex, InMemoryIndex, Manifest, MetadataResponse,
    Options, PythonRequirement, Resolver, ResolverProvider, VersionMap, VersionsResponse,
    WheelMetadataResult,
};
use uv_types::{BuildContext, EmptyInstalledPackages, HashStrategy, InFlight};

/// Objects that are needed for resolutions which can be shared between different resolutions.
#[derive(Clone)]
pub struct UvResolutionContext {
    pub cache: Cache,
    pub in_flight: Arc<InFlight>,
    pub no_build: NoBuild,
    pub no_binary: NoBinary,
    pub hash_strategy: HashStrategy,
    pub client: reqwest::Client,
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
        let in_flight = Arc::new(InFlight::default());
        Ok(Self {
            cache,
            in_flight,
            no_build: NoBuild::None,
            no_binary: NoBinary::None,
            hash_strategy: HashStrategy::None,
            client: project.client().clone(),
        })
    }
}

fn parse_hashes_from_hash_vec(hashes: &Vec<HashDigest>) -> Option<PackageHashes> {
    let mut sha256 = None;
    let mut md5 = None;

    for hash in hashes {
        match hash.algorithm() {
            HashAlgorithm::Sha256 => {
                sha256 = Some(hash.digest.to_string());
            }
            HashAlgorithm::Md5 => {
                md5 = Some(hash.digest.to_string());
            }
            HashAlgorithm::Sha384 | HashAlgorithm::Sha512 => {
                // We do not support these algorithms
            }
        }
    }

    match (sha256, md5) {
        (Some(sha256), None) => Some(PackageHashes::Sha256(
            parse_digest_from_hex::<Sha256>(&sha256).expect("invalid sha256"),
        )),
        (None, Some(md5)) => Some(PackageHashes::Md5(
            parse_digest_from_hex::<Md5>(&md5).expect("invalid md5"),
        )),
        (Some(sha256), Some(md5)) => Some(PackageHashes::Md5Sha256(
            parse_digest_from_hex::<Md5>(&md5).expect("invalid md5"),
            parse_digest_from_hex::<Sha256>(&sha256).expect("invalid sha256"),
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

            let prioritized_dist = PrioritizedDist::from_source(
                dist,
                Vec::new(),
                SourceDistCompatibility::Compatible(distribution_types::Hash::Matched),
            );

            return ready(Ok(VersionsResponse::Found(vec![VersionMap::from(
                BTreeMap::from_iter([(identifier.version.clone(), prioritized_dist)]),
            )])))
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
    ) -> impl Future<Output = WheelMetadataResult> + Send + 'io {
        if let Dist::Source(SourceDist::DirectUrl(DirectUrlSourceDist { name, .. })) = dist {
            if let Some((_, iden)) = self.conda_python_identifiers.get(name) {
                // If this is a Source dist and the package is actually installed by conda we
                // create fake metadata with no dependencies. We assume that all conda installed
                // packages are properly installed including its dependencies.
                return ready(Ok(MetadataResponse::Found(ArchiveMetadata {
                    metadata: Metadata23 {
                        name: iden.name.as_normalized().clone(),
                        version: iden.version.clone(),
                        requires_dist: vec![],
                        requires_python: None,
                        provides_extras: iden.extras.iter().cloned().collect(),
                    },
                    hashes: vec![],
                })))
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

/// Given a pyproject.toml and either case:
///   1) dependencies = [ foo @ /home/foo ]
///   2) tool.pixi.pypi-depencies.foo = { path = "/home/foo"}
/// uv has different behavior for each.
///
///   1) Because uv processes 1) during the 'source build' first we get a `file::` as a given. Which is never relative.
///        because of PEP508.
///   2) We get our processed path as a given, which can be relative, as our lock may store relative url's.
///
/// For case 1) we can just use the original path, as it can never be relative. And should be the same
/// For case 2) we need to use the given as it may be relative
///
/// I think this has to do with the order of UV processing the requirements
fn process_uv_path_url(path_url: &VerbatimUrl) -> PathBuf {
    let given = path_url.given().expect("path should have a given url");
    if given.starts_with("file://") {
        path_url
            .to_file_path()
            .expect("path should be a valid file path")
    } else {
        PathBuf::from(given)
    }
}

// Store a reference to the flat index
#[derive(Clone)]
struct FindLinksLocation {
    /// Canocialized path to the flat index.
    canonicalized_path: PathBuf,
    /// Manifest path to flat index.
    given_path: PathBuf,
}

/// Given a flat index url and a list of flat indexes, return the path to the flat index.
/// for that specific index.
fn find_links_for(
    flat_index_url: &IndexUrl,
    flat_indexes_paths: &[FindLinksLocation],
) -> Option<FindLinksLocation> {
    // Convert to file path
    let flat_index_url_path = flat_index_url
        .url()
        .to_file_path()
        .expect("invalid path-based index");

    // Find the flat index in the list of flat indexes
    // Compare with the path that we got from the `IndexUrl`
    // which is absolute
    flat_indexes_paths
        .iter()
        .find(|path| path.canonicalized_path == flat_index_url_path)
        .cloned()
}

/// Convert an absolute path to a path relative to the flat index url.
/// which is assumed to be a file:// url.
fn convert_flat_index_path(
    flat_index_url: &IndexUrl,
    absolute_path: &Path,
    given_flat_index_path: &Path,
) -> PathBuf {
    assert!(
        absolute_path.is_absolute(),
        "flat index package does not have an absolute path"
    );
    let base = flat_index_url
        .url()
        .to_file_path()
        .expect("invalid path-based index");
    // Strip the index from the path
    // This is safe because we know the index is a prefix of the path
    let path = absolute_path
        .strip_prefix(&base)
        .expect("base was not a prefix of the flat index path");
    // Join with the given flat index path
    given_flat_index_path.join(path)
}

#[allow(clippy::too_many_arguments)]
pub async fn resolve_pypi(
    context: UvResolutionContext,
    pypi_options: &PypiOptions,
    dependencies: IndexMap<PackageName, IndexSet<PyPiRequirement>>,
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
    // partition the requirements into editable and non-editable requirements
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
        .partition(|req| matches!(req, RequirementOrEditable::Editable(_, _)));

    let editables = editables
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

    let index_locations = pypi_options.to_index_locations();

    // TODO: create a cached registry client per index_url set?
    let registry_client = Arc::new(
        RegistryClientBuilder::new(context.cache.clone())
            .client(context.client.clone())
            .index_urls(index_locations.index_urls())
            .connectivity(Connectivity::Online)
            .build(),
    );
    // Resolve the flat indexes from `--find-links`.
    let flat_index = {
        let client = FlatIndexClient::new(&registry_client, &context.cache);
        let entries = client
            .fetch(index_locations.flat_index())
            .await
            .into_diagnostic()?;
        FlatIndex::from_entries(
            entries,
            &tags,
            &context.hash_strategy,
            &context.no_build,
            &context.no_binary,
        )
    };

    let in_memory_index = InMemoryIndex::default();
    let config_settings = ConfigSettings::default();

    // Create a shared in-memory index.
    let options = Options::default();
    let build_dispatch = BuildDispatch::new(
        &registry_client,
        &context.cache,
        &interpreter,
        &index_locations,
        &flat_index,
        &in_memory_index,
        &context.in_flight,
        SetupPyStrategy::default(),
        &config_settings,
        uv_types::BuildIsolation::Isolated,
        LinkMode::default(),
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
        .collect::<Vec<_>>();

    // Build any editables
    let built_editables = build_editables(&editables, &context.cache, &build_dispatch)
        .await
        .into_diagnostic()?
        .into_iter()
        .collect_vec();

    let manifest = Manifest::new(
        requirements,
        Constraints::from_requirements(constraints),
        Overrides::default(),
        Vec::new(),
        None,
        built_editables.clone(),
        uv_resolver::Exclusions::None,
        Vec::new(),
    );

    let fallback_provider = DefaultResolverProvider::new(
        &registry_client,
        DistributionDatabase::new(&registry_client, &build_dispatch),
        &flat_index,
        &tags,
        PythonRequirement::new(&interpreter, &marker_environment),
        AllowedYanks::default(),
        &context.hash_strategy,
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
        &context.hash_strategy,
        &marker_environment,
        PythonRequirement::new(&interpreter, &marker_environment),
        &in_memory_index,
        provider,
        &EmptyInstalledPackages,
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

    // Create a list of canocialized flat indexes.
    let flat_index_locations = index_locations
        .flat_index()
        // Take only path based flat indexes
        .filter_map(|i| match i {
            FlatIndexLocation::Path(path) => Some(path),
            FlatIndexLocation::Url(_) => None,
        })
        // Canonicalize the path
        .map(|path| {
            let canonicalized_path = path.canonicalize()?;
            Ok::<_, std::io::Error>(FindLinksLocation {
                canonicalized_path,
                given_path: path.clone(),
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .into_diagnostic()?;

    let database = DistributionDatabase::new(&registry_client, &build_dispatch);

    let mut locked_packages = LockedPypiPackages::with_capacity(resolution.len());
    for dist in resolution.into_distributions() {
        // If this refers to a conda package we can skip it
        if conda_python_packages.contains_key(dist.name()) {
            continue;
        }

        let pypi_package_data = match dist {
            ResolvedDist::Installed(_) => {
                // TODO handle installed distributions
                continue;
            }
            ResolvedDist::Installable(Dist::Built(dist)) => {
                let (url_or_path, hash) = match &dist {
                    BuiltDist::Registry(dist) => {
                        let url = match &dist.file.url {
                            FileLocation::AbsoluteUrl(url) => {
                                UrlOrPath::Url(Url::from_str(url).expect("invalid absolute url"))
                            }
                            // I (tim) thinks this only happens for flat path based indexes
                            FileLocation::Path(path) => {
                                let flat_index = find_links_for(&dist.index, &flat_index_locations)
                                    .expect("flat index does not exist for resolved ids");
                                UrlOrPath::Path(convert_flat_index_path(
                                    &dist.index,
                                    path,
                                    &flat_index.given_path,
                                ))
                            }
                            // This happens when it is relative to the non-standard index
                            FileLocation::RelativeUrl(base, relative) => {
                                let base = Url::from_str(base).expect("invalid base url");
                                let url = base.join(relative).expect("could not join urls");
                                UrlOrPath::Url(url)
                            }
                        };

                        let hash = parse_hashes_from_hash_vec(&dist.file.hashes);
                        (url, hash)
                    }
                    BuiltDist::DirectUrl(dist) => {
                        let url = dist.url.to_url();
                        let direct_url = Url::parse(&format!("direct+{url}"))
                            .expect("could not create direct-url");
                        (UrlOrPath::Url(direct_url), None)
                    }
                    BuiltDist::Path(dist) => {
                        (UrlOrPath::Path(process_uv_path_url(&dist.url)), None)
                    }
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
                    editable: false,
                    url_or_path,
                    hash,
                }
            }
            ResolvedDist::Installable(Dist::Source(source)) => {
                // Handle new hash stuff
                let hash = source
                    .file()
                    .and_then(|file| parse_hashes_from_hash_vec(&file.hashes));

                let metadata_response = database
                    .get_or_build_wheel_metadata(&Dist::Source(source.clone()), HashPolicy::None)
                    .await
                    .into_diagnostic()?;
                let metadata = metadata_response.metadata;

                // Use the precise url if we got it back
                // otherwise try to construct it from the source
                let (url_or_path, hash, editable) = match source {
                    SourceDist::Registry(reg) => {
                        let url_or_path = match &reg.file.url {
                            FileLocation::AbsoluteUrl(url) => {
                                UrlOrPath::Url(Url::from_str(url).expect("invalid absolute url"))
                            }
                            // I (tim) thinks this only happens for flat path based indexes
                            FileLocation::Path(path) => {
                                let flat_index = find_links_for(&reg.index, &flat_index_locations)
                                    .expect("flat index does not exist for resolved ids");
                                UrlOrPath::Path(convert_flat_index_path(
                                    &reg.index,
                                    path,
                                    &flat_index.given_path,
                                ))
                            }
                            // This happens when it is relative to the non-standard index
                            FileLocation::RelativeUrl(base, relative) => {
                                let base = Url::from_str(base).expect("invalid base url");
                                let url = base.join(relative).expect("could not join urls");
                                UrlOrPath::Url(url)
                            }
                        };
                        (url_or_path, hash, false)
                    }
                    SourceDist::DirectUrl(direct) => {
                        let url = direct.url.to_url();
                        let direct_url = Url::parse(&format!("direct+{url}"))
                            .expect("could not create direct-url");
                        (direct_url.into(), hash, false)
                    }
                    SourceDist::Git(git) => (git.url.to_url().into(), hash, false),
                    SourceDist::Path(path) => {
                        // Compute the hash of the package based on the source tree.
                        let hash = if path.path.is_dir() {
                            Some(
                                PypiSourceTreeHashable::from_directory(&path.path)
                                    .into_diagnostic()
                                    .context("failed to compute hash of pypi source tree")?
                                    .hash(),
                            )
                        } else {
                            None
                        };

                        // process the path or url that we get back from uv
                        let given_path = process_uv_path_url(&path.url);

                        // Create the url for the lock file. This is based on the passed in URL
                        // instead of from the source path to copy the path that was passed in from
                        // the requirement.
                        let url_or_path = UrlOrPath::Path(given_path);
                        (url_or_path, hash, path.editable)
                    }
                };

                PypiPackageData {
                    name: metadata.name,
                    version: metadata.version,
                    requires_dist: metadata.requires_dist,
                    requires_python: metadata.requires_python,
                    url_or_path,
                    hash,
                    editable,
                }
            }
        };

        // TODO: Store extras in the lock-file
        locked_packages.push((pypi_package_data, PypiPackageEnvironmentData::default()));
    }

    // Add the editables to the locked packages as well.
    for (editable, metadata) in built_editables {
        // Compute the hash of the package based on the source tree.
        let hash = PypiSourceTreeHashable::from_directory(&editable.path)
            .into_diagnostic()
            .context("failed to compute hash of pypi source tree")?
            .hash();

        // Create the url for the lock file. This is based on the passed in URL
        // instead of from the source path to copy the path that was passed in from
        // the requirement.
        let url_or_path = editable
            .url
            .given()
            .map(|path| UrlOrPath::Path(PathBuf::from(path)))
            // When using a direct url reference like https://foo/bla.whl we do not have a given
            .unwrap_or_else(|| editable.url.to_url().into());

        let pypi_package_data = PypiPackageData {
            name: metadata.name,
            version: metadata.version,
            requires_dist: metadata.requires_dist,
            requires_python: metadata.requires_python,
            url_or_path,
            hash: Some(hash),
            editable: true,
        };

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
            channel_priority: ChannelPriority::Strict,
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
