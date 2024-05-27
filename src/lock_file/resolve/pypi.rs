use super::pypi_editables::build_editables;
use crate::consts::PROJECT_MANIFEST;
use crate::lock_file::resolve::resolver_provider::CondaResolverProvider;
use crate::project::manifest::pypi_options::PypiOptions;
use crate::project::manifest::python::RequirementOrEditable;
use crate::uv_reporter::{UvReporter, UvReporterOptions};
use std::collections::HashMap;

use std::iter::once;

use crate::lock_file::{
    package_identifier, PypiPackageIdentifier, PypiRecord, UvResolutionContext,
};
use crate::pypi_marker_env::determine_marker_environment;
use crate::pypi_tags::{get_pypi_tags, is_python_record};
use crate::{
    lock_file::LockedPypiPackages,
    project::manifest::{PyPiRequirement, SystemRequirements},
};

use distribution_types::{
    BuiltDist, Dist, FlatIndexLocation, HashPolicy, IndexUrl, Name, Resolution, ResolvedDist,
    SourceDist,
};
use distribution_types::{FileLocation, RequirementSource};
use indexmap::{IndexMap, IndexSet};
use indicatif::ProgressBar;
use install_wheel_rs::linker::LinkMode;
use itertools::{Either, Itertools};
use miette::{Context, IntoDiagnostic};
use pep440_rs::{Operator, VersionSpecifier};
use pep508_rs::{VerbatimUrl, VersionOrUrl};
use pypi_types::VerbatimParsedUrl;
use pypi_types::{HashAlgorithm, HashDigest};
use rattler_conda_types::RepoDataRecord;
use rattler_digest::{parse_digest_from_hex, Md5, Sha256};
use rattler_lock::{
    PackageHashes, PypiPackageData, PypiPackageEnvironmentData, PypiSourceTreeHashable, UrlOrPath,
};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use uv_configuration::{ConfigSettings, Constraints, NoBuild, Overrides, SetupPyStrategy};

use url::Url;
use uv_client::{Connectivity, FlatIndexClient, RegistryClient, RegistryClientBuilder};
use uv_dispatch::BuildDispatch;
use uv_distribution::DistributionDatabase;
use uv_interpreter::Interpreter;
use uv_normalize::PackageName;
use uv_resolver::{
    AllowedYanks, BuiltEditableMetadata, DefaultResolverProvider, FlatIndex, InMemoryIndex,
    Manifest, Options, Preference, PythonRequirement, Resolver,
};
use uv_types::{BuildContext, EmptyInstalledPackages};

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

type CondaPythonPackages = HashMap<PackageName, (RepoDataRecord, PypiPackageIdentifier)>;

