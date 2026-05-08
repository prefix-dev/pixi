use std::{
    borrow::Cow,
    collections::HashMap,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, LazyLock},
};
use uv_redacted::DisplaySafeUrl;

use dashmap::DashMap;
use itertools::Itertools;
use once_cell::sync::OnceCell;
use pep440_rs::VersionSpecifiers;
use pixi_command_dispatcher::{CommandDispatcher, CommandDispatcherError};
use pixi_git::url::RepositoryUrl;
use pixi_install_pypi::LockedPypiRecord;
use pixi_manifest::{EnvironmentName, FeaturesExt};
use pixi_record::{LockedGitUrl, PixiRecord};
use pixi_spec::Subdirectory;
use pixi_uv_context::UvResolutionContext;
use pixi_uv_conversions::{
    configure_insecure_hosts_for_tls_bypass, into_pixi_reference, pypi_options_to_build_options,
    pypi_options_to_index_locations, to_index_strategy,
};
use pypi_modifiers::pypi_marker_env::determine_marker_environment;
use pypi_modifiers::pypi_tags::{get_pypi_tags, is_python_record};
use rattler_conda_types::{GenericVirtualPackage, Platform};
use rattler_lock::UrlOrPath;
use typed_path::Utf8TypedPathBuf;
use url::Url;
use uv_client::{BaseClientBuilder, Connectivity, FlatIndexClient, RegistryClientBuilder};
use uv_configuration::RAYON_INITIALIZE;
use uv_distribution::DistributionDatabase;
use uv_distribution_types::{ConfigSettings, DependencyMetadata, IndexUrl, RequirementSource};
use uv_git_types::GitReference;
use uv_pypi_types::PyProjectToml;
use uv_resolver::FlatIndex;

use super::errors::PlatformUnsat;
use super::platform::{RequirementOrigin, VerifySatisfiabilityContext};
use super::pypi_metadata;
use crate::{
    lock_file::{
        CondaPrefixUpdater, PixiRecordsByName, PypiRecordsByName,
        outdated::{BuildCacheKey, PypiEnvironmentBuildCache},
        records_by_name::LockedPypiRecordsByName,
        resolve::build_dispatch::{LazyBuildDispatch, UvBuildDispatchParams},
    },
    workspace::{
        Environment, EnvironmentVars, HasWorkspaceRef, grouped_environment::GroupedEnvironment,
    },
};

/// Compare two PyPI index URLs ignoring trailing slashes.
fn pypi_index_urls_match(a: &Url, b: &Url) -> bool {
    a.as_str().trim_end_matches('/') == b.as_str().trim_end_matches('/')
}

/// Check satisfiability of a pypi requirement against a locked pypi package
/// This also does an additional check for git urls when using direct url
/// references
pub(crate) fn pypi_satisfies_editable(
    spec: &uv_distribution_types::Requirement,
    locked_data: &LockedPypiRecord,
    project_root: &Path,
) -> Result<(), Box<PlatformUnsat>> {
    let locked_data = &locked_data.data;
    // We dont match on spec.is_editable() != locked_data.editable
    // as it will happen later in verify_package_platform_satisfiability
    // TODO: could be a potential refactoring opportunity

    match &spec.source {
        RequirementSource::Registry { .. }
        | RequirementSource::Url { .. }
        | RequirementSource::Path { .. }
        | RequirementSource::Git { .. } => {
            unreachable!(
                "editable requirement cannot be from registry, url, git or path (non-directory)"
            )
        }
        RequirementSource::Directory { install_path, .. } => match &**locked_data.location() {
            // If we have an url requirement locked, but the editable is requested, this does not
            // satisfy
            UrlOrPath::Url(url) => Err(Box::new(PlatformUnsat::EditablePackageIsUrl(
                spec.name.clone(),
                url.to_string(),
            ))),
            UrlOrPath::Path(path) => {
                // Most of the times the path will be relative to the project root
                let absolute_path = if path.is_absolute() {
                    Cow::Borrowed(Path::new(path.as_str()))
                } else {
                    Cow::Owned(project_root.join(Path::new(path.as_str())))
                };
                // Absolute paths can have symbolic links, so we canonicalize
                let canonicalized_path = dunce::canonicalize(&absolute_path).map_err(|e| {
                    Box::new(PlatformUnsat::FailedToCanonicalizePath(
                        absolute_path.to_path_buf(),
                        e,
                    ))
                })?;

                if canonicalized_path != install_path.as_ref() {
                    return Err(Box::new(PlatformUnsat::EditablePackagePathMismatch(
                        spec.name.clone(),
                        absolute_path.into_owned(),
                        install_path.to_path_buf(),
                    )));
                }
                Ok(())
            }
        },
    }
}

