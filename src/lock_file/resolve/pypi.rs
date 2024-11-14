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
    InstalledDist, InstalledRegistryDist, Name, Resolution, ResolvedDist, SourceDist,
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
fn process_uv_path_url(path_url: &uv_pep508::VerbatimUrl) -> PathBuf {
    let given = path_url.given().expect("path should have a given url");
    if given.starts_with("file://") {
        path_url
            .to_file_path()
            .expect("path should be a valid file path")
    } else {
        PathBuf::from(given)
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
            (
                uv_normalize::PackageName::new(p.name.as_normalized().to_string())
                    .expect("cannot convert to package name"),
                (record.clone(), p),
            )
        })
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
    // So make sure the interpreter does not touch the solve partgs of this function
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
    )
    .await
}

/// Create a vector of locked packages from a resolution
async fn lock_pypi_packages<'a>(
    conda_python_packages: CondaPythonPackages,
    build_dispatch: &BuildDispatch<'a>,
    registry_client: &Arc<RegistryClient>,
    resolution: Resolution,
    index_capabilities: &IndexCapabilities,
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
                        let best_wheel = dist.best_wheel();
                        let url = match &best_wheel.file.url {
                            FileLocation::AbsoluteUrl(url) => UrlOrPath::Url(
                                Url::from_str(url.as_ref()).expect("invalid absolute url"),
                            ),
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
                    .wheel_metadata(dist, index_capabilities)
                    .await
                    .expect("failed to get wheel metadata");
                PypiPackageData {
                    name: pep508_rs::PackageName::new(metadata.name.to_string())
                        .expect("cannot convert name"),
                    version: pep440_rs::Version::from_str(&metadata.version.to_string())
                        .expect("cannot convert version"),
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
                            FileLocation::AbsoluteUrl(url) => UrlOrPath::Url(
                                Url::from_str(url.as_ref()).expect("invalid absolute url"),
                            ),
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
                        let given_path = process_uv_path_url(&path.url);

                        // Create the url for the lock file. This is based on the passed in URL
                        // instead of from the source path to copy the path that was passed in from
                        // the requirement.
                        let url_or_path = UrlOrPath::Path(given_path);
                        (url_or_path, hash, false)
                    }
                    SourceDist::Directory(dir) => {
                        // TODO: check that `install_path` is correct
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
                        let given_path = process_uv_path_url(&dir.url);

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
                        .expect("cannot convert version"),
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
