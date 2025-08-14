use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    iter::once,
    ops::Deref,
    path::{Path, PathBuf},
    rc::Rc,
    str::FromStr,
    sync::Arc,
};

use chrono::{DateTime, Utc};
use indexmap::{IndexMap, IndexSet};
use indicatif::ProgressBar;
use itertools::{Either, Itertools};
use miette::{Context, IntoDiagnostic};
use pixi_consts::consts;
use pixi_manifest::{EnvironmentName, SystemRequirements, pypi::pypi_options::PypiOptions};
use pixi_pypi_spec::PixiPypiSpec;
use pixi_record::PixiRecord;
use pixi_reporters::{UvReporter, UvReporterOptions};
use pixi_uv_conversions::{
    ConversionError, as_uv_req, convert_uv_requirements_to_pep508, into_pinned_git_spec,
    pypi_options_to_build_options, pypi_options_to_index_locations, to_exclude_newer,
    to_index_strategy, to_normalize, to_requirements, to_uv_normalize, to_uv_version,
    to_version_specifiers,
};
use pypi_modifiers::{
    pypi_marker_env::determine_marker_environment,
    pypi_tags::{get_pypi_tags, is_python_record},
};
use rattler_digest::{Md5, Sha256, parse_digest_from_hex};
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
    IndexUrl, Name, RequirementSource, RequiresPython, Resolution, ResolvedDist, SourceDist,
    ToUrlError,
};
use uv_pypi_types::{Conflicts, HashAlgorithm, HashDigests};
use uv_requirements::LookaheadResolver;
use uv_resolver::{
    AllowedYanks, DefaultResolverProvider, FlatIndex, InMemoryIndex, Manifest, Options, Preference,
    PreferenceError, Preferences, PythonRequirement, ResolveError, Resolver, ResolverEnvironment,
};
use uv_types::EmptyInstalledPackages;

use crate::{
    environment::CondaPrefixUpdated,
    lock_file::{
        CondaPrefixUpdater, LockedPypiPackages, PixiRecordsByName, PypiPackageIdentifier,
        PypiRecord, UvResolutionContext,
        records_by_name::HasNameVersion,
        resolve::{
            build_dispatch::{
                LazyBuildDispatch, LazyBuildDispatchDependencies, UvBuildDispatchParams,
            },
            resolver_provider::CondaResolverProvider,
        },
    },
    workspace::{Environment, EnvironmentVars},
};

#[derive(Debug, thiserror::Error)]
#[error("Invalid hash: {0} type: {1}")]
struct InvalidHash(String, String);