/// Check satisfiability of a pypi requirement against a locked pypi package.
/// Also does an additional check for git urls when using direct url references.
///
/// `origin` disambiguates an absent `index`: `Manifest` triggers the strict
/// "removed the index" check; `RequiresDist` trusts the lock-file (pep508
/// carries no index info).
///
/// `locked_indexes` are the env-level indexes recorded in the lock-file
/// (already verified against the manifest); a requirement with no
/// per-package `index` is satisfied by any of them. Empty slice falls back
/// to the default PyPI URL (pre-v7 lockfiles).
pub(crate) fn pypi_satisfies_requirement(
    spec: &uv_distribution_types::Requirement,
    locked_record: &LockedPypiRecord,
    project_root: &Path,
    origin: RequirementOrigin,
    locked_indexes: &[Url],
) -> Result<(), Box<PlatformUnsat>> {
    let locked_data = &locked_record.data;
    if spec.name.to_string() != locked_data.name().to_string() {
        return Err(PlatformUnsat::LockedPyPINamesMismatch {
            expected: spec.name.to_string(),
            found: locked_data.name().to_string(),
        }
        .into());
    }

    match &spec.source {
        RequirementSource::Registry {
            specifier, index, ..
        } => {
            let version_string = locked_record.locked_version.to_string();
            if !specifier.contains(
                &uv_pep440::Version::from_str(&version_string).expect("could not parse version"),
            ) {
                return Err(PlatformUnsat::LockedPyPIVersionsMismatch {
                    name: spec.name.clone().to_string(),
                    specifiers: specifier.clone().to_string(),
                    version: version_string.to_owned(),
                }
                .into());
            }

            // Verify the index in the requirement matches the lock-file.
            // Pre-v7 lockfiles don't store per-package index URLs, so
            // index_url is None — skip the comparison in that case.
            match (
                index,
                locked_data.as_wheel().and_then(|w| w.index_url.as_ref()),
            ) {
                (Some(required_index), Some(locked_url)) => {
                    let required_url: Url = required_index.url.url().clone().into();
                    if locked_url != &required_url {
                        return Err(PlatformUnsat::LockedPyPIIndexMismatch {
                            name: spec.name.to_string(),
                            expected_index: required_url.to_string(),
                            locked_index: locked_url.to_string(),
                        }
                        .into());
                    }
                }
                (None, Some(locked_url)) if origin == RequirementOrigin::Manifest => {
                    // Issue #6060: accept the locked URL if it matches any
                    // env-level configured index; fall back to PyPI default.
                    let effective_indexes: &[Url] = if locked_indexes.is_empty() {
                        std::slice::from_ref(&*pixi_consts::consts::DEFAULT_PYPI_INDEX_URL)
                    } else {
                        locked_indexes
                    };
                    let acceptable = effective_indexes
                        .iter()
                        .any(|configured| pypi_index_urls_match(configured, locked_url));
                    if !acceptable {
                        return Err(PlatformUnsat::LockedPyPIIndexMismatch {
                            name: spec.name.to_string(),
                            expected_index: effective_indexes.iter().format(", ").to_string(),
                            locked_index: locked_url.to_string(),
                        }
                        .into());
                    }
                }
                // Either the locked index is missing (pre-v7 lockfile) or the
                // requirement comes from a parent's `requires_dist` (pep508
                // carries no index info, so we trust the lock-file's
                // recorded index).
                (_, None) | (None, _) => {}
            }

            Ok(())
        }
        RequirementSource::Url { url: spec_url, .. } => {
            if let UrlOrPath::Url(locked_url) = &**locked_data.location() {
                // Url may not start with git, and must start with direct+
                if locked_url.as_str().starts_with("git+")
                    || !locked_url.as_str().starts_with("direct+")
                {
                    return Err(PlatformUnsat::LockedPyPIMalformedUrl(locked_url.clone()).into());
                }
                let locked_url = locked_url
                    .as_ref()
                    .strip_prefix("direct+")
                    .and_then(|str| Url::parse(str).ok())
                    .unwrap_or(locked_url.clone());

                if *spec_url.raw() == DisplaySafeUrl::from_url(locked_url.clone()) {
                    return Ok(());
                } else {
                    return Err(PlatformUnsat::LockedPyPIDirectUrlMismatch {
                        name: spec.name.clone().to_string(),
                        spec_url: spec_url.raw().to_string(),
                        lock_url: locked_url.to_string(),
                    }
                    .into());
                }
            }
            Err(PlatformUnsat::LockedPyPIRequiresDirectUrl(spec.name.to_string()).into())
        }
        RequirementSource::Git {
            git, subdirectory, ..
        } => {
            let repository = git.repository();
            let reference = git.reference();
            match &**locked_data.location() {
                UrlOrPath::Url(url) => {
                    if let Ok(pinned_git_spec) = LockedGitUrl::new(url.clone()).to_pinned_git_spec()
                    {
                        let pinned_repository = RepositoryUrl::new(&pinned_git_spec.git);
                        let specified_repository = RepositoryUrl::new(repository);

                        let repo_is_same = pinned_repository == specified_repository;
                        if !repo_is_same {
                            return Err(PlatformUnsat::LockedPyPIGitUrlMismatch {
                                name: spec.name.clone().to_string(),
                                spec_url: repository.to_string(),
                                lock_url: pinned_git_spec.git.to_string(),
                            }
                            .into());
                        }
                        // If the spec uses DefaultBranch, we need to check what the lock has
                        // DefaultBranch in it
                        // otherwise any explicit ref in lock is not satisfiable
                        if *reference == GitReference::DefaultBranch {
                            match &pinned_git_spec.source.reference {
                                // Any explicit reference in lock is not satisfiable
                                // when manifest has DefaultBranch (user removed the explicit ref)
                                pixi_spec::GitReference::Branch(_)
                                | pixi_spec::GitReference::Tag(_)
                                | pixi_spec::GitReference::Rev(_) => {
                                    return Err(PlatformUnsat::LockedPyPIGitRefMismatch {
                                        name: spec.name.clone().to_string(),
                                        expected_ref: reference.to_string(),
                                        found_ref: pinned_git_spec.source.reference.to_string(),
                                    }
                                    .into());
                                }
                                // Only DefaultBranch in lock is satisfiable
                                pixi_spec::GitReference::DefaultBranch => {
                                    return Ok(());
                                }
                            }
                        }

                        // Normalize the input requirement subdirectory the same way we do in our
                        // lock-file. We convert to string to ensure we have a valid fallback if
                        // `Subdirectory` validation fails.
                        let spec_subdir_str = subdirectory
                            .as_deref()
                            .and_then(|s| Subdirectory::normalize(s).ok())
                            .map_or_else(
                                || {
                                    subdirectory
                                        .as_ref()
                                        .map(|s| s.to_string_lossy().to_string())
                                },
                                |s| Some(s.to_string_lossy().to_string()),
                            );
                        let lock_subdir_str =
                            pinned_git_spec.source.subdirectory.to_option_string();
                        if lock_subdir_str != spec_subdir_str {
                            return Err(PlatformUnsat::LockedPyPIGitSubdirectoryMismatch {
                                name: spec.name.clone().to_string(),
                                spec_subdirectory: spec_subdir_str.unwrap_or_default(),
                                lock_subdirectory: lock_subdir_str.unwrap_or_default(),
                            }
                            .into());
                        }
                        // v6 lockfiles encode git deps as
                        //   git+https://repo.git#<sha>
                        // without any ref information — no ?tag=/?branch=/?rev=
                        // query params and no @ref in the URL path. v7 lockfiles
                        // always include the ref as a query param. When the
                        // locked URL carries no ref information the original ref
                        // was not recorded and the commit SHA is the only
                        // authority — skip the ref comparison.
                        let has_ref_in_query = url
                            .query_pairs()
                            .any(|(k, _)| matches!(&*k, "tag" | "branch" | "rev"));
                        let has_ref_in_path = url.path().contains('@');
                        if !has_ref_in_query && !has_ref_in_path {
                            return Ok(());
                        }

                        // If the spec does specify a revision than the revision must match
                        // convert first to the same type
                        let pixi_reference = into_pixi_reference(reference.clone());

                        if pinned_git_spec.source.reference == pixi_reference {
                            return Ok(());
                        } else {
                            return Err(PlatformUnsat::LockedPyPIGitRefMismatch {
                                name: spec.name.clone().to_string(),
                                expected_ref: reference.to_string(),
                                found_ref: pinned_git_spec.source.reference.to_string(),
                            }
                            .into());
                        }
                    }
                    Err(PlatformUnsat::LockedPyPIRequiresGitUrl(
                        spec.name.to_string(),
                        url.to_string(),
                    )
                    .into())
                }
                UrlOrPath::Path(path) => Err(PlatformUnsat::LockedPyPIRequiresGitUrl(
                    spec.name.to_string(),
                    path.to_string(),
                )
                .into()),
            }
        }
        RequirementSource::Path { install_path, .. }
        | RequirementSource::Directory { install_path, .. } => {
            if let UrlOrPath::Path(locked_path) = &**locked_data.location() {
                let install_path =
                    Utf8TypedPathBuf::from(install_path.to_string_lossy().to_string());
                let project_root =
                    Utf8TypedPathBuf::from(project_root.to_string_lossy().to_string());
                // Join relative paths with the project root
                let locked_path = if locked_path.is_absolute() {
                    locked_path.clone()
                } else {
                    project_root.join(locked_path.to_path()).normalize()
                };
                if locked_path.to_path() != install_path {
                    return Err(PlatformUnsat::LockedPyPIPathMismatch {
                        name: spec.name.clone().to_string(),
                        install_path: install_path.to_string(),
                        locked_path: locked_path.to_string(),
                    }
                    .into());
                }
                return Ok(());
            }
            Err(PlatformUnsat::LockedPyPIRequiresPath(spec.name.to_string()).into())
        }
    }
}