/// Convert back to PEP508 without the VerbatimParsedUrl
/// We need this function because we need to convert to the introduced `VerbaimParsedUrl`
/// back to crates.io `VerbatimUrl`, for the locking
fn convert_uv_requirements_to_pep508<'req>(
    requires_dist: impl Iterator<Item = &'req pep508_rs::Requirement<VerbatimParsedUrl>>,
) -> Vec<pep508_rs::Requirement> {
    // Convert back top PEP508 Requirement<VerbatimUrl>
    requires_dist
        .map(|r| pep508_rs::Requirement {
            name: r.name.clone(),
            extras: r.extras.clone(),
            version_or_url: r.version_or_url.clone().map(|v| match v {
                VersionOrUrl::VersionSpecifier(v) => VersionOrUrl::VersionSpecifier(v),
                VersionOrUrl::Url(u) => VersionOrUrl::Url(u.verbatim),
            }),
            marker: r.marker.clone(),
            origin: r.origin.clone(),
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub async fn resolve_pypi(
    context: UvResolutionContext,
    pypi_options: &PypiOptions,
    dependencies: IndexMap<PackageName, IndexSet<PyPiRequirement>>,
    system_requirements: SystemRequirements,
    locked_conda_records: &[RepoDataRecord],
    locked_pypi_packages: &[PypiRecord],
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
            req.into_requirement_with_parsed_url()
                .map(distribution_types::Requirement::from)
                .expect("editable requirements treated as non-editable requirements")
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
            .keyring(context.keyring_provider)
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

    // Create a shared in-memory index.
    let in_memory_index = InMemoryIndex::default();
    let config_settings = ConfigSettings::default();

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
        context.concurrency,
    )
    .with_options(options)
    .with_build_extra_env_vars(env_variables.iter());

    // Constrain the conda packages to the specific python packages
    let constraints = conda_python_packages
        .values()
        .map(|(_, p)| {
            // Create pep440 version from the conda version
            let specifier = VersionSpecifier::from_version(Operator::Equal, p.version.clone())
                .expect("invalid version specifier");

            // Only one requirement source and we just assume that's a PyPI source
            let source = RequirementSource::Registry {
                specifier: specifier.into(),
                index: None,
            };

            distribution_types::Requirement {
                name: p.name.as_normalized().clone(),
                extras: vec![],
                marker: None,
                source,
                origin: None,
            }
        })
        .collect::<Vec<_>>();

    // Build any editables
    let built_editables = build_editables(&editables, &context.cache, &build_dispatch)
        .await
        .into_diagnostic()?
        .into_iter()
        .collect_vec();

    // Create preferences from the locked pypi packages
    // This will ensure minimal lock file updates
    // TODO refactor this later into function
    let preferences = locked_pypi_packages
        .iter()
        .map(|record| {
            let (package_data, _) = record;
            Preference::simple(package_data.name.clone(), package_data.version.clone())

            // let version =
            //     VersionSpecifier::from_version(Operator::Equal, package_data.version.clone())
            //         .expect("invalid version specifier");

            // let source = match &package_data.url_or_path {
            //     UrlOrPath::Url(url) => {
            //         // Strip the direct+ prefix
            //         // so that we can pass the url to uv
            //         if let Some(url) = url.as_ref().strip_prefix("direct+") {
            //             let url = url.parse::<Url>().expect("could not parse direct+ url");
            //             let direct_url =
            //                 ParsedUrl::try_from(url.clone()).expect("could not parse direct+ url");
            //             RequirementSource::from_parsed_url(direct_url, VerbatimUrl::from_url(url))
            //         } else {
            //             RequirementSource::Registry {
            //                 specifier: VersionSpecifiers::from(version),
            //                 index: None,
            //             }
            //         }
            //     }
            //     UrlOrPath::Path(path) => RequirementSource::Path {
            //         path: path.clone(),
            //         editable: package_data.editable,
            //         url: VerbatimUrl::from_url(
            //             Url::from_file_path(path).expect("could not create file-path url"),
            //         ),
            //     },
            // };

            // let requirement = distribution_types::Requirement {
            //     name: package_data.name.clone(),
            //     extras: environment_data.extras.iter().cloned().collect_vec(),
            //     marker: None,
            //     source,
            //     origin: None,
            // };
        })
        .collect::<Vec<_>>();

    let manifest = Manifest::new(
        requirements,
        Constraints::from_requirements(constraints),
        Overrides::default(),
        preferences,
        None,
        built_editables.clone(),
        uv_resolver::Exclusions::None,
        Vec::new(),
    );

    let fallback_provider = DefaultResolverProvider::new(
        DistributionDatabase::new(
            &registry_client,
            &build_dispatch,
            context.concurrency.downloads,
        ),
        &flat_index,
        &tags,
        PythonRequirement::new(&interpreter, interpreter.python_full_version()),
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
        Some(&marker_environment),
        &PythonRequirement::new(&interpreter, interpreter.python_full_version()),
        &in_memory_index,
        provider,
        EmptyInstalledPackages,
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

    // Collect resolution into locked packages
    lock_pypi_packages(
        conda_python_packages,
        &build_dispatch,
        &registry_client,
        flat_index_locations,
        resolution,
        built_editables,
        context.concurrency.downloads,
    )
    .await
}

/// Create a vector of locked packages from a resolution
async fn lock_pypi_packages<'a>(
    conda_python_packages: CondaPythonPackages,
    build_dispatch: &BuildDispatch<'a>,
    registry_client: &Arc<RegistryClient>,
    flat_index_locations: Vec<FindLinksLocation>,
    resolution: Resolution,
    built_editables: Vec<BuiltEditableMetadata>,
    concurrent_downloads: usize,
) -> miette::Result<Vec<(PypiPackageData, PypiPackageEnvironmentData)>> {
    let mut locked_packages = LockedPypiPackages::with_capacity(resolution.len());
    let database = DistributionDatabase::new(registry_client, build_dispatch, concurrent_downloads);
    for dist in resolution.distributions() {
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
                        let url = match &dist.best_wheel().file.url {
                            FileLocation::AbsoluteUrl(url) => {
                                UrlOrPath::Url(Url::from_str(url).expect("invalid absolute url"))
                            }
                            // I (tim) thinks this only happens for flat path based indexes
                            FileLocation::Path(path) => {
                                let flat_index =
                                    find_links_for(&dist.best_wheel().index, &flat_index_locations)
                                        .expect("flat index does not exist for resolved ids");
                                UrlOrPath::Path(convert_flat_index_path(
                                    &dist.best_wheel().index,
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

                        let hash = parse_hashes_from_hash_vec(&dist.best_wheel().file.hashes);
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
                    .wheel_metadata(dist)
                    .await
                    .expect("failed to get wheel metadata");
                PypiPackageData {
                    name: metadata.name,
                    version: metadata.version,
                    requires_dist: convert_uv_requirements_to_pep508(metadata.requires_dist.iter()),
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
                        (url_or_path, hash, false)
                    }
                    SourceDist::Directory(dir) => {
                        // Compute the hash of the package based on the source tree.
                        let hash = if dir.path.is_dir() {
                            Some(
                                PypiSourceTreeHashable::from_directory(&dir.path)
                                    .into_diagnostic()
                                    .context("failed to compute hash of pypi source tree")?
                                    .hash(),
                            )
                        } else {
                            None
                        };

                        // process the path or url that we get back from uv
                        let given_path = process_uv_path_url(&dir.url);

                        // Create the url for the lock file. This is based on the passed in URL
                        // instead of from the source path to copy the path that was passed in from
                        // the requirement.
                        let url_or_path = UrlOrPath::Path(given_path);
                        (url_or_path, hash, dir.editable)
                    }
                };

                PypiPackageData {
                    name: metadata.name,
                    version: metadata.version,
                    requires_dist: convert_uv_requirements_to_pep508(metadata.requires_dist.iter()),
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
    for editable_metadata in built_editables {
        // Compute the hash of the package based on the source tree.
        let hash = PypiSourceTreeHashable::from_directory(&editable_metadata.built.path)
            .into_diagnostic()
            .context("failed to compute hash of pypi source tree")?
            .hash();

        // Create the url for the lock file. This is based on the passed in URL
        // instead of from the source path to copy the path that was passed in from
        // the requirement.
        let url_or_path = editable_metadata
            .built
            .url
            .given()
            .map(|path| UrlOrPath::Path(PathBuf::from(path)))
            // When using a direct url reference like https://foo/bla.whl we do not have a given
            .unwrap_or_else(|| editable_metadata.built.url.to_url().into());

        let pypi_package_data = PypiPackageData {
            name: editable_metadata.metadata.name,
            version: editable_metadata.metadata.version,
            requires_dist: convert_uv_requirements_to_pep508(
                editable_metadata.metadata.requires_dist.iter(),
            ),
            requires_python: editable_metadata.metadata.requires_python,
            url_or_path,
            hash: Some(hash),
            editable: true,
        };

        locked_packages.push((pypi_package_data, PypiPackageEnvironmentData::default()));
    }
    Ok(locked_packages)
}
