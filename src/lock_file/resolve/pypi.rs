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
use pixi_manifest::{
    pypi::pypi_options::PypiOptions, EnvironmentName, PyPiRequirement, SystemRequirements,
};
use pixi_record::PixiRecord;
use pixi_uv_conversions::{
    as_uv_req, convert_uv_requirements_to_pep508, into_pinned_git_spec, no_build_to_build_options,
    pypi_options_to_index_locations, to_index_strategy, to_normalize, to_requirements,
    to_uv_normalize, to_uv_version, to_version_specifiers, ConversionError,
};
use pypi_modifiers::{
    pypi_marker_env::determine_marker_environment,
    pypi_tags::{get_pypi_tags, is_python_record},
};
use rattler_digest::{parse_digest_from_hex, Md5, Sha256};
use rattler_lock::{
    PackageHashes, PypiPackageData, PypiPackageEnvironmentData, PypiSourceTreeHashable, UrlOrPath,
};
use typed_path::Utf8TypedPathBuf;
use url::Url;
use uv_client::{Connectivity, FlatIndexClient, RegistryClient, RegistryClientBuilder};
use uv_configuration::{ConfigSettings, Constraints, Overrides};
use uv_distribution::DistributionDatabase;
use uv_distribution_types::{
    BuiltDist, DependencyMetadata, Diagnostic, Dist, FileLocation, HashPolicy, IndexCapabilities,
    IndexUrl, Name, Resolution, ResolvedDist, SourceDist, ToUrlError,
};
use uv_pypi_types::{Conflicts, HashAlgorithm, HashDigest, RequirementSource};
use uv_requirements::LookaheadResolver;
use uv_resolver::{
    AllowedYanks, DefaultResolverProvider, FlatIndex, InMemoryIndex, Manifest, Options, Preference,
    PreferenceError, Preferences, PythonRequirement, Resolver, ResolverEnvironment,
};
use uv_types::EmptyInstalledPackages;