// Resolve metadata for all path-based pypi source packages upfront, then
// lock every pypi record with a concrete version.  Wheels already carry
// their version; source packages get it from the source tree metadata.
//
// The metadata is read into `ctx.static_metadata_cache` so later calls to
// `read_local_package_metadata` for the same path return instantly.
#[allow(clippy::result_large_err)]
pub(super) async fn lock_pypi_packages(
    ctx: &VerifySatisfiabilityContext<'_>,
    locked_pixi_records: &PixiRecordsByName,
    unresolved_pypi_environment: &PypiRecordsByName,
    building_pixi_records: Result<PixiRecordsByName, PlatformUnsat>,
) -> Result<LockedPypiRecordsByName, CommandDispatcherError<Box<PlatformUnsat>>> {
    let mut locked_pypi_records: Vec<LockedPypiRecord> =
        Vec::with_capacity(unresolved_pypi_environment.len());
    for record in &unresolved_pypi_environment.records {
        let pkg = record.as_package_data();

        // Only local directories can drift. Git/URL/archive sources are
        // content-pinned by the lock and trusted as-is.
        let metadata = if let UrlOrPath::Path(path) = &**pkg.location() {
            let absolute_path = if path.is_absolute() {
                Cow::Borrowed(Path::new(path.as_str()))
            } else {
                Cow::Owned(ctx.project_root.join(Path::new(path.as_str())))
            };

            if absolute_path.is_dir() {
                // Lock says wheel but path is a directory, needs re-solve.
                if pkg.as_wheel().is_some() {
                    return Err(CommandDispatcherError::Failed(Box::new(
                        PlatformUnsat::DistributionShouldBeSource {
                            name: pkg.name().clone(),
                        },
                    )));
                }

                let uv_ctx = ctx
                    .uv_context
                    .get_or_try_init(|| {
                        UvResolutionContext::from_config(
                            ctx.config,
                            ctx.environment.workspace().client()?.clone(),
                        )
                    })
                    .map_err(|e| {
                        CommandDispatcherError::Failed(Box::new(
                            PlatformUnsat::FailedToReadLocalMetadata(
                                pkg.name().clone(),
                                format!("failed to initialize UV context: {e}"),
                            ),
                        ))
                    })?;

                let build_ctx = BuildMetadataContext {
                    environment: ctx.environment,
                    locked_pixi_records,
                    platform: ctx.platform,
                    project_root: ctx.project_root,
                    uv_context: uv_ctx,
                    project_env_vars: &ctx.project_env_vars,
                    command_dispatcher: ctx.command_dispatcher.clone(),
                    build_caches: ctx.build_caches,
                    building_pixi_records: &building_pixi_records,
                    static_metadata_cache: ctx.static_metadata_cache,
                };

                read_local_package_metadata(&absolute_path, pkg.name(), &build_ctx)
                    .await
                    .map_err(|e| {
                        CommandDispatcherError::Failed(Box::new(
                            PlatformUnsat::FailedToReadLocalMetadata(
                                pkg.name().clone(),
                                format!("failed to read metadata: {e}"),
                            ),
                        ))
                    })?
            } else {
                None
            }
        } else {
            None
        };

        // Determine the version: prefer the wheel version from the lock file,
        // fall back to the version read from the source tree metadata.
        let version = pkg
            .version()
            .cloned()
            .or_else(|| metadata.as_ref().and_then(|m| m.version.clone()))
            .unwrap_or_else(|| pep440_rs::MIN_VERSION.clone());

        locked_pypi_records.push(record.lock(version));
    }

    Ok(LockedPypiRecordsByName::from_iter(
        locked_pypi_records.drain(..),
    ))
}

/// Context for building dynamic metadata for local packages.
struct BuildMetadataContext<'a> {
    environment: &'a Environment<'a>,
    locked_pixi_records: &'a PixiRecordsByName,
    platform: Platform,
    project_root: &'a Path,
    uv_context: &'a UvResolutionContext,
    project_env_vars: &'a HashMap<EnvironmentName, EnvironmentVars>,
    command_dispatcher: CommandDispatcher,
    build_caches: &'a DashMap<BuildCacheKey, Arc<PypiEnvironmentBuildCache>>,
    building_pixi_records: &'a Result<PixiRecordsByName, PlatformUnsat>,
    static_metadata_cache: &'a DashMap<PathBuf, pypi_metadata::LocalPackageMetadata>,
}

