use std::{
    cell::RefCell,
    collections::HashMap,
    iter::once,
    ops::Deref,
    path::{Path, PathBuf},
    rc::Rc,
    str::FromStr,
    sync::Arc,
};

use indexmap::{IndexMap, IndexSet};
use indicatif::ProgressBar;
use itertools::{Either, Itertools};
use miette::{Context, IntoDiagnostic};
use pixi_manifest::{pypi::pypi_options::PypiOptions, PyPiRequirement, SystemRequirements};
use pixi_uv_conversions::{
    as_uv_req, convert_uv_requirements_to_pep508, isolated_names_to_packages,
    names_to_build_isolation, pypi_options_to_index_locations, to_index_strategy, to_normalize,
    to_requirements, to_uv_normalize, to_uv_version, to_version_specifiers, ConversionError,
};
use pypi_modifiers::{
    pypi_marker_env::determine_marker_environment,
    pypi_tags::{get_pypi_tags, is_python_record},
};
use rattler_conda_types::RepoDataRecord;
use rattler_digest::{parse_digest_from_hex, Md5, Sha256};
use rattler_lock::{
    PackageHashes, PypiPackageData, PypiPackageEnvironmentData, PypiSourceTreeHashable, UrlOrPath,
};
use url::Url;
use uv_client::{Connectivity, FlatIndexClient, RegistryClient, RegistryClientBuilder};
use uv_configuration::{ConfigSettings, Constraints, IndexStrategy, LowerBound, Overrides};
use uv_dispatch::BuildDispatch;
use uv_distribution::DistributionDatabase;
use uv_distribution_types::{
    BuiltDist, DependencyMetadata, Diagnostic, Dist, FileLocation, HashPolicy, IndexCapabilities,
    IndexUrl, InstalledDist, InstalledRegistryDist, Name, Resolution, ResolvedDist, SourceDist,
};
use uv_git::GitResolver;
use uv_install_wheel::linker::LinkMode;
use uv_pypi_types::{HashAlgorithm, HashDigest, RequirementSource};
use uv_python::{Interpreter, PythonEnvironment};
use uv_requirements::LookaheadResolver;
use uv_resolver::{
    AllowedYanks, DefaultResolverProvider, FlatIndex, InMemoryIndex, Manifest, Options, Preference,
    Preferences, PythonRequirement, Resolver, ResolverEnvironment,
};
use uv_types::EmptyInstalledPackages;

use crate::{
    lock_file::{
        package_identifier, records_by_name::HasNameVersion,
        resolve::resolver_provider::CondaResolverProvider, LockedPypiPackages,
        PypiPackageIdentifier, PypiRecord, UvResolutionContext,
    },
    uv_reporter::{UvReporter, UvReporterOptions},
};

#[derive(Debug, thiserror::Error)]
#[error("Invalid hash: {0} type: {1}")]
struct InvalidHash(String, String);

fn parse_hashes_from_hash_vec(
    hashes: &Vec<HashDigest>,
) -> Result<Option<PackageHashes>, InvalidHash> {
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
        (Some(sha256), None) => Ok(Some(PackageHashes::Sha256(
            parse_digest_from_hex::<Sha256>(&sha256)
                .ok_or_else(|| InvalidHash(sha256.clone(), "sha256".to_string()))?,
        ))),
        (None, Some(md5)) => Ok(Some(PackageHashes::Md5(
            parse_digest_from_hex::<Md5>(&md5)
                .ok_or_else(|| InvalidHash(md5.clone(), "md5".to_string()))?,
        ))),
        (Some(sha256), Some(md5)) => Ok(Some(PackageHashes::Md5Sha256(
            parse_digest_from_hex::<Md5>(&md5)
                .ok_or_else(|| InvalidHash(md5.clone(), "md5".to_string()))?,
            parse_digest_from_hex::<Sha256>(&sha256)
                .ok_or_else(|| InvalidHash(sha256.clone(), "sha256".to_string()))?,
        ))),
        (None, None) => Ok(None),
    }
}

#[derive(Debug, thiserror::Error)]
enum ProcessPathUrlError {
    #[error("expected given path for {0} but none found")]
    NoGivenPath(String),
    #[error("given path is an invalid file path")]
    InvalidFilePath(String),
}