fn parse_hashes_from_hash_vec(hashes: &HashDigests) -> Result<Option<PackageHashes>, InvalidHash> {
    let mut sha256 = None;
    let mut md5 = None;

    for hash in hashes.iter() {
        match hash.algorithm() {
            HashAlgorithm::Sha256 => {
                sha256 = Some(hash.digest.to_string());
            }
            HashAlgorithm::Md5 => {
                md5 = Some(hash.digest.to_string());
            }
            HashAlgorithm::Sha384 | HashAlgorithm::Sha512 | HashAlgorithm::Blake2b => {
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
    #[error("cannot make {} relative to {}", .0, .1)]
    CannotMakeRelative(String, String),
    #[error("path is not UTF-8: {}", .0.display())]
    NotUtf8(PathBuf),
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
/// We try to create a relative path from the install path to the lock file
/// path. And we try to keep the path absolute if so specified, by the users.
/// So that's assuming it was `given` in that sense.
fn process_uv_path_url(
    path_url: &uv_pep508::VerbatimUrl,
    install_path: &Path,
    project_root: &Path,
) -> Result<Utf8TypedPathBuf, ProcessPathUrlError> {
    let given = path_url
        .given()
        .ok_or_else(|| ProcessPathUrlError::NoGivenPath(path_url.to_string()))?;
    let keep_abs = if given.starts_with("file://") {
        // Processed by UV this is a file url
        // don't keep it absolute as the origin is 1) and relative paths are impossible
        // we are assuming the intention was to keep it relative
        false
    } else {
        let path = PathBuf::from(given);
        // Determine if the path was given as an absolute path
        path.is_absolute()
    };

    let std_path = if !keep_abs {
        // Find the path relative to the project root
        let path = pathdiff::diff_paths(install_path, project_root).ok_or_else(|| {
            ProcessPathUrlError::CannotMakeRelative(
                install_path.to_string_lossy().to_string(),
                project_root.to_string_lossy().to_string(),
            )
        })?;

        // We used to lock with ./ before changes where made so let's add it back
        // if we are not moving down in the directory structure
        if !path.starts_with("..") {
            PathBuf::from(".").join(&path)
        } else {
            path
        }
    } else {
        // Keep the path absolute if it is provided so by the user
        PathBuf::from(install_path)
    };

    let Some(path_str) = std_path.to_str() else {
        return Err(ProcessPathUrlError::NotUtf8(std_path));
    };

    Ok(if cfg!(windows) {
        // Replace backslashes with forward slashes on Windows because pathdiff can
        // return paths with backslashes.
        Utf8TypedPathBuf::from(path_str.replace("\\", "/"))
    } else {
        Utf8TypedPathBuf::from(path_str)
    })
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

#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum SolveError {
    #[error("failed to resolve pypi dependencies")]
    NoSolution {
        source: Box<uv_resolver::NoSolutionError>,
        #[help]
        advice: Option<String>,
    },
    #[error("failed to resolve pypi dependencies")]
    Other(#[from] ResolveError),
}

/// Creates a custom `SolveError` from a `ResolveError`.
/// to add some extra information about locked conda packages
fn create_solve_error(
    error: ResolveError,
    conda_python_packages: &CondaPythonPackages,
) -> SolveError {
    match error {
        ResolveError::NoSolution(no_solution) => {
            let packages: HashSet<_> = no_solution.packages().collect();
            let conflicting_packages: Vec<String> = conda_python_packages
                .iter()
                .filter_map(|(pypi_name, (_, pypi_identifier))| {
                    if packages.contains(pypi_name) {
                        Some(format!(
                            "{}=={}",
                            pypi_identifier.name.as_source(),
                            pypi_identifier.version
                        ))
                    } else {
                        None
                    }
                })
                .collect();

            let advice = if conflicting_packages.is_empty() {
                None
            } else {
                Some(format!(
                    "The following PyPI packages have been pinned by the conda solve, and this version may be causing a conflict:\n{}",
                    conflicting_packages.join("\n")
                ))
            };

            SolveError::NoSolution {
                source: no_solution,
                advice,
            }
        }
        _ => SolveError::Other(error),
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn resolve_pypi(
    context: UvResolutionContext,
    pypi_options: &PypiOptions,
    dependencies: IndexMap<uv_normalize::PackageName, IndexSet<PixiPypiSpec>>,
    system_requirements: SystemRequirements,
    locked_pixi_records: &[PixiRecord],
    locked_pypi_packages: &[PypiRecord],
    platform: rattler_conda_types::Platform,
    pb: &ProgressBar,
    project_root: &Path,
    prefix_updater: CondaPrefixUpdater,
    repodata_building_records: miette::Result<Arc<PixiRecordsByName>>,
    project_env_vars: HashMap<EnvironmentName, EnvironmentVars>,
    environment_name: Environment<'_>,
    disallow_install_conda_prefix: bool,
    exclude_newer: Option<DateTime<Utc>>,
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
                uv_normalize::PackageName::from_str(p.name.as_normalized().as_ref())?,
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

    // Determine the python interpreter that is installed as part of the conda
    // packages.
    let python_record = locked_pixi_records
        .iter()
        .find(|r| match r {
            PixiRecord::Binary(r) => is_python_record(r),
            _ => false,
        })
        .ok_or_else(|| {
            miette::miette!(
                help = format!("Try: {}", consts::TASK_STYLE.apply_to("pixi add python")),
                "No Python interpreter found in the dependencies"
            )
        })?;

    // Construct the marker environment for the target platform
    let marker_environment = determine_marker_environment(platform, python_record.as_ref())?;

    // Determine the tags for this particular solve.
    let tags = get_pypi_tags(platform, &system_requirements, python_record.as_ref())?;

    // We need to setup both an interpreter and a requires_python specifier.
    // The interpreter is used to (potentially) build the wheel, and the
    // requires_python specifier is used to determine the python version of the
    // wheel. So make sure the interpreter does not touch the solve parts of
    // this function
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
    let requires_python =
        RequiresPython::from_specifiers(&uv_pep440::VersionSpecifiers::from(python_specifier));
    tracing::debug!(
        "using requires-python specifier (this may differ from the above): {}",
        requires_python
    );

    let index_locations =
        pypi_options_to_index_locations(pypi_options, project_root).into_diagnostic()?;

    // TODO: create a cached registry client per index_url set?
    let index_strategy = to_index_strategy(pypi_options.index_strategy.as_ref());
    let mut uv_client_builder = RegistryClientBuilder::new(context.cache.clone())
        .allow_insecure_host(context.allow_insecure_host.clone())
        .index_locations(&index_locations)
        .index_strategy(index_strategy)
        .markers(&marker_environment)
        .keyring(context.keyring_provider)
        .connectivity(Connectivity::Online)
        .extra_middleware(context.extra_middleware.clone());

    for p in &context.proxies {
        uv_client_builder = uv_client_builder.proxy(p.clone())
    }

    let registry_client = Arc::new(uv_client_builder.build());

    let build_options = pypi_options_to_build_options(
        &pypi_options.no_build.clone().unwrap_or_default(),
        &pypi_options.no_binary.clone().unwrap_or_default(),
    )
    .into_diagnostic()?;
    let dependency_overrides =
        pypi_options.dependency_overrides.as_ref().map(|overrides|->Result<Vec<_>, _> {
            overrides
                .iter()
                .map(|(name, spec)| {
                    as_uv_req(spec,name.as_normalized().as_ref(), project_root)
                    .into_diagnostic()
                    .with_context(||{
                        format!(
                            "dependency override {name}:{spec:?} should able to convert to uv requirement",
                            name = name.as_source(),
                            spec = spec.to_string()
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()
        }).transpose()?.unwrap_or_default();

    let overrides = Overrides::from_requirements(dependency_overrides);

    // Resolve the flat indexes from `--find-links`.
    // In UV 0.7.8, we need to fetch flat index entries from the index locations
    let flat_index_client = FlatIndexClient::new(
        registry_client.cached_client(),
        Connectivity::Online,
        &context.cache,
    );
    let flat_index_urls: Vec<&IndexUrl> = index_locations
        .flat_indexes()
        .map(|index| index.url())
        .collect();
    let flat_index_entries = flat_index_client
        .fetch_all(flat_index_urls.into_iter())
        .await
        .into_diagnostic()?;
    let flat_index = FlatIndex::from_entries(
        flat_index_entries,
        Some(&tags),
        &context.hash_strategy,
        &build_options,
    );

    // Hi maintainers! For anyone coming here, if you expose any additional `uv`
    // options, similar to `index_strategy`, make sure to include them in this
    // struct as well instead of relying on the default. Otherwise there be
    // panics.
    let options = Options {
        index_strategy,
        build_options: build_options.clone(),
        exclude_newer: exclude_newer.map(to_exclude_newer).unwrap_or_default(),
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
    .with_exclude_newer(options.exclude_newer.clone())
    .with_workspace_cache(context.workspace_cache.clone())
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
        repodata_building_records.map(|r| r.records.clone()),
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

            Ok::<_, ConversionError>(uv_distribution_types::Requirement {
                name: to_uv_normalize(p.name.as_normalized())?,
                extras: vec![].into(),
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
                extras: Vec::new().into(),
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
        &overrides,
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
        overrides,
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
        options.exclude_newer.clone(),
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
        &marker_environment,
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
    .map_err(|e| create_solve_error(e, &conda_python_packages))?;
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
                        .map_err(|_| GetUrlOrPathError::InvalidBaseUrl(base.to_string()))?;
                    let url = base.join(relative).map_err(|_| {
                        GetUrlOrPathError::CannotJoin(base.to_string(), relative.to_string())
                    })?;
                    UrlOrPath::Url(url)
                }
            };
            Ok(url)
        }
        // From observation this is the case where the index is a `--find-links` path
        // i.e a path to a directory. This is not a PyPI index, but a directory or a file with links
        // to wheels
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
                    // not the path returned by the uv solver. Why? Because we need the path
                    // relative to the project root, **not** the path relative
                    // to the --find-links path. This is because during
                    // installation we do something like: `project_root.join(relative_path)`
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
                        .map_err(|_| GetUrlOrPathError::InvalidBaseUrl(base.to_string()))?;
                    let base = base
                        .to_file_path()
                        .map_err(|_| GetUrlOrPathError::ExpectedPath(base.to_string()))?;

                    let relative = PathBuf::from_str(relative)
                        .map_err(|_| GetUrlOrPathError::ExpectedPath(relative.to_string()))?;
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

            ResolvedDist::Installable { dist, .. } => match &**dist {
                Dist::Built(dist) => {
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
                            UrlOrPath::Path(
                                process_uv_path_url(
                                    &dist.url,
                                    &dist.install_path,
                                    abs_project_root,
                                )
                                .into_diagnostic()?,
                            ),
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
                        requires_dist: convert_uv_requirements_to_pep508(
                            metadata.requires_dist.iter(),
                        )
                        .into_diagnostic()?,
                        editable: false,
                        location,
                        hash,
                    }
                }
                Dist::Source(source) => {
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
                        .get_or_build_wheel_metadata(
                            &Dist::Source(source.clone()),
                            HashPolicy::None,
                        )
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
                            let install_path = process_uv_path_url(
                                &path.url,
                                &path.install_path,
                                abs_project_root,
                            )
                            .into_diagnostic()?;

                            // Create the url for the lock file. This is based on the passed in URL
                            // instead of from the source path to copy the path that was passed in
                            // from the requirement.
                            let url_or_path = UrlOrPath::Path(install_path);
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
                            let install_path =
                                process_uv_path_url(&dir.url, &dir.install_path, abs_project_root)
                                    .into_diagnostic()?;

                            // Create the url for the lock file. This is based on the passed in URL
                            // instead of from the source path to copy the path that was passed in
                            // from the requirement.
                            let url_or_path = UrlOrPath::Path(install_path);
                            (url_or_path, hash, dir.editable.unwrap_or(false))
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
            },
        };

        // TODO: Store extras in the lock-file
        locked_packages.push((pypi_package_data, PypiPackageEnvironmentData::default()));
    }

    Ok(locked_packages)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    // In this case we want to make the path relative to the project_root or lock
    // file path
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn process_uv_path_relative_path() {
        let url = uv_pep508::VerbatimUrl::parse_url("file:///a/b/c")
            .unwrap()
            .with_given("./b/c");
        let path =
            process_uv_path_url(&url, &PathBuf::from("/a/b/c"), &PathBuf::from("/a")).unwrap();
        assert_eq!(path.as_str(), "./b/c");
    }

    // In this case we want to make the path relative to the project_root or lock
    // file path
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn process_uv_path_project_root_subdir() {
        let url = uv_pep508::VerbatimUrl::parse_url("file:///a/b/c")
            .unwrap()
            .with_given("./b/c");
        let path =
            process_uv_path_url(&url, &PathBuf::from("/a/c/z"), &PathBuf::from("/a/b/f")).unwrap();
        assert_eq!(path.as_str(), "../../c/z");
    }

    // In this case we want to make the path relative to the project_root or lock
    // file path
    #[cfg(target_os = "windows")]
    #[test]
    fn process_uv_path_relative_path() {
        let url = uv_pep508::VerbatimUrl::parse_url("file://C/a/b/c")
            .unwrap()
            .with_given("./b/c");
        let path =
            process_uv_path_url(&url, &PathBuf::from("C:\\a\\b\\c"), &PathBuf::from("C:\\a"))
                .unwrap();
        assert_eq!(path.as_str(), "./b/c");
    }

    // In this case we want to make the path relative to the project_root or lock
    // file path
    #[cfg(target_os = "windows")]
    #[test]
    fn process_uv_path_project_root_subdir() {
        let url = uv_pep508::VerbatimUrl::parse_url("file://C/a/b/c")
            .unwrap()
            .with_given("./b/c");
        let path = process_uv_path_url(
            &url,
            &PathBuf::from("C:\\a\\c\\z"),
            &PathBuf::from("C:\\a\\b\\f"),
        )
        .unwrap();
        assert_eq!(path.as_str(), "../../c/z");
    }

    // In this case we want to keep the absolute path
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn process_uv_path_absolute_path() {
        let url = uv_pep508::VerbatimUrl::parse_url("file:///a/b/c")
            .unwrap()
            .with_given("/a/b/c");
        let path =
            process_uv_path_url(&url, &PathBuf::from("/a/b/c"), &PathBuf::from("/a")).unwrap();
        assert_eq!(path.as_str(), "/a/b/c");
    }

    // In this case we want to keep the absolute path
    #[cfg(target_os = "windows")]
    #[test]
    fn process_uv_path_absolute_path() {
        let url = uv_pep508::VerbatimUrl::parse_url("file://C/a/b/c")
            .unwrap()
            .with_given("C:\\a\\b\\c");
        let path =
            process_uv_path_url(&url, &PathBuf::from("C:\\a\\b\\c"), &PathBuf::from("C:\\a"))
                .unwrap();
        assert_eq!(path.as_str(), "C:/a/b/c");
    }
}