/// Statically read metadata for a local directory PyPI package via
/// [`DistributionDatabase::requires_dist`]. Returns `Ok(None)` when uv
/// can't extract statically (dynamic deps, missing or unparsable
/// pyproject), in which case the caller trusts the lock. Result is
/// cached platform-independently in `static_metadata_cache`.
///
/// Building wheel metadata as a fallback would need Python in the host
/// conda prefix, which is not guaranteed under cross-platform
/// satisfiability, so we deliberately don't fall back to a build.
#[allow(clippy::result_large_err)]
async fn read_local_package_metadata(
    directory: &Path,
    package_name: &pep508_rs::PackageName,
    ctx: &BuildMetadataContext<'_>,
) -> Result<Option<pypi_metadata::LocalPackageMetadata>, PlatformUnsat> {
    // Check if we already have static metadata cached for this directory
    if let Some(cached_metadata) = ctx.static_metadata_cache.get(directory) {
        tracing::debug!(package = %package_name, "using cached static metadata");
        return Ok(Some(cached_metadata.value().clone()));
    }

    let pypi_options = ctx.environment.pypi_options();

    // Find the Python interpreter from locked records
    let python_record = ctx
        .locked_pixi_records
        .records
        .iter()
        .find(|r| is_python_record(r))
        .ok_or_else(|| {
            PlatformUnsat::FailedToReadLocalMetadata(
                package_name.clone(),
                "No Python interpreter found in locked packages".to_string(),
            )
        })?;

    // Create marker environment for the target platform
    let marker_environment = determine_marker_environment(ctx.platform, python_record.as_ref())
        .map_err(|e| {
            PlatformUnsat::FailedToReadLocalMetadata(
                package_name.clone(),
                format!("Failed to determine marker environment: {e}"),
            )
        })?;

    let index_strategy = to_index_strategy(pypi_options.index_strategy.as_ref());

    // Get or create cache entry for this environment and host platform
    // We use best_platform() since the build prefix is shared across all target platforms
    let best_platform = ctx.environment.best_platform();
    let cache_key = BuildCacheKey::new(ctx.environment.name().clone(), best_platform);
    let cache = ctx.build_caches.entry(cache_key).or_default().clone();

    let index_locations = pypi_options_to_index_locations(&pypi_options, ctx.project_root)
        .map_err(|e| {
            PlatformUnsat::FailedToReadLocalMetadata(
                package_name.clone(),
                format!("Failed to setup index locations: {e}"),
            )
        })?;

    let build_options = pypi_options_to_build_options(
        &pypi_options.no_build.clone().unwrap_or_default(),
        &pypi_options.no_binary.clone().unwrap_or_default(),
    )
    .map_err(|e| {
        PlatformUnsat::FailedToReadLocalMetadata(
            package_name.clone(),
            format!("Failed to create build options: {e}"),
        )
    })?;

    let dependency_metadata = DependencyMetadata::default();

    // Configure insecure hosts
    let allow_insecure_hosts = configure_insecure_hosts_for_tls_bypass(
        ctx.uv_context.allow_insecure_host.clone(),
        ctx.uv_context.tls_no_verify,
        &index_locations,
    );

    let registry_client = {
        let base_client_builder = BaseClientBuilder::default()
            .allow_insecure_host(allow_insecure_hosts.clone())
            .markers(&marker_environment)
            .keyring(ctx.uv_context.keyring_provider)
            .connectivity(Connectivity::Online)
            .native_tls(ctx.uv_context.use_native_tls)
            .extra_middleware(ctx.uv_context.extra_middleware.clone());

        let mut uv_client_builder =
            RegistryClientBuilder::new(base_client_builder, ctx.uv_context.cache.clone())
                .index_locations(index_locations.clone())
                .index_strategy(index_strategy);

        for p in &ctx.uv_context.proxies {
            uv_client_builder = uv_client_builder.proxy(p.clone())
        }

        Arc::new(uv_client_builder.build())
    };

    // Get tags for this platform (needed for FlatIndex)
    let system_requirements = ctx.environment.system_requirements();
    let tags =
        get_pypi_tags(ctx.platform, &system_requirements, python_record.as_ref()).map_err(|e| {
            PlatformUnsat::FailedToReadLocalMetadata(
                package_name.clone(),
                format!("Failed to determine pypi tags: {e}"),
            )
        })?;

    let flat_index = {
        let flat_index_client = FlatIndexClient::new(
            registry_client.cached_client(),
            Connectivity::Online,
            &ctx.uv_context.cache,
        );
        let flat_index_urls: Vec<&IndexUrl> = index_locations
            .flat_indexes()
            .map(|index| index.url())
            .collect();
        let flat_index_entries = flat_index_client
            .fetch_all(flat_index_urls.into_iter())
            .await
            .map_err(|e| {
                PlatformUnsat::FailedToReadLocalMetadata(
                    package_name.clone(),
                    format!("Failed to fetch flat index entries: {e}"),
                )
            })?;
        FlatIndex::from_entries(
            flat_index_entries,
            Some(&tags),
            &ctx.uv_context.hash_strategy,
            &build_options,
        )
    };

    // Create build dispatch parameters
    let config_settings = ConfigSettings::default();
    let build_params = UvBuildDispatchParams::new(
        &registry_client,
        &ctx.uv_context.cache,
        &index_locations,
        &flat_index,
        &dependency_metadata,
        &config_settings,
        &build_options,
        &ctx.uv_context.hash_strategy,
    )
    .with_index_strategy(index_strategy)
    .with_workspace_cache(ctx.uv_context.workspace_cache.clone())
    .with_shared_state(ctx.uv_context.shared_state.fork())
    .with_no_sources(ctx.uv_context.no_sources.clone())
    .with_concurrency(ctx.uv_context.concurrency);

    // Get or create conda prefix updater for the environment
    // Use best_platform() because we can only install/run Python on the host platform
    let conda_prefix_updater = cache
        .conda_prefix_updater
        .get_or_try_init(|| {
            let prefix_platform = ctx.environment.best_platform();
            let group = GroupedEnvironment::Environment(ctx.environment.clone());
            let virtual_packages = ctx.environment.virtual_packages(prefix_platform);

            // Force the initialization of the rayon thread pool to avoid implicit creation
            // by the uv.
            LazyLock::force(&RAYON_INITIALIZE);

            CondaPrefixUpdater::builder(
                group,
                prefix_platform,
                virtual_packages
                    .into_iter()
                    .map(GenericVirtualPackage::from)
                    .collect(),
                ctx.command_dispatcher.clone(),
            )
            .finish()
            .map_err(|e| {
                PlatformUnsat::FailedToReadLocalMetadata(
                    package_name.clone(),
                    format!("Failed to create conda prefix updater: {e}"),
                )
            })
        })?
        .clone();

    // Use cached lazy build dispatch dependencies
    let last_error = Arc::new(OnceCell::new());
    // Use building_pixi_records (host platform) for installing Python and building,
    // since we can only run binaries on the host platform
    let building_records: miette::Result<Vec<PixiRecord>> = ctx
        .building_pixi_records
        .as_ref()
        .map(|r| r.records.clone())
        .map_err(|e| miette::miette!("{}", e));
    let lazy_build_dispatch = LazyBuildDispatch::new(
        build_params,
        conda_prefix_updater,
        ctx.project_env_vars.clone(),
        ctx.environment.clone(),
        building_records,
        pypi_options.no_build_isolation.clone(),
        &cache.lazy_build_dispatch_deps,
        None,
        false,
        Arc::clone(&last_error),
    );

    // Create distribution database
    let database = DistributionDatabase::new(
        &registry_client,
        &lazy_build_dispatch,
        ctx.uv_context.concurrency.downloads,
    );

    // Missing or unparsable pyproject -> trust the lock.
    let pyproject_path = directory.join("pyproject.toml");
    let Ok(contents) = fs_err::read_to_string(&pyproject_path) else {
        tracing::debug!(package = %package_name, "no readable pyproject.toml");
        return Ok(None);
    };
    let Ok(pyproject_toml) = PyProjectToml::from_toml(&contents) else {
        tracing::debug!(package = %package_name, "pyproject.toml could not be parsed");
        return Ok(None);
    };

    // Read version and requires-python ourselves; uv's types differ so
    // we round-trip via string.
    let project = pyproject_toml.project.as_ref();
    let version = project
        .and_then(|p| p.version.as_ref())
        .and_then(|v| v.to_string().parse::<pep440_rs::Version>().ok());
    let requires_python = project
        .and_then(|p| p.requires_python.as_ref())
        .and_then(|rp| rp.parse::<VersionSpecifiers>().ok());

    // `dynamic` is set when *any* `[project.dynamic]` field is listed,
    // not just dependencies, so we accept the deps regardless.
    let requires_dist = match database.requires_dist(directory, &pyproject_toml).await {
        Ok(Some(rd)) => {
            tracing::debug!(
                package = %package_name,
                dynamic = rd.dynamic,
                "extracted requires_dist statically",
            );
            rd
        }
        // uv: "static doesn't apply", trust lock.
        Ok(None) => {
            tracing::debug!(
                package = %package_name,
                "requires_dist returned None, trusting lock",
            );
            return Ok(None);
        }
        // uv: hard error, source diverged, force re-solve.
        Err(e) => {
            return Err(PlatformUnsat::FailedToReadLocalMetadata(
                package_name.clone(),
                format!("static metadata extraction failed: {e}"),
            ));
        }
    };

    // A round-trip failure here would be a uv bug, not "static doesn't
    // apply", so propagate rather than swallow as `Ok(None)`.
    let requires_dist_vec: Vec<pep508_rs::Requirement> = requires_dist
        .requires_dist
        .iter()
        .map(|req| {
            req.to_string()
                .parse::<pep508_rs::Requirement>()
                .map_err(|e| {
                    PlatformUnsat::FailedToReadLocalMetadata(
                        package_name.clone(),
                        format!("Invalid requirement: {e}"),
                    )
                })
        })
        .collect::<Result<_, _>>()?;

    let metadata = pypi_metadata::LocalPackageMetadata {
        version,
        requires_dist: requires_dist_vec,
        requires_python,
    };

    ctx.static_metadata_cache
        .insert(directory.to_path_buf(), metadata.clone());

    Ok(Some(metadata))
}