/// Given a pyproject.toml and either case:
///   1) dependencies = [ foo @ /home/foo ]
///   2) tool.pixi.pypi-dependencies.foo = { path = "/home/foo"}
///
/// uv has different behavior for each.
///
///   1) Because uv processes 1) during the 'source build' first we get a
///      `file::` as a given. Which is never relative. because of PEP508.
///   2) We get our processed path as a given, which can be relative, as our
///      lock may store relative url's.
///
/// For case 1) we can just use the original path, as it can never be relative.
/// And should be the same For case 2) we need to use the given as it may be
/// relative
///
/// I think this has to do with the order of UV processing the requirements
fn process_uv_path_url(path_url: &uv_pep508::VerbatimUrl) -> Result<PathBuf, ProcessPathUrlError> {
    let given = path_url
        .given()
        .ok_or_else(|| ProcessPathUrlError::NoGivenPath(path_url.to_string()))?;
    if given.starts_with("file://") {
        path_url
            .to_file_path()
            .map_err(|_| ProcessPathUrlError::InvalidFilePath(path_url.to_string()))
    } else {
        Ok(PathBuf::from(given))
    }
}

type CondaPythonPackages =
    HashMap<uv_normalize::PackageName, (RepoDataRecord, PypiPackageIdentifier)>;