use crate::{
    environment::CondaPrefixUpdated,
    lock_file::{
        records_by_name::HasNameVersion,
        resolve::{
            build_dispatch::{
                LazyBuildDispatch, LazyBuildDispatchDependencies, UvBuildDispatchParams,
            },
            resolver_provider::CondaResolverProvider,
        },
        CondaPrefixUpdater, LockedPypiPackages, PixiRecordsByName, PypiPackageIdentifier,
        PypiRecord, UvResolutionContext,
    },
    uv_reporter::{UvReporter, UvReporterOptions},
    workspace::{Environment, EnvironmentVars},
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

type CondaPythonPackages = HashMap<uv_normalize::PackageName, (PixiRecord, PypiPackageIdentifier)>;

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
    locked_pixi_records: &[PixiRecord],
    locked_pypi_packages: &[PypiRecord],
    platform: rattler_conda_types::Platform,
    pb: &ProgressBar,
    project_root: &Path,
    prefix_updater: CondaPrefixUpdater,
    platform_repodata_records: Arc<PixiRecordsByName>,
    project_env_vars: HashMap<EnvironmentName, EnvironmentVars>,
    environment_name: Environment<'_>,
    disallow_install_conda_prefix: bool,
) -> miette::Result<(LockedPypiPackages, Option<CondaPrefixUpdated>)> {
    // Solve python packages
    pb.set_message("resolving pypi dependencies");

    // Determine which pypi packages are already installed as conda package.
    let conda_python_packages = locked_pixi_records
        .iter()
        .flat_map(|record| {
            let result = match record {
                PixiRecord::Binary(repodata_record) => {
                    PypiPackageIdentifier::from_repodata_record(repodata_record)
                }
                PixiRecord::Source(source_record) => {
                    PypiPackageIdentifier::from_package_record(&source_record.package_record)
                }
            };

            result.map_or_else(
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
    let python_record = locked_pixi_records
        .iter()
        .find(|r| match r {
            PixiRecord::Binary(r) => is_python_record(r),
            _ => false,
        })
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
            .allow_insecure_host(context.allow_insecure_host.clone())
            .index_urls(index_locations.index_urls())
            .index_strategy(index_strategy)
            .markers(&marker_environment)
            .keyring(context.keyring_provider)
            .connectivity(Connectivity::Online)
            .build(),
    );
    let build_options =
        no_build_to_build_options(&pypi_options.no_build.clone().unwrap_or_default())
            .into_diagnostic()?;

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
        FlatIndex::from_entries(entries, Some(&tags), &context.hash_strategy, &build_options)
    };

    // Hi maintainers! For anyone coming here, if you expose any additional `uv` options, similar to `index_strategy`, make sure to
    // include them in this struct as well instead of relying on the default.
    // Otherwise there be panics.
    let options = Options {
        index_strategy,
        build_options: build_options.clone(),
        ..Options::default()
    };

    let config_settings = ConfigSettings::default();
    let dependency_metadata = DependencyMetadata::default();
    let build_params = UvBuildDispatchParams::new(
        &registry_client,
        &context.cache,
        &index_locations,
        &flat_index,
        &dependency_metadata,
        &config_settings,
        &build_options,
        &context.hash_strategy,
    )
    .with_index_strategy(index_strategy)
    // Create a forked shared state that condains the in-memory index.
    // We need two in-memory indexes, one for the build dispatch and one for the
    // resolver. because we manually override requests for the resolver,
    // but we don't want to override requests for the build dispatch.
    //
    // The BuildDispatch might resolve or install when building wheels which will be
    // mostly with build isolation. In that case we want to use fresh
    // non-tampered requests.
    .with_shared_state(context.shared_state.fork())
    .with_source_strategy(context.source_strategy)
    .with_concurrency(context.concurrency);

    let lazy_build_dispatch_dependencies = LazyBuildDispatchDependencies::default();
    let lazy_build_dispatch = LazyBuildDispatch::new(
        build_params,
        prefix_updater,
        project_env_vars,
        environment_name,
        platform_repodata_records.records.clone(),
        pypi_options.no_build_isolation.clone(),
        &lazy_build_dispatch_dependencies,
        disallow_install_conda_prefix,
    );

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
                conflict: None,
            };

            Ok::<_, ConversionError>(uv_pypi_types::Requirement {
                name: to_uv_normalize(p.name.as_normalized())?,
                extras: vec![],
                marker: Default::default(),
                source,
                groups: Default::default(),
                origin: None,
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .into_diagnostic()?;

    #[derive(Debug, thiserror::Error)]
    enum PixiPreferencesError {
        #[error(transparent)]
        Conversion(#[from] ConversionError),
        #[error(transparent)]
        Preference(#[from] PreferenceError),
    }

    // Create preferences from the locked pypi packages
    // This will ensure minimal lock file updates
    let preferences = locked_pypi_packages
        .iter()
        .map(|record| {
            let (package_data, _) = record;
            let requirement = uv_pep508::Requirement {
                name: to_uv_normalize(&package_data.name)?,
                extras: Vec::new(),
                version_or_url: Some(uv_pep508::VersionOrUrl::VersionSpecifier(
                    uv_pep440::VersionSpecifiers::from(
                        uv_pep440::VersionSpecifier::equals_version(to_uv_version(
                            &package_data.version,
                        )?),
                    ),
                )),
                marker: uv_pep508::MarkerTree::TRUE,
                origin: None,
            };

            let named = uv_requirements_txt::RequirementsTxtRequirement::Named(requirement);
            let entry = uv_requirements_txt::RequirementEntry {
                requirement: named,
                hashes: Default::default(),
            };

            Ok(Preference::from_entry(entry)?)
        })
        .filter_map(|pref| pref.transpose())
        .collect::<Result<Vec<_>, PixiPreferencesError>>()
        .into_diagnostic()?;

    let resolver_env = ResolverEnvironment::specific(marker_environment.clone().into());

    let constraints = Constraints::from_requirements(constraints.iter().cloned());
    let lookahead_index = InMemoryIndex::default();
    let lookaheads = LookaheadResolver::new(
        &requirements,
        &constraints,
        &Overrides::default(),
        &context.hash_strategy,
        &lookahead_index,
        DistributionDatabase::new(
            &registry_client,
            &lazy_build_dispatch,
            context.concurrency.downloads,
        ),
    )
    .with_reporter(UvReporter::new_arc(
        UvReporterOptions::new().with_existing(pb.clone()),
    ))
    .resolve(&resolver_env)
    .await
    .into_diagnostic()?;

    let manifest = Manifest::new(
        requirements,
        constraints,
        Overrides::default(),
        Preferences::from_iter(preferences, &resolver_env),
        None,
        Default::default(),
        uv_resolver::Exclusions::default(),
        lookaheads,
    );

    let provider_tags = tags.clone();
    let fallback_provider = DefaultResolverProvider::new(
        DistributionDatabase::new(
            &registry_client,
            &lazy_build_dispatch,
            context.concurrency.downloads,
        ),
        &flat_index,
        Some(&provider_tags),
        &requires_python,
        AllowedYanks::from_manifest(&manifest, &resolver_env, options.dependency_mode),
        &context.hash_strategy,
        options.exclude_newer,
        &build_options,
        &context.capabilities,
    );
    let package_requests = Rc::new(RefCell::new(Default::default()));
    let provider = CondaResolverProvider {
        fallback: fallback_provider,
        conda_python_identifiers: &conda_python_packages,
        package_requests: package_requests.clone(),
    };

    // We need a new in-memory index for the resolver so that it does not conflict
    // with the build dispatch one. As we have noted in the comment above.
    let resolver_in_memory_index = InMemoryIndex::default();
    let resolution = Resolver::new_custom_io(
        manifest,
        options,
        &context.hash_strategy,
        resolver_env,
        Some(tags),
        &PythonRequirement::from_marker_environment(&marker_environment, requires_python.clone()),
        Conflicts::default(),
        &resolver_in_memory_index,
        context.shared_state.git(),
        &context.capabilities,
        &index_locations,
        provider,
        EmptyInstalledPackages,
    )
    .into_diagnostic()
    .context("failed to resolve pypi dependencies")?
    .with_reporter(UvReporter::new_arc(
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
    let locked_packages = lock_pypi_packages(
        conda_python_packages,
        &lazy_build_dispatch,
        &registry_client,
        resolution,
        &context.capabilities,
        context.concurrency.downloads,
        project_root,
    )
    .await?;

    let conda_task = lazy_build_dispatch.conda_task;

    Ok((locked_packages, conda_task))
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
    #[error("invalid URL")]
    InvalidUrl(#[from] ToUrlError),
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
                        .to_url()?
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
                    UrlOrPath::Path(Utf8TypedPathBuf::from(path.to_string_lossy().to_string()))
                }
                // This happens when it is relative to the non-standard index
                // location on disk.
                FileLocation::RelativeUrl(base, relative) => {
                    // base is a file:// url, but we need to convert it to a path
                    // This is the same logic as the `AbsoluteUrl` case
                    // basically but we just make an absolute path first
                    let base = Url::from_str(base)
                        .map_err(|_| GetUrlOrPathError::InvalidBaseUrl(base.clone()))?;
                    let base = base
                        .to_file_path()
                        .map_err(|_| GetUrlOrPathError::ExpectedPath(base.to_string()))?;

                    let relative = PathBuf::from_str(relative)
                        .map_err(|_| GetUrlOrPathError::ExpectedPath(relative.clone()))?;
                    let absolute = base.join(relative);

                    let relative = absolute.strip_prefix(abs_project_root);
                    let path = match relative {
                        Ok(relative) => PathBuf::from_str(RELATIVE_BASE)
                            .map_err(|_| {
                                GetUrlOrPathError::ExpectedPath(RELATIVE_BASE.to_string())
                            })?
                            .join(relative),
                        Err(_) => absolute,
                    };
                    UrlOrPath::Path(Utf8TypedPathBuf::from(path.to_string_lossy().to_string()))
                }
            };
            Ok(url)
        }
    }
}

/// Create a vector of locked packages from a resolution
async fn lock_pypi_packages(
    conda_python_packages: CondaPythonPackages,
    pixi_build_dispatch: &LazyBuildDispatch<'_>,
    registry_client: &Arc<RegistryClient>,
    resolution: Resolution,
    index_capabilities: &IndexCapabilities,
    concurrent_downloads: usize,
    abs_project_root: &Path,
) -> miette::Result<Vec<(PypiPackageData, PypiPackageEnvironmentData)>> {
    let mut locked_packages = LockedPypiPackages::with_capacity(resolution.len());
    let database =
        DistributionDatabase::new(registry_client, pixi_build_dispatch, concurrent_downloads);
    for dist in resolution.distributions() {
        // If this refers to a conda package we can skip it
        if conda_python_packages.contains_key(dist.name()) {
            continue;
        }

        let pypi_package_data = match dist {
            // Ignore installed distributions
            ResolvedDist::Installed { .. } => {
                continue;
            }

            ResolvedDist::Installable {
                dist: Dist::Built(dist),
                ..
            } => {
                let (location, hash) = match &dist {
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
                        UrlOrPath::Path(Utf8TypedPathBuf::from(
                            process_uv_path_url(&dist.url)
                                .into_diagnostic()?
                                .to_string_lossy()
                                .to_string(),
                        )),
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
                    location,
                    hash,
                }
            }
            ResolvedDist::Installable {
                dist: Dist::Source(source),
                ..
            } => {
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
                let (location, hash, editable) = match source {
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
                    SourceDist::Git(git) => {
                        // convert resolved source dist into a pinned git spec
                        let pinned_git_spec = into_pinned_git_spec(git.clone());
                        (
                            pinned_git_spec.into_locked_git_url().to_url().into(),
                            hash,
                            false,
                        )
                    }
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
                        let url_or_path = UrlOrPath::Path(Utf8TypedPathBuf::from(
                            given_path.to_string_lossy().to_string(),
                        ));
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
                        let url_or_path = UrlOrPath::Path(Utf8TypedPathBuf::from(
                            given_path.to_string_lossy().to_string(),
                        ));
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
                    location,
                    requires_dist: to_requirements(metadata.requires_dist.iter())
                        .into_diagnostic()?,
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