#[cfg(test)]
mod tests {
    use std::{
        path::{Path, PathBuf},
        str::FromStr,
    };

    use pep440_rs::Version;
    use pep508_rs;
    use pixi_install_pypi::{LockedPypiRecord, UnresolvedPypiRecord};
    use pixi_manifest::pypi::pypi_options::NoBuild;
    use pixi_pypi_spec::PixiPypiSource;
    use pixi_uv_conversions::pep508_requirement_to_uv_requirement;
    use rattler_lock::{PypiPackageData, UrlOrPath, Verbatim};
    use url::Url;
    use uv_distribution_types::RequirementSource;
    use uv_redacted::DisplaySafeUrl;

    use super::super::PypiNoBuildCheck;
    use super::super::platform::RequirementOrigin;
    use super::pypi_satisfies_requirement;
    use crate::lock_file::tests::{make_source_package_with, make_wheel_package_with};

    /// Lock a `PypiPackageData` into a `LockedPypiRecord` for testing.
    /// Uses the package version for wheels, a dummy version for source packages.
    fn lock_for_test(data: PypiPackageData) -> LockedPypiRecord {
        let version = data
            .version()
            .cloned()
            .unwrap_or_else(|| Version::from_str("42.23").unwrap());
        UnresolvedPypiRecord::from(data).lock(version)
    }

    #[test]
    fn test_pypi_git_check_with_rev() {
        // Mock locked data
        let locked_data = lock_for_test(make_wheel_package_with(
            "mypkg",
            "0.1.0",
            "git+https://github.com/mypkg@rev=29932f3915935d773dc8d52c292cadd81c81071d#29932f3915935d773dc8d52c292cadd81c81071d"
                .parse()
                .expect("failed to parse url"),
            None,
            None,
            vec![],
            None,
        ));
        let spec = pep508_requirement_to_uv_requirement(
            pep508_rs::Requirement::from_str("mypkg @ git+https://github.com/mypkg@2993").unwrap(),
        )
        .unwrap();
        let project_root = PathBuf::from_str("/").unwrap();
        // This will not satisfy because the rev length is different, even being
        // resolved to the same one
        pypi_satisfies_requirement(
            &spec,
            &locked_data,
            &project_root,
            RequirementOrigin::Manifest,
            &[],
        )
        .unwrap_err();

        let locked_data = lock_for_test(make_wheel_package_with(
            "mypkg",
            "0.1.0",
            "git+https://github.com/mypkg.git?rev=29932f3915935d773dc8d52c292cadd81c81071d#29932f3915935d773dc8d52c292cadd81c81071d"
                .parse()
                .expect("failed to parse url"),
            None,
            None,
            vec![],
            None,
        ));
        let spec = pep508_requirement_to_uv_requirement(
            pep508_rs::Requirement::from_str(
                "mypkg @ git+https://github.com/mypkg.git@29932f3915935d773dc8d52c292cadd81c81071d",
            )
            .unwrap(),
        )
        .unwrap();
        let project_root = PathBuf::from_str("/").unwrap();
        // This will satisfy
        pypi_satisfies_requirement(
            &spec,
            &locked_data,
            &project_root,
            RequirementOrigin::Manifest,
            &[],
        )
        .unwrap();
        let non_matching_spec = pep508_requirement_to_uv_requirement(
            pep508_rs::Requirement::from_str("mypkg @ git+https://github.com/mypkg@defgd").unwrap(),
        )
        .unwrap();
        pypi_satisfies_requirement(
            &non_matching_spec,
            &locked_data,
            &project_root,
            RequirementOrigin::Manifest,
            &[],
        )
        .unwrap_err();

        // Removing the rev from the Requirement should NOT satisfy when lock has
        // explicit Rev. This ensures that when a user removes an explicit ref
        // from the manifest, the lock file gets re-resolved.
        let spec_without_rev = pep508_requirement_to_uv_requirement(
            pep508_rs::Requirement::from_str("mypkg @ git+https://github.com/mypkg").unwrap(),
        )
        .unwrap();
        pypi_satisfies_requirement(
            &spec_without_rev,
            &locked_data,
            &project_root,
            RequirementOrigin::Manifest,
            &[],
        )
        .unwrap_err();

        // When lock has DefaultBranch (no explicit ref), removing rev from manifest
        // should satisfy
        // No ?rev= query param, only the fragment with commit hash
        let locked_data_default_branch = lock_for_test(make_wheel_package_with(
            "mypkg",
            "0.1.0",
            "git+https://github.com/mypkg.git#29932f3915935d773dc8d52c292cadd81c81071d"
                .parse()
                .expect("failed to parse url"),
            None,
            None,
            vec![],
            None,
        ));
        pypi_satisfies_requirement(
            &spec_without_rev,
            &locked_data_default_branch,
            &project_root,
            RequirementOrigin::Manifest,
            &[],
        )
        .unwrap();
    }