/// Prints the number of overridden uv PyPI package requests
fn print_overridden_requests(package_requests: &HashMap<uv_normalize::PackageName, u32>) {
    if !package_requests.is_empty() {
        // Print package requests in form of (PackageName, NumRequest)
        let package_requests = package_requests
            .iter()
            .map(|(name, value)| format!("[{name}: {value}]"))
            .collect::<Vec<_>>()
            .join(",");
        tracing::debug!("overridden uv PyPI package requests [name: amount]: {package_requests}");
    } else {
        tracing::debug!("no uv PyPI package requests overridden by locked conda dependencies");
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn resolve_pypi(
    context: UvResolutionContext,
    pypi_options: &PypiOptions,
    dependencies: IndexMap<uv_normalize::PackageName, IndexSet<PyPiRequirement>>,
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
        .map_ok(|(record, p)| {
            Ok((
                uv_normalize::PackageName::new(p.name.as_normalized().to_string())?,
                (record.clone(), p),
            ))
        })
        .collect::<Result<Result<HashMap<_, _>, uv_normalize::InvalidNameError>, _>>()
        .into_diagnostic()?
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

    let requirements = dependencies
        .into_iter()
        .flat_map(|(name, req)| {
            req.into_iter()
                .map(move |r| as_uv_req(&r, name.as_ref(), project_root))
        })
        .collect::<Result<Vec<_>, _>>()
        .into_diagnostic()?;

    use pixi_consts::consts::PROJECT_MANIFEST;
    // Determine the python interpreter that is installed as part of the conda
    // packages.
    let python_record = locked_conda_records
        .iter()
        .find(|r| is_python_record(r))
        .ok_or_else(|| miette::miette!("could not resolve pypi dependencies because no python interpreter is added to the dependencies of the project.\nMake sure to add a python interpreter to the [dependencies] section of the {PROJECT_MANIFEST}, or run:\n\n\tpixi add python"))?;

    // Construct the marker environment for the target platform
    let marker_environment = determine_marker_environment(platform, python_record.as_ref())?;

    // Determine the tags for this particular solve.
    let tags = get_pypi_tags(platform, &system_requirements, python_record.as_ref())?;

    // We need to setup both an interpreter and a requires_python specifier.
    // The interpreter is used to (potentially) build the wheel, and the requires_python specifier is used
    // to determine the python version of the wheel.
    // So make sure the interpreter does not touch the solve parts of this function
    let interpreter_version = python_record
        .version()
        .as_major_minor()
        .ok_or_else(|| miette::miette!("conda python record missing major.minor version"))?;
    let pep_version = uv_pep440::Version::from_str(&format!(
        "{}.{}",
        interpreter_version.0, interpreter_version.1
    ))
    .into_diagnostic()
    .context("error parsing pep440 version for python interpreter")?;
    let python_specifier =
        uv_pep440::VersionSpecifier::from_version(uv_pep440::Operator::EqualStar, pep_version)
            .into_diagnostic()
            .context("error creating version specifier for python version")?;
    let requires_python = uv_resolver::RequiresPython::from_specifiers(
        &uv_pep440::VersionSpecifiers::from(python_specifier),
    );
    let interpreter = Interpreter::query(python_location, &context.cache)
        .into_diagnostic()
        .wrap_err("failed to query python interpreter")?;
    tracing::debug!(
        "using python interpreter (should be assumed for building only): {}",
        interpreter.key()
    );
    tracing::info!(
        "using requires python specifier (this may differ from the above): {}",
        requires_python
    );

    let index_locations =
        pypi_options_to_index_locations(pypi_options, project_root).into_diagnostic()?;

    // TODO: create a cached registry client per index_url set?
    let index_strategy = to_index_strategy(pypi_options.index_strategy.as_ref());
    let registry_client = Arc::new(
        RegistryClientBuilder::new(context.cache.clone())
            .client(context.client.clone())
            .index_urls(index_locations.index_urls())
            .index_strategy(index_strategy)
            .markers(&marker_environment)
            .keyring(context.keyring_provider)
            .connectivity(Connectivity::Online)
            .build(),
    );

    // Resolve the flat indexes from `--find-links`.
    let flat_index = {
        let client = FlatIndexClient::new(&registry_client, &context.cache);
        let entries = client
            .fetch(
                index_locations
                    .flat_indexes()
                    .map(uv_distribution_types::Index::url),
            )
            .await
            .into_diagnostic()
            .wrap_err("failed to query find-links locations")?;
        FlatIndex::from_entries(
            entries,
            Some(&tags),
            &context.hash_strategy,
            &context.build_options,
        )
    };

    // Create a shared in-memory index.
    // We need two in-memory indexes, one for the build dispatch and one for the resolver.
    // because we manually override requests for the resolver,
    // but we don't want to override requests for the build dispatch.
    //
    // The BuildDispatch might resolve or install when building wheels which will be mostly
    // with build isolation. In that case we want to use fresh non-tampered requests.
    let build_dispatch_in_memory_index = InMemoryIndex::default();
    let config_settings = ConfigSettings::default();

    let env = PythonEnvironment::from_interpreter(interpreter.clone());
    let non_isolated_packages =
        isolated_names_to_packages(pypi_options.no_build_isolation.as_deref()).into_diagnostic()?;
    let build_isolation = names_to_build_isolation(non_isolated_packages.as_deref(), &env);
    tracing::debug!("using build-isolation: {:?}", build_isolation);

    let dependency_metadata = DependencyMetadata::default();
    let options = Options {
        index_strategy,
        ..Options::default()
    };
    let git_resolver = GitResolver::default();
    let build_dispatch = BuildDispatch::new(
        &registry_client,
        &context.cache,
        Constraints::default(),
        &interpreter,
        &index_locations,
        &flat_index,
        &dependency_metadata,
        // TODO: could use this later to add static metadata
        &build_dispatch_in_memory_index,
        &git_resolver,
        &context.capabilities,
        &context.in_flight,
        IndexStrategy::default(),
        &config_settings,
        build_isolation,
        LinkMode::default(),
        &context.build_options,
        &context.hash_strategy,
        None,
        LowerBound::default(),
        context.source_strategy,
        context.concurrency,
    )
    .with_build_extra_env_vars(env_variables.iter());

    // Constrain the conda packages to the specific python packages
    let constraints = conda_python_packages
        .values()
        .map(|(_, p)| {
            // Create pep440 version from the conda version
            let specifier = uv_pep440::VersionSpecifier::from_version(
                uv_pep440::Operator::Equal,
                to_uv_version(&p.version)?,
            )?;

            // Only one requirement source and we just assume that's a PyPI source
            let source = RequirementSource::Registry {
                specifier: specifier.into(),
                index: None,
            };

            Ok::<_, ConversionError>(uv_pypi_types::Requirement {
                name: to_uv_normalize(p.name.as_normalized())?,
                extras: vec![],
                marker: Default::default(),
                source,
                origin: None,
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .into_diagnostic()?;

    // Create preferences from the locked pypi packages
    // This will ensure minimal lock file updates
    // TODO refactor this later into function
    let preferences = locked_pypi_packages
        .iter()
        .map(|record| {
            let (package_data, _) = record;
            // Fake being an InstalledRegistryDist
            let installed = InstalledRegistryDist {
                name: to_uv_normalize(&package_data.name)?,
                version: to_uv_version(&package_data.version)?,
                // This is not used, so we can just set it to a random value
                path: PathBuf::new().join("does_not_exist"),
                cache_info: None,
            };
            Ok(Preference::from_installed(&InstalledDist::Registry(
                installed,
            )))
        })
        .collect::<Result<Vec<_>, ConversionError>>()
        .into_diagnostic()?;

    let resolver_env = ResolverEnvironment::specific(marker_environment.clone().into());

    let constraints = Constraints::from_requirements(constraints.iter().cloned());
    let lookahead_index = InMemoryIndex::default();
    let lookaheads = LookaheadResolver::new(
        &requirements,
        &constraints,
        &Overrides::default(),
        &[],
        &context.hash_strategy,
        &lookahead_index,
        DistributionDatabase::new(
            &registry_client,
            &build_dispatch,
            context.concurrency.downloads,
        ),
    )
    .with_reporter(UvReporter::new(
        UvReporterOptions::new().with_existing(pb.clone()),
    ))
    .resolve(&resolver_env)
    .await
    .into_diagnostic()?;

    let manifest = Manifest::new(
        requirements,
        constraints,
        Overrides::default(),
        Default::default(),
        Preferences::from_iter(preferences, &resolver_env),
        None,
        None,
        uv_resolver::Exclusions::None,
        lookaheads,
    );

    let fallback_provider = DefaultResolverProvider::new(
        DistributionDatabase::new(
            &registry_client,
            &build_dispatch,
            context.concurrency.downloads,
        ),
        &flat_index,
        Some(&tags),
        &requires_python,
        AllowedYanks::from_manifest(&manifest, &resolver_env, options.dependency_mode),
        &context.hash_strategy,
        options.exclude_newer,
        &context.build_options,
        &context.capabilities,
    );
    let package_requests = Rc::new(RefCell::new(Default::default()));
    let provider = CondaResolverProvider {
        fallback: fallback_provider,
        conda_python_identifiers: &conda_python_packages,
        package_requests: package_requests.clone(),
    };

    // We need a new in-memory index for the resolver so that it does not conflict with the build dispatch
    // one. As we have noted in the comment above.
    let resolver_in_memory_index = InMemoryIndex::default();
    let resolution = Resolver::new_custom_io(
        manifest,
        options,
        &context.hash_strategy,
        resolver_env,
        &PythonRequirement::from_marker_environment(&marker_environment, requires_python.clone()),
        &resolver_in_memory_index,
        &git_resolver,
        &context.capabilities,
        &index_locations,
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

    // Print the overridden package requests
    print_overridden_requests(package_requests.borrow().deref());

    // Print any diagnostics
    for diagnostic in resolution.diagnostics() {
        tracing::warn!("{}", diagnostic.message());
    }

    // Collect resolution into locked packages
    lock_pypi_packages(
        conda_python_packages,
        &build_dispatch,
        &registry_client,
        resolution,
        &context.capabilities,
        context.concurrency.downloads,
        project_root,
    )
    .await
}

#[derive(Debug, thiserror::Error)]
enum GetUrlOrPathError {
    #[error("expected absolute path found: {path}", path = .0.display())]
    InvalidAbsolutePath(PathBuf),
    #[error("invalid base url: {0}")]
    InvalidBaseUrl(String),
    #[error("cannot join these urls {0} + {1}")]
    CannotJoin(String, String),
    #[error("expected path found: {0}")]
    ExpectedPath(String),
}

/// Get the UrlOrPath from the index url and file location
/// This will be used to handle the case of a source or built distribution
/// coming from a registry index or a `--find-links` path
fn get_url_or_path(
    index_url: &IndexUrl,
    file_location: &FileLocation,
    abs_project_root: &Path,
) -> Result<UrlOrPath, GetUrlOrPathError> {
    const RELATIVE_BASE: &str = "./";
    match index_url {
        // This is the case where the registry index is a PyPI index
        // or an URL
        IndexUrl::Pypi(_) | IndexUrl::Url(_) => {
            let url = match file_location {
                // Normal case, can be something like:
                // https://files.pythonhosted.org/packages/12/90/3c9ff0512038035f59d279fddeb79f5f1eccd8859f06d6163c58798b9487/certifi-2024.8.30-py3-none-any.whl
                FileLocation::AbsoluteUrl(url) => {
                    UrlOrPath::Url(Url::from_str(url.as_ref()).map_err(|_| {
                        GetUrlOrPathError::InvalidAbsolutePath(PathBuf::from(url.to_string()))
                    })?)
                }
                // This happens when it is relative to the non-standard index
                // because we only lock absolute URLs, we need to join with the base
                FileLocation::RelativeUrl(base, relative) => {
                    let base = Url::from_str(base)
                        .map_err(|_| GetUrlOrPathError::InvalidBaseUrl(base.clone()))?;
                    let url = base.join(relative).map_err(|_| {
                        GetUrlOrPathError::CannotJoin(base.to_string(), relative.clone())
                    })?;
                    UrlOrPath::Url(url)
                }
            };
            Ok(url)
        }
        // From observation this is the case where the index is a `--find-links` path
        // i.e a path to a directory. This is not a PyPI index, but a directory or a file with links to wheels
        IndexUrl::Path(_) => {
            let url = match file_location {
                // Okay we would have something like:
                // file:///home/user/project/dist/certifi-2024.8.30-py3-none-any.whl
                FileLocation::AbsoluteUrl(url) => {
                    // Convert to a relative path from the base path
                    let absolute = url
                        .to_url()
                        .to_file_path()
                        .map_err(|_| GetUrlOrPathError::ExpectedPath(url.to_string()))?;
                    // !IMPORTANT! We need to strip the base path from the absolute path
                    // not the path returned by the uv solver. Why? Because we need the path relative
                    // to the project root, **not** the path relative to the --find-links path.
                    // This is because during installation we do something like: `project_root.join(relative_path)`
                    let relative = absolute.strip_prefix(abs_project_root);
                    let path = match relative {
                        // Apparently, we can make it relative to the project root
                        Ok(relative) => PathBuf::from_str(RELATIVE_BASE)
                            .map_err(|_| {
                                GetUrlOrPathError::ExpectedPath(RELATIVE_BASE.to_string())
                            })?
                            .join(relative),
                        // We can't make it relative to the project root
                        // so we just return the absolute path
                        Err(_) => absolute,
                    };
                    UrlOrPath::Path(path)
                }
                // This happens when it is relative to the non-standard index
                // location on disk.
                FileLocation::RelativeUrl(base, relative) => {
                    // This is the same logic as the `AbsoluteUrl` case
                    // basically but we just make an absolute path first
                    let absolute = PathBuf::from_str(base)
                        .map_err(|_| GetUrlOrPathError::ExpectedPath(base.clone()))?;
                    let relative = PathBuf::from_str(relative)
                        .map_err(|_| GetUrlOrPathError::ExpectedPath(relative.clone()))?;
                    let absolute = absolute.join(relative);

                    let relative = absolute.strip_prefix(abs_project_root);
                    let path = match relative {
                        Ok(relative) => PathBuf::from_str(RELATIVE_BASE)
                            .map_err(|_| {
                                GetUrlOrPathError::ExpectedPath(RELATIVE_BASE.to_string())
                            })?
                            .join(relative),
                        Err(_) => absolute,
                    };
                    UrlOrPath::Path(path)
                }
            };
            Ok(url)
        }
    }
}

/// Create a vector of locked packages from a resolution
async fn lock_pypi_packages<'a>(
    conda_python_packages: CondaPythonPackages,
    build_dispatch: &BuildDispatch<'a>,
    registry_client: &Arc<RegistryClient>,
    resolution: Resolution,
    index_capabilities: &IndexCapabilities,
    concurrent_downloads: usize,
    abs_project_root: &Path,
) -> miette::Result<Vec<(PypiPackageData, PypiPackageEnvironmentData)>> {
    let mut locked_packages = LockedPypiPackages::with_capacity(resolution.len());
    let database = DistributionDatabase::new(registry_client, build_dispatch, concurrent_downloads);
    for dist in resolution.distributions() {
        // If this refers to a conda package we can skip it
        if conda_python_packages.contains_key(dist.name()) {
            continue;
        }

        let pypi_package_data = match dist {
            // Ignore installed distributions
            ResolvedDist::Installed(_) => {
                continue;
            }

            ResolvedDist::Installable(Dist::Built(dist)) => {
                let (url_or_path, hash) = match &dist {
                    BuiltDist::Registry(dist) => {
                        let best_wheel = dist.best_wheel();
                        let hash = parse_hashes_from_hash_vec(&dist.best_wheel().file.hashes)
                            .into_diagnostic()
                            .context("cannot parse hashes for registry dist")?;
                        let url_or_path = get_url_or_path(
                            &best_wheel.index,
                            &best_wheel.file.url,
                            abs_project_root,
                        )
                        .into_diagnostic()
                        .context("cannot convert registry dist")?;
                        (url_or_path, hash)
                    }
                    BuiltDist::DirectUrl(dist) => {
                        let url = dist.url.to_url();
                        let direct_url = Url::parse(&format!("direct+{url}"))
                            .into_diagnostic()
                            .context("cannot create direct url")?;

                        (UrlOrPath::Url(direct_url), None)
                    }
                    BuiltDist::Path(dist) => (
                        UrlOrPath::Path(process_uv_path_url(&dist.url).into_diagnostic()?),
                        None,
                    ),
                };

                let metadata = registry_client
                    .wheel_metadata(dist, index_capabilities)
                    .await
                    .into_diagnostic()
                    .wrap_err("cannot get wheel metadata")?;
                PypiPackageData {
                    name: pep508_rs::PackageName::new(metadata.name.to_string())
                        .into_diagnostic()
                        .context("cannot convert name")?,
                    version: pep440_rs::Version::from_str(&metadata.version.to_string())
                        .into_diagnostic()
                        .context("cannot convert version")?,
                    requires_python: metadata
                        .requires_python
                        .map(|r| to_version_specifiers(&r))
                        .transpose()
                        .into_diagnostic()?,
                    requires_dist: convert_uv_requirements_to_pep508(metadata.requires_dist.iter())
                        .into_diagnostic()?,
                    editable: false,
                    url_or_path,
                    hash,
                }
            }
            ResolvedDist::Installable(Dist::Source(source)) => {
                // Handle new hash stuff
                let hash = source
                    .file()
                    .and_then(|file| {
                        parse_hashes_from_hash_vec(&file.hashes)
                            .into_diagnostic()
                            .context("cannot parse hashes for sdist")
                            .transpose()
                    })
                    .transpose()?;

                let metadata_response = database
                    .get_or_build_wheel_metadata(&Dist::Source(source.clone()), HashPolicy::None)
                    .await
                    .into_diagnostic()?;
                let metadata = metadata_response.metadata;

                // Use the precise url if we got it back
                // otherwise try to construct it from the source
                let (url_or_path, hash, editable) = match source {
                    SourceDist::Registry(reg) => {
                        let url_or_path =
                            get_url_or_path(&reg.index, &reg.file.url, abs_project_root)
                                .into_diagnostic()
                                .context("cannot convert registry sdist")?;
                        (url_or_path, hash, false)
                    }
                    SourceDist::DirectUrl(direct) => {
                        let url = direct.url.to_url();
                        let direct_url = Url::parse(&format!("direct+{url}"))
                            .into_diagnostic()
                            .context("could not create direct-url")?;
                        (direct_url.into(), hash, false)
                    }
                    SourceDist::Git(git) => (git.url.to_url().into(), hash, false),
                    SourceDist::Path(path) => {
                        // Compute the hash of the package based on the source tree.
                        let hash = if path.install_path.is_dir() {
                            Some(
                                PypiSourceTreeHashable::from_directory(&path.install_path)
                                    .into_diagnostic()
                                    .context("failed to compute hash of pypi source tree")?
                                    .hash(),
                            )
                        } else {
                            None
                        };

                        // process the path or url that we get back from uv
                        let given_path = process_uv_path_url(&path.url).into_diagnostic()?;

                        // Create the url for the lock file. This is based on the passed in URL
                        // instead of from the source path to copy the path that was passed in from
                        // the requirement.
                        let url_or_path = UrlOrPath::Path(given_path);
                        (url_or_path, hash, false)
                    }
                    SourceDist::Directory(dir) => {
                        // Compute the hash of the package based on the source tree.
                        let hash = if dir.install_path.is_dir() {
                            Some(
                                PypiSourceTreeHashable::from_directory(&dir.install_path)
                                    .into_diagnostic()
                                    .context("failed to compute hash of pypi source tree")?
                                    .hash(),
                            )
                        } else {
                            None
                        };

                        // process the path or url that we get back from uv
                        let given_path = process_uv_path_url(&dir.url).into_diagnostic()?;

                        // Create the url for the lock file. This is based on the passed in URL
                        // instead of from the source path to copy the path that was passed in from
                        // the requirement.
                        let url_or_path = UrlOrPath::Path(given_path);
                        (url_or_path, hash, dir.editable)
                    }
                };

                PypiPackageData {
                    name: to_normalize(&metadata.name).into_diagnostic()?,
                    version: pep440_rs::Version::from_str(&metadata.version.to_string())
                        .into_diagnostic()?,
                    requires_python: metadata
                        .requires_python
                        .map(|r| to_version_specifiers(&r))
                        .transpose()
                        .into_diagnostic()?,
                    requires_dist: to_requirements(metadata.requires_dist.iter())
                        .into_diagnostic()?,
                    url_or_path,
                    hash,
                    editable,
                }
            }
        };

        // TODO: Store extras in the lock-file
        locked_packages.push((pypi_package_data, PypiPackageEnvironmentData::default()));
    }

    Ok(locked_packages)
}