    /// Reproduces issue #5661: PyPI dependency with full commit hash from a
    /// `pyproject.toml`-style PEP 508 string roundtrips through pixi's manifest
    /// types (PixiPypiSpec) and through `as_uv_req` -- which is the path
    /// actually exercised by the satisfiability check -- and must satisfy a
    /// lockfile entry that pixi just wrote for the same dependency.
    #[test]
    fn test_pypi_git_full_commit_via_as_uv_req() {
        use pixi_pypi_spec::PixiPypiSpec;
        use pixi_uv_conversions::as_uv_req;

        // 1. Parse the pyproject.toml-style PEP 508 string the same way pixi
        //    does when reading the manifest.
        let pep_req = pep508_rs::Requirement::from_str(
            "dacite @ git+https://github.com/konradhalas/dacite.git@9898ccbb783e7e6a35ae165e7deb9fa84edfe21c",
        )
        .unwrap();
        let pixi_spec = PixiPypiSpec::try_from(pep_req).unwrap();

        // 2. Convert into a uv Requirement using the same conversion the
        //    satisfiability check uses for top-level PyPI requirements.
        let project_root = PathBuf::from_str("/").unwrap();
        let uv_req = as_uv_req(&pixi_spec, "dacite", &project_root).unwrap();

        // 3. Build the locked record exactly as pixi writes it via
        //    `into_locked_git_url`: ?rev=<sha>#<sha>.
        let locked_data = lock_for_test(make_wheel_package_with(
            "dacite",
            "1.8.1",
            "git+https://github.com/konradhalas/dacite.git?rev=9898ccbb783e7e6a35ae165e7deb9fa84edfe21c#9898ccbb783e7e6a35ae165e7deb9fa84edfe21c"
                .parse()
                .expect("failed to parse url"),
            None,
            None,
            vec![],
            None,
        ));

        // The manifest spec must satisfy the lockfile entry pixi wrote for
        // the very same dependency.
        pypi_satisfies_requirement(
            &uv_req,
            &locked_data,
            &project_root,
            RequirementOrigin::Manifest,
            &[],
        )
        .unwrap();
    }

    // Do not use unix paths on windows: The path gets normalized to something
    // unix-y, and the lockfile keeps the "pretty" path the user filled in at
    // all times. So on windows the test fails.

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_unix_absolute_path_handling() {
        let locked_data = lock_for_test(make_wheel_package_with(
            "mypkg",
            "0.1.0",
            Verbatim::new(UrlOrPath::Path("/home/username/mypkg.tar.gz".into())),
            None,
            None,
            vec![],
            None,
        ));

        let spec =
            pep508_rs::Requirement::from_str("mypkg @ file:///home/username/mypkg.tar.gz").unwrap();

        let spec = pep508_requirement_to_uv_requirement(spec).unwrap();

        pypi_satisfies_requirement(
            &spec,
            &locked_data,
            Path::new(""),
            RequirementOrigin::Manifest,
            &[],
        )
        .unwrap();
    }

    #[test]
    fn test_windows_absolute_path_handling() {
        let locked_data = lock_for_test(make_wheel_package_with(
            "mypkg",
            "0.1.0",
            Verbatim::new(UrlOrPath::Path("C:\\Users\\username\\mypkg.tar.gz".into())),
            None,
            None,
            vec![],
            None,
        ));

        let spec =
            pep508_rs::Requirement::from_str("mypkg @ file:///C:\\Users\\username\\mypkg.tar.gz")
                .unwrap();

        let spec = pep508_requirement_to_uv_requirement(spec).unwrap();

        pypi_satisfies_requirement(
            &spec,
            &locked_data,
            Path::new(""),
            RequirementOrigin::Manifest,
            &[],
        )
        .unwrap();
    }

    #[test]
    fn pypi_editable_satisfied() {
        let pypi_no_build_check = PypiNoBuildCheck::new(Some(&NoBuild::All));

        pypi_no_build_check
            .check(
                &make_source_package_with(
                    "sdist",
                    UrlOrPath::from_str(".").expect("invalid path").into(),
                    vec![],
                    None,
                )
                .into(),
                Some(&PixiPypiSource::Path {
                    path: PathBuf::from("").into(),
                    editable: Some(true),
                }),
            )
            .expect("check must pass");
    }

    /// Test that `pypi_satisfies_requirement` works correctly when a pypi
    /// package has no version (dynamic version from a source dependency).
    /// Path-based requirements should still satisfy.
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_pypi_satisfies_path_requirement_without_version() {
        let locked_data = lock_for_test(make_source_package_with(
            "dynamic-dep",
            Verbatim::new(UrlOrPath::Path("/home/user/project/dynamic-dep".into())),
            vec![],
            None,
        ));

        let spec = pep508_requirement_to_uv_requirement(
            pep508_rs::Requirement::from_str("dynamic-dep @ file:///home/user/project/dynamic-dep")
                .unwrap(),
        )
        .unwrap();

        // A path-based source dependency without a version should still satisfy
        // a path-based requirement.
        pypi_satisfies_requirement(
            &spec,
            &locked_data,
            Path::new(""),
            RequirementOrigin::Manifest,
            &[],
        )
        .unwrap();
    }

    /// Windows variant of the path-based dynamic version test.
    #[cfg(target_os = "windows")]
    #[test]
    fn test_pypi_satisfies_path_requirement_without_version() {
        let locked_data = lock_for_test(make_source_package_with(
            "dynamic-dep",
            Verbatim::new(UrlOrPath::Path(
                "C:\\Users\\user\\project\\dynamic-dep".into(),
            )),
            vec![],
            None,
        ));

        let spec = pep508_requirement_to_uv_requirement(
            pep508_rs::Requirement::from_str(
                "dynamic-dep @ file:///C:\\Users\\user\\project\\dynamic-dep",
            )
            .unwrap(),
        )
        .unwrap();

        // A path-based source dependency without a version should still satisfy
        // a path-based requirement.
        pypi_satisfies_requirement(
            &spec,
            &locked_data,
            Path::new(""),
            RequirementOrigin::Manifest,
            &[],
        )
        .unwrap();
    }

    /// Test that `pypi_satisfies_requirement` works with a git-based
    /// requirement when the locked package has no version.
    #[test]
    fn test_pypi_satisfies_git_requirement_without_version() {
        let locked_data = lock_for_test(make_source_package_with(
            "mypkg",
            "git+https://github.com/mypkg.git#29932f3915935d773dc8d52c292cadd81c81071d"
                .parse()
                .expect("failed to parse url"),
            vec![],
            None,
        ));

        let spec = pep508_requirement_to_uv_requirement(
            pep508_rs::Requirement::from_str("mypkg @ git+https://github.com/mypkg").unwrap(),
        )
        .unwrap();

        // A git-based source dependency without a version should still satisfy.
        pypi_satisfies_requirement(
            &spec,
            &locked_data,
            Path::new(""),
            RequirementOrigin::Manifest,
            &[],
        )
        .unwrap();
    }

    /// Regression test: removing a PyPI `index` from the manifest should
    /// invalidate the lock-file when the locked package was resolved from that
    /// index.
    ///
    /// Verify that removing an explicit index from a PyPI requirement
    /// invalidates the lock-file entry that was resolved from that index.
    #[test]
    fn test_pypi_index_removed_should_invalidate() {
        // Locked data: package was resolved from a custom index.
        let locked_data = lock_for_test(make_wheel_package_with(
            "my-dep",
            "1.0.0",
            "https://custom.example.com/simple/packages/my_dep-1.0.0-py3-none-any.whl"
                .parse()
                .expect("failed to parse url"),
            None,
            Some(Url::parse("https://custom.example.com/simple").unwrap()),
            vec![],
            None,
        ));

        // Requirement: no index specified (user removed the `index` field).
        let spec = pep508_requirement_to_uv_requirement(
            pep508_rs::Requirement::from_str("my-dep>=1.0").unwrap(),
        )
        .unwrap();

        let project_root = PathBuf::from_str("/").unwrap();

        let result = pypi_satisfies_requirement(
            &spec,
            &locked_data,
            &project_root,
            RequirementOrigin::Manifest,
            &[],
        );
        assert!(
            result.is_err(),
            "expected index removal to invalidate satisfiability, \
             but pypi_satisfies_requirement returned Ok(())"
        );
    }

    /// Regression test for a false-positive index mismatch on transitive
    /// dependencies pulled from a custom index.
    ///
    /// When a package resolved from a custom index has a transitive
    /// `requires_dist` entry, that requirement is materialized from pep508
    /// and therefore carries no `index` info. The locked record for the
    /// transitive dep, however, faithfully records the custom index it was
    /// resolved from. Treating that as a mismatch incorrectly marks the
    /// environment as out-of-date on every subsequent `pixi lock` run.
    #[test]
    fn test_pypi_transitive_custom_index_should_satisfy() {
        // Locked transitive package was resolved from a custom index.
        let locked_data = lock_for_test(make_wheel_package_with(
            "my-dep",
            "1.0.0",
            "https://custom.example.com/simple/packages/my_dep-1.0.0-py3-none-any.whl"
                .parse()
                .expect("failed to parse url"),
            None,
            Some(Url::parse("https://custom.example.com/simple").unwrap()),
            vec![],
            None,
        ));

        // Transitive requirement: parsed from a parent's `requires_dist`.
        // pep508 has no concept of an index, so `index` is always None here.
        let spec = pep508_requirement_to_uv_requirement(
            pep508_rs::Requirement::from_str("my-dep>=1.0,<2.0").unwrap(),
        )
        .unwrap();

        let project_root = PathBuf::from_str("/").unwrap();

        // Direct (non-transitive) check should still flag the mismatch as
        // before, because the user could have removed the `index` from the
        // manifest.
        pypi_satisfies_requirement(
            &spec,
            &locked_data,
            &project_root,
            RequirementOrigin::Manifest,
            &[],
        )
        .expect_err("direct requirement without index must not satisfy custom-index lock");

        // Transitive check must accept the lock-file's recorded index.
        pypi_satisfies_requirement(
            &spec,
            &locked_data,
            &project_root,
            RequirementOrigin::RequiresDist,
            &[],
        )
        .expect("transitive requirement with no pep508 index must satisfy a custom-index lock");
    }

    /// Helper to build a `uv_distribution_types::Requirement` with an explicit index.
    fn registry_requirement_with_index(
        name: &str,
        specifier: &str,
        index_url: &str,
    ) -> uv_distribution_types::Requirement {
        use uv_normalize::PackageName as UvPackageName;
        use uv_pep440::VersionSpecifiers;

        let index =
            uv_distribution_types::IndexMetadata::from(uv_distribution_types::IndexUrl::from(
                uv_pep508::VerbatimUrl::from_url(DisplaySafeUrl::parse(index_url).unwrap()),
            ));
        uv_distribution_types::Requirement {
            name: UvPackageName::from_str(name).unwrap(),
            extras: vec![].into(),
            groups: vec![].into(),
            marker: uv_pep508::MarkerTree::TRUE,
            source: RequirementSource::Registry {
                specifier: VersionSpecifiers::from_str(specifier).unwrap(),
                index: Some(index),
                conflict: None,
            },
            origin: None,
        }
    }

    /// Verify that changing a PyPI index to a different non-default index
    /// invalidates the lock-file.
    #[test]
    fn test_pypi_index_changed_should_invalidate() {
        let locked_data = lock_for_test(make_wheel_package_with(
            "my-dep",
            "1.0.0",
            "https://old-index.example.com/packages/my_dep-1.0.0-py3-none-any.whl"
                .parse()
                .expect("failed to parse url"),
            None,
            Some(Url::parse("https://old-index.example.com/simple").unwrap()),
            vec![],
            None,
        ));

        let spec = registry_requirement_with_index(
            "my-dep",
            ">=1.0",
            "https://new-index.example.com/simple",
        );

        let project_root = PathBuf::from_str("/").unwrap();
        let result = pypi_satisfies_requirement(
            &spec,
            &locked_data,
            &project_root,
            RequirementOrigin::Manifest,
            &[],
        );
        assert!(
            result.is_err(),
            "expected index change to invalidate satisfiability"
        );
    }

    /// Verify that a matching non-default index is considered satisfiable.
    #[test]
    fn test_pypi_index_matching_should_satisfy() {
        let index_url = "https://custom.example.com/simple";
        let locked_data = lock_for_test(make_wheel_package_with(
            "my-dep",
            "1.0.0",
            "https://custom.example.com/packages/my_dep-1.0.0-py3-none-any.whl"
                .parse()
                .expect("failed to parse url"),
            None,
            Some(Url::parse(index_url).unwrap()),
            vec![],
            None,
        ));

        let spec = registry_requirement_with_index("my-dep", ">=1.0", index_url);

        let project_root = PathBuf::from_str("/").unwrap();
        let result = pypi_satisfies_requirement(
            &spec,
            &locked_data,
            &project_root,
            RequirementOrigin::Manifest,
            &[],
        );
        assert!(
            result.is_ok(),
            "expected matching index to satisfy, got: {:?}",
            result.unwrap_err()
        );
    }

    /// Verify that adding an index to a requirement that was locked with the
    /// default index invalidates the lock-file.
    #[test]
    fn test_pypi_index_added_should_invalidate() {
        let locked_data = lock_for_test(make_wheel_package_with(
            "my-dep",
            "1.0.0",
            "https://pypi.org/packages/my_dep-1.0.0-py3-none-any.whl"
                .parse()
                .expect("failed to parse url"),
            None,
            Some(Url::parse("https://pypi.org/simple").unwrap()),
            vec![],
            None,
        ));

        let spec =
            registry_requirement_with_index("my-dep", ">=1.0", "https://custom.example.com/simple");

        let project_root = PathBuf::from_str("/").unwrap();
        let result = pypi_satisfies_requirement(
            &spec,
            &locked_data,
            &project_root,
            RequirementOrigin::Manifest,
            &[],
        );
        assert!(
            result.is_err(),
            "expected adding an index to invalidate satisfiability"
        );
    }

    /// Regression for #6060: a feature-level `index-url` plus a manifest
    /// requirement with no per-package `index` must satisfy a lock-file
    /// recorded against that custom URL.
    #[test]
    fn test_pypi_feature_level_index_should_satisfy() {
        let custom_index = "https://custom.example.com/simple";

        let locked_data = lock_for_test(make_wheel_package_with(
            "my-dep",
            "1.0.0",
            "https://custom.example.com/simple/packages/my_dep-1.0.0-py3-none-any.whl"
                .parse()
                .expect("failed to parse url"),
            None,
            Some(Url::parse(custom_index).unwrap()),
            vec![],
            None,
        ));

        let spec = pep508_requirement_to_uv_requirement(
            pep508_rs::Requirement::from_str("my-dep>=1.0").unwrap(),
        )
        .unwrap();

        let project_root = PathBuf::from_str("/").unwrap();

        let result = pypi_satisfies_requirement(
            &spec,
            &locked_data,
            &project_root,
            RequirementOrigin::Manifest,
            &[Url::parse(custom_index).unwrap()],
        );
        assert!(result.is_ok(), "{:?}", result.unwrap_err());

        // Trailing slash on the manifest-side URL must still match.
        let result_with_trailing_slash = pypi_satisfies_requirement(
            &spec,
            &locked_data,
            &project_root,
            RequirementOrigin::Manifest,
            &[Url::parse(&format!("{custom_index}/")).unwrap()],
        );
        assert!(result_with_trailing_slash.is_ok());

        // An unrelated configured index must still invalidate.
        let result_unrelated = pypi_satisfies_requirement(
            &spec,
            &locked_data,
            &project_root,
            RequirementOrigin::Manifest,
            &[Url::parse("https://unrelated.example.com/simple").unwrap()],
        );
        assert!(result_unrelated.is_err());
    }

    /// A package locked from an `extra-index-urls` entry must satisfy a
    /// manifest requirement with no per-package `index`.
    #[test]
    fn test_pypi_extra_index_should_satisfy() {
        let extra_index = "https://extra.example.com/simple";
        let locked_data = lock_for_test(make_wheel_package_with(
            "my-dep",
            "1.0.0",
            "https://extra.example.com/simple/packages/my_dep-1.0.0-py3-none-any.whl"
                .parse()
                .expect("failed to parse url"),
            None,
            Some(Url::parse(extra_index).unwrap()),
            vec![],
            None,
        ));

        let spec = pep508_requirement_to_uv_requirement(
            pep508_rs::Requirement::from_str("my-dep>=1.0").unwrap(),
        )
        .unwrap();

        let project_root = PathBuf::from_str("/").unwrap();

        let result = pypi_satisfies_requirement(
            &spec,
            &locked_data,
            &project_root,
            RequirementOrigin::Manifest,
            &[
                pixi_consts::consts::DEFAULT_PYPI_INDEX_URL.clone(),
                Url::parse(extra_index).unwrap(),
            ],
        );
        assert!(result.is_ok(), "{:?}", result.unwrap_err());
    }

    /// V6 lockfiles don't store per-package PyPI index URLs, so
    /// `index_url` is `None` after parsing. When the manifest specifies a
    /// per-package `index`, the satisfiability check must not treat the
    /// missing locked index as a mismatch — it is simply absent from the
    /// older format.
    ///
    /// This is a regression test for a bug observed in crater runs where
    /// `pixi install --all` upgraded v6 lockfiles to v7.
    #[test]
    fn test_v6_missing_index_url_should_not_invalidate() {
        let index_url = "https://custom.example.com/simple";

        // Simulate a v6 locked package: resolved from a custom index, but
        // index_url is None because v6 doesn't store it.
        let locked_data = lock_for_test(make_wheel_package_with(
            "my-dep",
            "1.0.0",
            "https://custom.example.com/packages/my_dep-1.0.0-py3-none-any.whl"
                .parse()
                .expect("failed to parse url"),
            None,
            None, // v6: no per-package index_url
            vec![],
            None,
        ));

        let spec = registry_requirement_with_index("my-dep", ">=1.0", index_url);

        let project_root = PathBuf::from_str("/").unwrap();
        let result = pypi_satisfies_requirement(
            &spec,
            &locked_data,
            &project_root,
            RequirementOrigin::Manifest,
            &[],
        );
        assert!(
            result.is_ok(),
            "v6 lockfile with missing index_url should still satisfy a \
             requirement with an explicit index, got: {:?}",
            result.unwrap_err()
        );
    }
}
