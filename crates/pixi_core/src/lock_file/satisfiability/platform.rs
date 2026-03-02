use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};

use dashmap::DashMap;
use futures::TryStreamExt;
use itertools::{Either, Itertools};
use once_cell::sync::OnceCell;
use pixi_command_dispatcher::{
    BuildBackendMetadataSpec, CommandDispatcher, CommandDispatcherError,
    CommandDispatcherErrorResultExt, ComputeResultExt, DevSourceMetadataSpec, EnvironmentRef,
    WorkspaceEnvRef, executor::CancellationAwareFutures, source_checkout::SourceCheckoutExt,
};
use pixi_config::Config;
use pixi_install_pypi::UnresolvedPypiRecord;
use pixi_manifest::{EnvironmentName, FeaturesExt};
use pixi_record::{
    DevSourceRecord, LockFileResolver, PixiRecord, SourceRecordData, UnresolvedPixiRecord,
};
use pixi_spec::{PixiSpec, SourceAnchor, SourceLocationSpec, SourceSpec, SpecConversionError};
use pixi_uv_context::UvResolutionContext;
use pixi_uv_conversions::{
    as_uv_req, pep508_requirement_to_uv_requirement, to_normalize, to_uv_specifiers, to_uv_version,
};
use pypi_modifiers::pypi_marker_env::determine_marker_environment;
use rattler_conda_types::{
    GenericVirtualPackage, MatchSpec, Matches, PackageName, ParseChannelError, ParseMatchSpecError,
    ParseStrictness::Lenient, Platform,
};
use rattler_lock::{LockedPackage, UrlOrPath};
use uv_distribution_types::{RequirementSource, RequiresPython};

use super::errors::{LocalMetadataMismatch, PlatformUnsat, SolveGroupUnsat};
use super::legacy;
use super::pypi::{lock_pypi_packages, pypi_satisfies_editable, pypi_satisfies_requirement};
use super::pypi_metadata;
use super::source_record::{
    verify_build_source_matches_manifest, verify_partial_source_record_against_backend,
};
use crate::{
    lock_file::{
        PixiRecordsByName, PypiRecordsByName,
        outdated::{BuildCacheKey, PypiEnvironmentBuildCache},
        package_identifier::ConversionError,
        records_by_name::{HasNameVersion, LockedPypiRecordsByName},
    },
    workspace::{Environment, EnvironmentVars},
};

/// Context for verifying platform satisfiability.
pub struct VerifySatisfiabilityContext<'a> {
    pub environment: &'a Environment<'a>,
    pub command_dispatcher: CommandDispatcher,
    pub platform: Platform,
    pub project_root: &'a Path,
    pub uv_context: &'a OnceCell<UvResolutionContext>,
    pub config: &'a Config,
    pub project_env_vars: HashMap<EnvironmentName, EnvironmentVars>,
    pub build_caches: &'a DashMap<BuildCacheKey, Arc<PypiEnvironmentBuildCache>>,
    /// Cache for static metadata extracted from pyproject.toml files.
    /// This is shared across platforms since static metadata is platform-independent.
    pub static_metadata_cache: &'a DashMap<PathBuf, pypi_metadata::LocalPackageMetadata>,
    /// Resolver for the lock-file being verified. Built once at the top of
    /// `find_unsatisfiable_targets` and shared across all per-platform
    /// contexts.
    pub resolver: &'a LockFileResolver,
}

pub type PlatformSatisfiabilityResult = Result<
    (VerifiedIndividualEnvironment, LockedPypiRecordsByName),
    CommandDispatcherError<Box<PlatformUnsat>>,
>;

fn build_platform_verification_setup(
    ctx: &VerifySatisfiabilityContext<'_>,
) -> Result<
    crate::lock_file::platform_setup::PlatformSetup,
    CommandDispatcherError<Box<PlatformUnsat>>,
> {
    use crate::lock_file::platform_setup::{PlatformSetupError, build_platform_setup};
    build_platform_setup(ctx.environment, ctx.platform, &ctx.command_dispatcher).map_err(|err| {
        match err {
            PlatformSetupError::InvalidChannel(e) => {
                CommandDispatcherError::Failed(Box::new(PlatformUnsat::InvalidChannel(e)))
            }
            PlatformSetupError::Variants(e) => {
                CommandDispatcherError::Failed(Box::new(PlatformUnsat::Variants(e)))
            }
        }
    })
}

/// Verifies that the package requirements of the specified `environment` can be
/// satisfied with the packages present in the lock-file.
///
/// Both Conda and pypi packages are verified by this function. First all the
/// conda package are verified and then all the pypi packages are verified. This
/// is done so that if we can check if we only need to update the pypi
/// dependencies or also the conda dependencies.
///
/// This function returns a [`PlatformUnsat`] error if a verification issue
/// occurred. The [`PlatformUnsat`] error should contain enough information for
/// the user and developer to figure out what went wrong.
///
#[allow(clippy::result_large_err)]
pub async fn verify_platform_satisfiability(
    ctx: &VerifySatisfiabilityContext<'_>,
    locked_environment: rattler_lock::Environment<'_>,
) -> PlatformSatisfiabilityResult {
    let platform_setup = match build_platform_verification_setup(ctx) {
        Ok(setup) => setup,
        Err(err) => {
            return Err(err);
        }
    };

    // Convert the lock file into a list of conda and pypi packages.
    // Read as UnresolvedPixiRecord first, then resolve any partial source records.
    let mut unresolved_records: Vec<UnresolvedPixiRecord> = Vec::new();
    let mut pypi_packages: Vec<UnresolvedPypiRecord> = Vec::new();
    let resolver = ctx.resolver;
    let lock_platform = locked_environment
        .lock_file()
        .platform(&ctx.platform.to_string());
    for package in lock_platform
        .and_then(|p| locked_environment.packages(p))
        .into_iter()
        .flatten()
    {
        match package {
            LockedPackage::Conda(_) => {
                let record = resolver
                    .get_for_package(package)
                    .expect("conda package from lock file not found in resolver");
                unresolved_records.push(record);
            }
            LockedPackage::Pypi(pypi) => {
                pypi_packages.push(pypi.clone().into());
            }
        }
    }

    // Pre-v7 lock files don't store the resolved build/host
    // environments of source records, so the records arrive here with
    // empty `build_packages` / `host_packages`. Reify them by
    // recomputing from the build backend before the verify loop runs;
    // afterwards the v7 path treats them identically. v7+ lock files
    // skip this entirely.
    legacy::reify_legacy_source_envs(
        &ctx.command_dispatcher,
        &mut unresolved_records,
        locked_environment.lock_file().version(),
        &platform_setup.workspace_env_ref,
    )
    .await
    .map_err(|err| match err {
        CommandDispatcherError::Cancelled => CommandDispatcherError::Cancelled,
        CommandDispatcherError::Failed(err) => CommandDispatcherError::Failed(Box::new(
            PlatformUnsat::LegacySourceEnvReify(err.to_string()),
        )),
    })?;

    // Resolve every unresolved source record into a fully-resolved
    // [`PixiRecord::Source`].
    //
    // Immutable + full records (e.g. git pins with a stored
    // `PackageRecord`) downcast directly. Mutable or partial records
    // (e.g. local paths, or a stored partial entry whose full metadata
    // would have been stale) are verified against fresh backend
    // metadata: we ask the build backend for the source's outputs,
    // pick the output matching the locked variants, and check that
    // every backend-declared build/host spec is satisfied by the
    // record's locked `build_packages` / `host_packages`. PyPI-style
    // dependencies in build/host environments are rejected. On
    // success, we synthesize a fresh `FullSourceRecord` from the
    // backend output's metadata while keeping the locked build/host
    // package sets, so downstream verification still operates against
    // the same env the solver previously chose. Failures emit a
    // specific [`PlatformUnsat`] variant carrying the offending spec
    // so re-locking is forced with informative diagnostics.
    let mut resolved_records = Vec::new();
    for record in unresolved_records {
        match record {
            UnresolvedPixiRecord::Binary(record) => {
                resolved_records.push(PixiRecord::Binary(record))
            }
            UnresolvedPixiRecord::Source(record) => {
                let needs_backend_check = record.data.is_partial() || record.has_mutable_source();
                if needs_backend_check {
                    // Partial records carry no version/build material in
                    // the lockfile, so they must be resolved from the
                    // backend. Mutable sources (path-based, or with a
                    // path-based build source) must also re-evaluate via
                    // the backend because the manifest can change without
                    // any lockfile-visible signal — there is no
                    // content-pinned identifier we can use to detect
                    // edits to e.g. host-dependencies. Skipping the
                    // backend here would silently accept stale lockfiles.
                    let resolved =
                        verify_partial_source_record_against_backend(ctx, &platform_setup, &record)
                            .await?;
                    resolved_records.push(PixiRecord::Source(resolved));
                } else {
                    // Fully immutable + full record: the source is
                    // content-pinned (git commit / url+sha), so the
                    // backend cannot tell us anything we can't already
                    // read off the locked record. Trust the locked
                    // metadata as-is and avoid contacting the backend
                    // (which would otherwise require it to be available
                    // just to pass satisfiability).
                    let full_record =
                        Arc::unwrap_or_clone(record).try_map_data(|data| match data {
                            SourceRecordData::Full(data) => Ok(data),
                            SourceRecordData::Partial(p) => Err(p),
                        });
                    match full_record {
                        Ok(full) => resolved_records.push(PixiRecord::Source(Arc::new(full))),
                        Err(_) => {
                            unreachable!("guarded by `data.is_partial()` check above")
                        }
                    }
                }
            }
        }
    }

    // Create a lookup table from package name to package record. Returns an error
    // if we find a duplicate entry for a record
    let package_verification_future = async {
        // to reflect new purls for pypi packages
        // we need to invalidate the locked environment
        // if all conda packages have empty purls
        if ctx.environment.has_pypi_dependencies()
            && pypi_packages.is_empty()
            && resolved_records
                .iter()
                .filter_map(PixiRecord::as_binary)
                .all(|record| record.package_record.purls.is_none())
        {
            return Err(CommandDispatcherError::Failed(Box::new(
                PlatformUnsat::MissingPurls,
            )));
        }

        let pixi_records_by_name = PixiRecordsByName::from_unique_iter(resolved_records.clone())
            .map_err(|duplicate| {
                CommandDispatcherError::Failed(Box::new(PlatformUnsat::DuplicateEntry(
                    duplicate.package_record().name.as_source().to_string(),
                )))
            })?;

        // Create a lookup table from package name to package record. Returns an error
        // if we find a duplicate entry for a record
        let pypi_records_by_name =
            PypiRecordsByName::from_unique_iter(pypi_packages).map_err(|duplicate| {
                CommandDispatcherError::Failed(Box::new(PlatformUnsat::DuplicateEntry(
                    duplicate.name().to_string(),
                )))
            })?;

        // Get host platform records for building (we can only run Python on the host platform)
        let best_platform = ctx.environment.best_platform();
        let building_pixi_records = if ctx.platform == best_platform {
            // Same platform, reuse the records
            Ok(pixi_records_by_name.clone())
        } else {
            // Different platform - extract host platform records for building
            let mut host_pixi_records: Vec<PixiRecord> = Vec::new();
            let lock_best_platform = locked_environment
                .lock_file()
                .platform(&best_platform.to_string());
            for package in lock_best_platform
                .and_then(|p| locked_environment.packages(p))
                .into_iter()
                .flatten()
            {
                if let LockedPackage::Conda(_) = package {
                    let record = resolver
                        .get_for_package(package)
                        .expect("conda package from lock file not found in resolver");
                    // Partial source records (e.g. packages that haven't
                    // been built yet) cannot be resolved. Skip them — only
                    // fully resolved records are needed as build
                    // dependencies for UV metadata builds.
                    if let Ok(resolved) = record.try_into_resolved() {
                        host_pixi_records.push(resolved);
                    }
                }
            }
            PixiRecordsByName::from_unique_iter(host_pixi_records).map_err(|duplicate| {
                PlatformUnsat::DuplicateEntry(
                    duplicate.package_record().name.as_source().to_string(),
                )
            })
        };

        verify_package_platform_satisfiability(
            ctx,
            &platform_setup,
            &pixi_records_by_name,
            &pypi_records_by_name,
            building_pixi_records,
        )
        .await
    };

    package_verification_future.await
}

/// Where a pypi requirement came from. The `index` semantics of a
/// [`uv_distribution_types::Requirement`] depend on this: a `None` index from
/// the manifest means the user did not pin a per-package index, while a
/// `None` index from a parent's `requires_dist` simply means pep508 cannot
/// encode an index at all.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RequirementOrigin {
    /// A direct pypi-dependency declared in the workspace manifest.
    Manifest,
    /// A transitive requirement parsed from another package's
    /// `requires_dist`. The originating pep508 syntax carries no `index`
    /// information.
    RequiresDist,
}

#[allow(clippy::large_enum_variant)]
/// A dependency that needs to be checked in the lock file
pub enum Dependency {
    Input(PackageName, PixiSpec, Cow<'static, str>),
    Conda(MatchSpec, Cow<'static, str>),
    CondaSource(PackageName, SourceSpec, Cow<'static, str>),
    PyPi(
        uv_distribution_types::Requirement,
        Cow<'static, str>,
        RequirementOrigin,
    ),
}

impl Dependency {
    /// Extract the conda package name from this dependency, if it has one.
    /// Returns None for PyPi dependencies.
    pub fn conda_package_name(&self) -> Option<PackageName> {
        match self {
            Dependency::Input(name, _, _) => Some(name.clone()),
            Dependency::Conda(spec, _) => spec.name.as_exact().cloned(),
            Dependency::CondaSource(name, _, _) => Some(name.clone()),
            Dependency::PyPi(_, _, _) => None,
        }
    }
}

/// A struct that records some information about an environment that has been
/// verified.
///
/// Some of this information from an individual environment is useful to have
/// when considering solve groups.
pub struct VerifiedIndividualEnvironment {
    /// All packages in the environment that are expected to be conda packages
    /// e.g. they are in the environment as a direct or transitive dependency of
    /// another conda package.
    pub expected_conda_packages: HashSet<PackageName>,

    /// All conda packages that satisfy a pypi requirement.
    pub conda_packages_used_by_pypi: HashSet<PackageName>,
}

/// Resolve dev dependencies and get all their dependencies
pub async fn resolve_dev_dependencies(
    dev_dependencies: Vec<(PackageName, SourceSpec)>,
    command_dispatcher: &CommandDispatcher,
    channel_config: &rattler_conda_types::ChannelConfig,
    workspace_env_ref: WorkspaceEnvRef,
) -> Result<Vec<Dependency>, CommandDispatcherError<Box<PlatformUnsat>>> {
    // Collect all dev source package names to filter out interdependencies
    let dev_source_names: HashSet<PackageName> = dev_dependencies
        .iter()
        .map(|(name, _)| name.clone())
        .collect();

    let mut futures = CancellationAwareFutures::new(command_dispatcher.executor());

    for (package_name, source_spec) in dev_dependencies {
        let command_dispatcher = command_dispatcher.clone();
        let channel_config = channel_config.clone();
        let workspace_env_ref = workspace_env_ref.clone();
        let dev_source_names = dev_source_names.clone();

        futures.push(resolve_single_dev_dependency(
            package_name,
            source_spec,
            command_dispatcher,
            channel_config,
            workspace_env_ref,
            dev_source_names,
        ));
    }

    futures
        .try_fold(
            Vec::new(),
            |mut resolved_dependencies, elements| async move {
                resolved_dependencies.extend(elements);
                Ok(resolved_dependencies)
            },
        )
        .await
        .map_err_with(Box::new)
}

/// Resolves all dependencies of a single dev dependency
async fn resolve_single_dev_dependency(
    package_name: PackageName,
    source_spec: SourceSpec,
    command_dispatcher: CommandDispatcher,
    channel_config: rattler_conda_types::ChannelConfig,
    workspace_env_ref: WorkspaceEnvRef,
    dev_source_names: HashSet<PackageName>,
) -> Result<Vec<Dependency>, CommandDispatcherError<PlatformUnsat>> {
    let pinned_source = command_dispatcher
        .engine()
        .with_ctx(async |ctx| ctx.pin_and_checkout(source_spec.location).await)
        .await
        .map_err_into_dispatcher(PlatformUnsat::from)?;

    // Create the spec for getting dev source metadata
    let spec = DevSourceMetadataSpec {
        package_name: package_name.clone(),
        backend_metadata: BuildBackendMetadataSpec {
            manifest_source: pinned_source.pinned,
            preferred_build_source: None,
            env_ref: EnvironmentRef::Workspace(workspace_env_ref),
            build_string_prefix: None,
            build_number: None,
        },
    };

    let dev_metadata = command_dispatcher
        .dev_source_metadata(spec)
        .await
        .map_err_with(PlatformUnsat::from)?;

    let dev_deps = DevSourceRecord::dev_source_dependencies(&dev_metadata.records);

    let (dev_source, dev_bin) =
        DevSourceRecord::split_into_source_and_binary_requirements(dev_deps);

    let mut dependencies = Vec::new();

    // Process source dependencies, filtering out dependencies that are also dev sources
    for (dep_name, dep) in dev_source
        .into_specs()
        .filter(|(name, _)| !dev_source_names.contains(name))
    {
        let anchored_source = dep.resolve(&SourceAnchor::Workspace);

        dependencies.push(Dependency::CondaSource(
            dep_name.clone(),
            anchored_source,
            Cow::Owned(package_name.as_source().to_string()),
        ));
    }

    // Process binary dependencies, filtering out dependencies that are also dev sources
    for (dep_name, binary_spec) in dev_bin
        .into_specs()
        .filter(|(name, _)| !dev_source_names.contains(name))
    {
        // Convert BinarySpec to NamelessMatchSpec
        let nameless_spec = binary_spec
            .try_into_nameless_match_spec(&channel_config)
            .map_err(|e| {
                CommandDispatcherError::Failed(PlatformUnsat::FailedToParseMatchSpec(
                    dep_name.as_source().to_string(),
                    spec_conversion_to_match_spec_error(e),
                ))
            })?;

        let spec = MatchSpec::from_nameless(nameless_spec, dep_name.clone().into());

        dependencies.push(Dependency::Conda(
            spec,
            Cow::Owned(package_name.as_source().to_string()),
        ));
    }

    Ok(dependencies)
}

async fn verify_package_platform_satisfiability(
    ctx: &VerifySatisfiabilityContext<'_>,
    platform_setup: &crate::lock_file::platform_setup::PlatformSetup,
    locked_pixi_records: &PixiRecordsByName,
    unresolved_pypi_environment: &PypiRecordsByName,
    building_pixi_records: Result<PixiRecordsByName, PlatformUnsat>,
) -> Result<
    (VerifiedIndividualEnvironment, LockedPypiRecordsByName),
    CommandDispatcherError<Box<PlatformUnsat>>,
> {
    // Determine the dependencies requested by the environment
    let environment_dependencies = ctx
        .environment
        .combined_dependencies(Some(ctx.platform))
        .into_specs()
        .map(|(package_name, spec)| Dependency::Input(package_name, spec, "<environment>".into()))
        .collect_vec();

    // Get the dev dependencies for this platform
    let dev_dependencies = ctx
        .environment
        .combined_dev_dependencies(Some(ctx.platform))
        .into_specs()
        .collect_vec();

    // retrieve dependency-overrides
    // map it to (name => requirement) for later matching
    let dependency_overrides = ctx
        .environment
        .pypi_options()
        .dependency_overrides
        .unwrap_or_default()
        .into_iter()
        .map(|(name, req)| -> Result<_, Box<PlatformUnsat>> {
            let uv_req = as_uv_req(&req, name.as_source(), ctx.project_root).map_err(|e| {
                Box::new(PlatformUnsat::AsPep508Error(
                    name.as_normalized().clone(),
                    e,
                ))
            })?;
            Ok((uv_req.name.clone(), uv_req))
        })
        .collect::<Result<indexmap::IndexMap<_, _>, _>>()
        .map_err(CommandDispatcherError::Failed)?;

    // Find the python interpreter from the list of conda packages. Note that this
    // refers to the locked python interpreter, it might not match the specs
    // from the environment. That is ok because we will find that out when we
    // check all the records.
    let python_interpreter_record = locked_pixi_records.python_interpreter_record();

    // Determine the marker environment from the python interpreter package.
    let marker_environment = python_interpreter_record
        .map(|interpreter| determine_marker_environment(ctx.platform, &interpreter.package_record))
        .transpose()
        .map_err(|err| {
            Box::new(PlatformUnsat::FailedToDetermineMarkerEnvironment(
                err.into(),
            ))
        });

    let pypi_dependencies = ctx.environment.pypi_dependencies(Some(ctx.platform));

    // We cannot determine the marker environment, for example if installing
    // `wasm32` dependencies. However, it also doesn't really matter if we don't
    // have any pypi requirements.
    let marker_environment = match marker_environment {
        Err(err) => {
            if !pypi_dependencies.is_empty() {
                return Err(CommandDispatcherError::Failed(err));
            } else {
                None
            }
        }
        Ok(marker_environment) => marker_environment,
    };

    // Transform from PyPiPackage name into UV Requirement type
    let project_root = ctx.project_root;
    let pypi_requirements = pypi_dependencies
        .iter()
        .flat_map(|(name, reqs)| {
            reqs.iter()
                .map(|req| as_uv_req(req, name.as_source(), project_root))
                .filter_ok(|req| req.evaluate_markers(marker_environment.as_ref(), &req.extras))
                .map(move |req| {
                    Ok::<Dependency, Box<PlatformUnsat>>(Dependency::PyPi(
                        req.map_err(|e| {
                            Box::new(PlatformUnsat::AsPep508Error(
                                name.as_normalized().clone(),
                                e,
                            ))
                        })?,
                        "<environment>".into(),
                        RequirementOrigin::Manifest,
                    ))
                })
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(CommandDispatcherError::Failed)?;

    if pypi_requirements.is_empty() && !unresolved_pypi_environment.is_empty() {
        return Err(CommandDispatcherError::Failed(Box::new(
            PlatformUnsat::TooManyPypiPackages(
                unresolved_pypi_environment.names().cloned().collect(),
            ),
        )));
    }

    let virtual_packages = platform_setup
        .virtual_packages
        .iter()
        .cloned()
        .map(|vpkg| (vpkg.name.clone(), vpkg))
        .collect::<HashMap<_, _>>();

    let channel_config = platform_setup.channel_config.clone();

    // Check that all locked conda packages satisfy the current constraints.
    // If a constraint is violated, the lock file needs to be re-solved.
    for (package_name, pixi_spec) in ctx
        .environment
        .combined_constraints(Some(ctx.platform))
        .into_specs()
    {
        // Source specs are not valid in [constraints]; raise an error.
        let binary_spec = match pixi_spec.into_source_or_binary() {
            Either::Left(_) => {
                return Err(CommandDispatcherError::Failed(Box::new(
                    PlatformUnsat::SourceConstraintNotSupported(
                        package_name.as_source().to_string(),
                    ),
                )));
            }
            Either::Right(binary_spec) => binary_spec,
        };
        let nameless_spec = binary_spec
            .try_into_nameless_match_spec(&channel_config)
            .map_err(|e| {
                CommandDispatcherError::Failed(failed_to_parse_match_spec_unsat(
                    package_name.as_source(),
                    spec_conversion_to_match_spec_error(e),
                ))
            })?;
        // Only check packages that are actually locked; constraints only apply
        // to installed packages. Source records are excluded because they are
        // controlled via their source spec, not version constraints.
        if let Some(locked_record) = locked_pixi_records.by_name(&package_name)
            && let Some(binary_record) = locked_record.as_binary()
            && !nameless_spec.matches(&binary_record.package_record)
        {
            return Err(CommandDispatcherError::Failed(Box::new(
                PlatformUnsat::ConstraintViolated {
                    package: package_name.as_source().to_string(),
                    locked_version: binary_record.package_record.version.to_string(),
                    constraint: nameless_spec.to_string(),
                },
            )));
        }
    }

    let resolve_dev_dependencies_future = resolve_dev_dependencies(
        dev_dependencies,
        &ctx.command_dispatcher,
        &channel_config,
        platform_setup.workspace_env_ref.clone(),
    );

    // Determine the pypi packages provided by the locked conda packages.
    let locked_conda_pypi_packages = locked_pixi_records
        .by_pypi_name()
        .map_err(|e| CommandDispatcherError::Failed(Box::new(e.into())))?;

    let lock_pypi_packages_future = lock_pypi_packages(
        ctx,
        locked_pixi_records,
        unresolved_pypi_environment,
        building_pixi_records,
    );
    let (resolved_dev_dependencies, locked_pypi_records) =
        futures::try_join!(resolve_dev_dependencies_future, lock_pypi_packages_future)?;

    if (environment_dependencies.is_empty() && resolved_dev_dependencies.is_empty())
        && !locked_pixi_records.is_empty()
    {
        return Err(CommandDispatcherError::Failed(Box::new(
            PlatformUnsat::TooManyCondaPackages(Vec::new()),
        )));
    }

    // Keep a list of all conda packages that we have already visited
    let mut conda_packages_visited = HashSet::new();
    let mut pypi_packages_visited = HashSet::new();
    let mut pypi_requirements_visited = pypi_requirements
        .iter()
        .filter_map(|r| match r {
            Dependency::PyPi(req, _, _) => Some(req.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();

    // Iterate over all packages. First iterate over all conda matchspecs and then
    // over all pypi requirements. We want to ensure we always check the conda
    // packages first.
    let mut conda_stack = environment_dependencies
        .into_iter()
        .chain(resolved_dev_dependencies)
        .collect_vec();
    let mut pypi_queue = pypi_requirements;
    let mut expected_conda_source_dependencies = HashSet::new();
    let mut expected_conda_packages = HashSet::new();
    let mut conda_packages_used_by_pypi = HashSet::new();
    let mut delayed_pypi_error = None;

    while let Some(package) = conda_stack.pop().or_else(|| pypi_queue.pop()) {
        // Determine the package that matches the requirement of matchspec.
        let found_package = match package {
            Dependency::Input(name, spec, source) => {
                let found_package = match spec.into_source_or_binary() {
                    Either::Left(source_spec) => {
                        expected_conda_source_dependencies.insert(name.clone());
                        find_matching_source_package(locked_pixi_records, name, source_spec, source)
                            .map_err(CommandDispatcherError::Failed)?
                    }
                    Either::Right(binary_spec) => {
                        let spec = binary_spec
                            .try_into_nameless_match_spec(&channel_config)
                            .map_err(|e| {
                                CommandDispatcherError::Failed(failed_to_parse_match_spec_unsat(
                                    name.as_source(),
                                    spec_conversion_to_match_spec_error(e),
                                ))
                            })?;
                        match find_matching_package(
                            locked_pixi_records,
                            &virtual_packages,
                            MatchSpec::from_nameless(spec, name.into()),
                            source,
                        )
                        .map_err(CommandDispatcherError::Failed)?
                        {
                            Some(pkg) => pkg,
                            None => continue,
                        }
                    }
                };

                expected_conda_packages
                    .insert(locked_pixi_records.records[found_package.0].name().clone());
                FoundPackage::Conda(found_package)
            }
            Dependency::Conda(spec, source) => {
                match find_matching_package(locked_pixi_records, &virtual_packages, spec, source)
                    .map_err(CommandDispatcherError::Failed)?
                {
                    Some(pkg) => {
                        expected_conda_packages
                            .insert(locked_pixi_records.records[pkg.0].name().clone());
                        FoundPackage::Conda(pkg)
                    }
                    None => continue,
                }
            }
            Dependency::CondaSource(name, source_spec, source) => {
                expected_conda_source_dependencies.insert(name.clone());
                FoundPackage::Conda(
                    find_matching_source_package(locked_pixi_records, name, source_spec, source)
                        .map_err(CommandDispatcherError::Failed)?,
                )
            }
            Dependency::PyPi(requirement, source, origin) => {
                // Check if there is a pypi identifier that matches our requirement.
                if let Some((identifier, repodata_idx, _)) =
                    locked_conda_pypi_packages.get(&requirement.name)
                {
                    if requirement.is_editable() {
                        delayed_pypi_error.get_or_insert_with(|| {
                            Box::new(PlatformUnsat::EditableDependencyOnCondaInstalledPackage(
                                requirement.name.clone(),
                                Box::new(requirement.source.clone()),
                            ))
                        });
                    }

                    if matches!(requirement.source, RequirementSource::Url { .. }) {
                        delayed_pypi_error.get_or_insert_with(|| {
                            Box::new(PlatformUnsat::DirectUrlDependencyOnCondaInstalledPackage(
                                requirement.name.clone(),
                            ))
                        });
                    }

                    if matches!(requirement.source, RequirementSource::Git { .. }) {
                        delayed_pypi_error.get_or_insert_with(|| {
                            Box::new(PlatformUnsat::GitDependencyOnCondaInstalledPackage(
                                requirement.name.clone(),
                            ))
                        });
                    }

                    // Use the overridden requirement if specified (e.g. for pytorch/torch)
                    let requirement_to_check = dependency_overrides
                        .get(&requirement.name)
                        .cloned()
                        .unwrap_or(requirement.clone());

                    if !identifier
                        .satisfies(&requirement_to_check)
                        .map_err(CommandDispatcherError::Failed)?
                    {
                        // The record does not match the spec, the lock-file is inconsistent.
                        delayed_pypi_error.get_or_insert_with(|| {
                            Box::new(PlatformUnsat::CondaUnsatisfiableRequirement(
                                Box::new(requirement.clone()),
                                source.into_owned(),
                            ))
                        });
                    }
                    let pkg_idx = CondaPackageIdx(*repodata_idx);
                    conda_packages_used_by_pypi
                        .insert(locked_pixi_records.records[pkg_idx.0].name().clone());
                    FoundPackage::Conda(pkg_idx)
                } else {
                    match to_normalize(&requirement.name)
                        .map(|name| locked_pypi_records.index_by_name(&name))
                    {
                        Ok(Some(idx)) => {
                            let record = &locked_pypi_records.records[idx];

                            // use the overridden requirements if specified
                            let requirement = dependency_overrides
                                .get(&requirement.name)
                                .cloned()
                                .unwrap_or(requirement);

                            if requirement.is_editable() {
                                if let Err(err) =
                                    pypi_satisfies_editable(&requirement, record, ctx.project_root)
                                {
                                    delayed_pypi_error.get_or_insert(err);
                                }

                                FoundPackage::PyPi(PypiPackageIdx(idx), requirement.extras.to_vec())
                            } else {
                                if let Err(err) = pypi_satisfies_requirement(
                                    &requirement,
                                    record,
                                    ctx.project_root,
                                    origin,
                                ) {
                                    delayed_pypi_error.get_or_insert(err);
                                }

                                FoundPackage::PyPi(PypiPackageIdx(idx), requirement.extras.to_vec())
                            }
                        }
                        Ok(None) => {
                            // The record does not match the spec, the lock-file is inconsistent.
                            delayed_pypi_error.get_or_insert_with(|| {
                                Box::new(PlatformUnsat::UnsatisfiableRequirement(
                                    Box::new(requirement),
                                    source.into_owned(),
                                ))
                            });
                            continue;
                        }
                        Err(err) => {
                            // An error occurred while converting the package name.
                            delayed_pypi_error.get_or_insert_with(|| {
                                Box::new(PlatformUnsat::from(ConversionError::NameConversion(err)))
                            });
                            continue;
                        }
                    }
                }
            }
        };

        // Add all the requirements of the package to the queue.
        match found_package {
            FoundPackage::Conda(idx) => {
                if !conda_packages_visited.insert(idx) {
                    // We already visited this package, so we can skip adding its dependencies to
                    // the queue
                    continue;
                }

                let record = &locked_pixi_records.records[idx.0];
                for depends in &record.package_record().depends {
                    let spec = MatchSpec::from_str(depends.as_str(), Lenient).map_err(|e| {
                        CommandDispatcherError::Failed(Box::new(
                            PlatformUnsat::FailedToParseMatchSpec(depends.clone(), e),
                        ))
                    })?;
                    let (name, spec) = spec.into_nameless();

                    let (origin, anchor) = match record {
                        PixiRecord::Binary(record) => (
                            Cow::Owned(record.identifier.to_file_name()),
                            SourceAnchor::Workspace,
                        ),
                        PixiRecord::Source(record) => (
                            Cow::Owned(format!(
                                "{} @ {}",
                                record.name().as_source(),
                                record.manifest_source
                            )),
                            SourceLocationSpec::from(record.manifest_source.clone()).into(),
                        ),
                    };

                    if let Some((source, package_name)) = record.as_source().and_then(|record| {
                        let package_name = name
                            .as_exact()
                            .expect("depends can only contain exact package names");
                        Some((
                            record.sources().get(package_name.as_normalized())?,
                            package_name,
                        ))
                    }) {
                        let anchored_location = anchor.resolve(source.clone());
                        let source_spec = SourceSpec::new(anchored_location, spec);
                        conda_stack.push(Dependency::CondaSource(
                            package_name.clone(),
                            source_spec,
                            origin,
                        ));
                    } else {
                        conda_stack.push(Dependency::Conda(
                            MatchSpec::from_nameless(spec, name),
                            origin,
                        ));
                    }
                }
            }
            FoundPackage::PyPi(idx, extras) => {
                let record = &locked_pypi_records.records[idx.0];
                let pkg = &record.data;

                // If there is no marker environment there is no python version
                let Some(marker_environment) = marker_environment.as_ref() else {
                    return Err(CommandDispatcherError::Failed(Box::new(
                        PlatformUnsat::MissingPythonInterpreter,
                    )));
                };

                if pypi_packages_visited.insert(idx) {
                    // Compare cached metadata with locked metadata for
                    // path-based source packages. The metadata was read into
                    // static_metadata_cache during lock_pypi_packages().
                    if let UrlOrPath::Path(path) = &**pkg.location() {
                        let absolute_path = if path.is_absolute() {
                            PathBuf::from(path.as_str())
                        } else {
                            ctx.project_root.join(Path::new(path.as_str()))
                        };
                        if let Some(current_metadata) =
                            ctx.static_metadata_cache.get(&absolute_path)
                            && let Some(mismatch) =
                                pypi_metadata::compare_metadata(record, &current_metadata)
                        {
                            let local_mismatch = match mismatch {
                                pypi_metadata::MetadataMismatch::RequiresDist(diff) => {
                                    LocalMetadataMismatch::RequiresDist {
                                        added: diff.added,
                                        removed: diff.removed,
                                    }
                                }
                                pypi_metadata::MetadataMismatch::Version { locked, current } => {
                                    LocalMetadataMismatch::Version { locked, current }
                                }
                                pypi_metadata::MetadataMismatch::RequiresPython {
                                    locked,
                                    current,
                                } => LocalMetadataMismatch::RequiresPython { locked, current },
                            };
                            delayed_pypi_error.get_or_insert_with(|| {
                                Box::new(PlatformUnsat::LocalPackageMetadataMismatch(
                                    pkg.name().clone(),
                                    local_mismatch,
                                ))
                            });
                        }
                    }

                    // Ensure that the record matches the currently selected interpreter.
                    if let Some(requires_python) = pkg.requires_python() {
                        let uv_specifier_requires_python = to_uv_specifiers(requires_python)
                            .expect("pep440 conversion should never fail");

                        let marker_version = pep440_rs::Version::from_str(
                            &marker_environment.python_full_version().version.to_string(),
                        )
                        .expect("cannot parse version");
                        let uv_maker_version = to_uv_version(&marker_version)
                            .expect("cannot convert python marker version to uv_pep440");

                        let marker_requires_python =
                            RequiresPython::greater_than_equal_version(&uv_maker_version);
                        // Use the function of RequiresPython object as it implements the lower
                        // bound logic Related issue https://github.com/astral-sh/uv/issues/4022
                        if !marker_requires_python.is_contained_by(&uv_specifier_requires_python) {
                            delayed_pypi_error.get_or_insert_with(|| {
                                Box::new(PlatformUnsat::PythonVersionMismatch(
                                    pkg.name().clone(),
                                    requires_python.clone(),
                                    marker_version.into(),
                                ))
                            });
                        }
                    }
                }

                // Add all the requirements of the package to the queue.
                for requirement in pkg.requires_dist() {
                    let requirement =
                        match pep508_requirement_to_uv_requirement(requirement.clone()) {
                            Ok(requirement) => requirement,
                            Err(err) => {
                                delayed_pypi_error.get_or_insert_with(|| {
                                    Box::new(ConversionError::NameConversion(err).into())
                                });
                                continue;
                            }
                        };

                    // Skip this requirement if it does not apply.
                    if !requirement.evaluate_markers(Some(marker_environment), &extras) {
                        continue;
                    }

                    // Skip this requirement if it has already been visited.
                    if !pypi_requirements_visited.insert(requirement.clone()) {
                        continue;
                    }

                    pypi_queue.push(Dependency::PyPi(
                        requirement.clone(),
                        pkg.name().as_ref().to_string().into(),
                        RequirementOrigin::RequiresDist,
                    ));
                }
            }
        }
    }

    // Check if all locked packages have also been visited
    if conda_packages_visited.len() != locked_pixi_records.len() {
        return Err(CommandDispatcherError::Failed(Box::new(
            PlatformUnsat::TooManyCondaPackages(
                locked_pixi_records
                    .names()
                    .enumerate()
                    .filter_map(|(idx, name)| {
                        if conda_packages_visited.contains(&CondaPackageIdx(idx)) {
                            None
                        } else {
                            Some(name.clone())
                        }
                    })
                    .collect(),
            ),
        )));
    }

    // Check if all records that are source records should actually be source
    // records. If there are no source specs in the environment for a particular
    // package than the package must be a binary package.
    for record in locked_pixi_records
        .records
        .iter()
        .filter_map(PixiRecord::as_source)
    {
        if !expected_conda_source_dependencies.contains(record.name()) {
            return Err(CommandDispatcherError::Failed(Box::new(
                PlatformUnsat::RequiredBinaryIsSource(record.name().as_source().to_string()),
            )));
        }
    }

    // Now that we checked all conda requirements, check if there were any pypi
    // issues.
    if let Some(err) = delayed_pypi_error {
        return Err(CommandDispatcherError::Failed(err));
    }

    if pypi_packages_visited.len() != locked_pypi_records.len() {
        return Err(CommandDispatcherError::Failed(Box::new(
            PlatformUnsat::TooManyPypiPackages(
                locked_pypi_records
                    .names()
                    .enumerate()
                    .filter_map(|(idx, name)| {
                        if pypi_packages_visited.contains(&PypiPackageIdx(idx)) {
                            None
                        } else {
                            Some(name.clone())
                        }
                    })
                    .collect(),
            ),
        )));
    }

    // Note: Editability is NOT checked here. The lock file always stores
    // editable=false (which is omitted from serialization). Editability is
    // looked up from the manifest at install time. This allows different
    // environments in a solve-group to have different editability settings for
    // the same path-based package.

    // Verify the pixi build package's package_build_source matches the manifest.
    verify_build_source_matches_manifest(ctx.environment, locked_pixi_records)
        .map_err(CommandDispatcherError::Failed)?;

    Ok((
        VerifiedIndividualEnvironment {
            expected_conda_packages,
            conda_packages_used_by_pypi,
        },
        locked_pypi_records,
    ))
}

enum FoundPackage {
    Conda(CondaPackageIdx),
    PyPi(PypiPackageIdx, Vec<uv_normalize::ExtraName>),
}

/// An index into the list of conda packages.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
#[repr(transparent)]
pub struct CondaPackageIdx(usize);

/// An index into the list of pypi packages.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
#[repr(transparent)]
pub struct PypiPackageIdx(usize);

fn find_matching_package(
    locked_pixi_records: &PixiRecordsByName,
    virtual_packages: &HashMap<PackageName, GenericVirtualPackage>,
    spec: MatchSpec,
    source: Cow<str>,
) -> Result<Option<CondaPackageIdx>, Box<PlatformUnsat>> {
    let found_package = match spec.name.as_exact() {
        None => {
            // No exact name means we have to find any package that matches the spec.
            match locked_pixi_records
                .records
                .iter()
                .position(|record| spec.matches(record))
            {
                None => {
                    // No records match the spec.
                    return Err(Box::new(PlatformUnsat::UnsatisfiableMatchSpec(
                        Box::new(spec),
                        source.into_owned(),
                    )));
                }
                Some(idx) => idx,
            }
        }
        Some(name) => {
            match locked_pixi_records
                .index_by_name(name)
                .map(|idx| (idx, &locked_pixi_records.records[idx]))
            {
                Some((idx, record)) if spec.matches(record) => idx,
                Some(_) => {
                    // The record does not match the spec, the lock-file is
                    // inconsistent.
                    return Err(Box::new(PlatformUnsat::UnsatisfiableMatchSpec(
                        Box::new(spec),
                        source.into_owned(),
                    )));
                }
                None => {
                    // Check if there is a virtual package by that name
                    if let Some(vpkg) = virtual_packages.get(name.as_normalized()) {
                        if vpkg.matches(&spec) {
                            // The matchspec matches a virtual package. No need to
                            // propagate the dependencies.
                            return Ok(None);
                        } else {
                            // The record does not match the spec, the lock-file is
                            // inconsistent.
                            return Err(Box::new(PlatformUnsat::UnsatisfiableMatchSpec(
                                Box::new(spec),
                                source.into_owned(),
                            )));
                        }
                    } else {
                        // The record does not match the spec, the lock-file is
                        // inconsistent.
                        return Err(Box::new(PlatformUnsat::UnsatisfiableMatchSpec(
                            Box::new(spec),
                            source.into_owned(),
                        )));
                    }
                }
            }
        }
    };

    Ok(Some(CondaPackageIdx(found_package)))
}

fn find_matching_source_package(
    locked_pixi_records: &PixiRecordsByName,
    name: PackageName,
    source_spec: SourceSpec,
    source: Cow<str>,
) -> Result<CondaPackageIdx, Box<PlatformUnsat>> {
    // Find the package that matches the source spec.
    let Some((idx, package)) = locked_pixi_records
        .index_by_name(&name)
        .map(|idx| (idx, &locked_pixi_records.records[idx]))
    else {
        // The record does not match the spec, the lock-file is
        // inconsistent.
        return Err(Box::new(PlatformUnsat::SourcePackageMissing(
            name.as_source().to_string(),
            source.into_owned(),
        )));
    };

    let PixiRecord::Source(source_package) = package else {
        return Err(Box::new(PlatformUnsat::RequiredSourceIsBinary(
            name.as_source().to_string(),
            source.into_owned(),
        )));
    };

    source_package
        .manifest_source
        .satisfies(&source_spec.location)
        .map_err(|e| PlatformUnsat::SourcePackageMismatch(name.as_source().to_string(), e))?;

    let match_spec = source_spec.to_nameless_match_spec();
    if !match_spec.matches(package) {
        return Err(Box::new(PlatformUnsat::UnsatisfiableMatchSpec(
            Box::new(MatchSpec::from_nameless(match_spec, name.into())),
            source.into_owned(),
        )));
    }

    Ok(CondaPackageIdx(idx))
}

/// Map a `SpecConversionError` raised while turning a pixi spec into a
/// nameless [`MatchSpec`] into a [`ParseMatchSpecError`] suitable for
/// surfacing through [`PlatformUnsat::FailedToParseMatchSpec`].
///
/// The same translation is needed wherever `BinarySpec` /
/// `try_into_nameless_match_spec` is called from the satisfiability
/// path (workspace deps, dev deps, transitive deps, backend-declared
/// build/host deps), so this helper keeps every call site aligned.
pub(super) fn spec_conversion_to_match_spec_error(e: SpecConversionError) -> ParseMatchSpecError {
    match e {
        SpecConversionError::NonAbsoluteRootDir(p) => {
            ParseChannelError::NonAbsoluteRootDir(p).into()
        }
        SpecConversionError::NotUtf8RootDir(p) => ParseChannelError::NotUtf8RootDir(p).into(),
        SpecConversionError::InvalidPath(p) => ParseChannelError::InvalidPath(p).into(),
        SpecConversionError::InvalidChannel(_name, p) => p.into(),
        SpecConversionError::MissingName => ParseMatchSpecError::MissingPackageName,
    }
}

/// Wrap a parse error into a `PlatformUnsat::FailedToParseMatchSpec`.
pub(super) fn failed_to_parse_match_spec_unsat(
    name: &str,
    err: ParseMatchSpecError,
) -> Box<PlatformUnsat> {
    Box::new(PlatformUnsat::FailedToParseMatchSpec(name.to_string(), err))
}

trait MatchesMatchspec {
    fn matches(&self, spec: &MatchSpec) -> bool;
}

impl MatchesMatchspec for GenericVirtualPackage {
    fn matches(&self, spec: &MatchSpec) -> bool {
        if !spec.name.matches(&self.name) {
            return false;
        }

        if let Some(version) = &spec.version
            && !version.matches(&self.version)
        {
            return false;
        }

        if let Some(build) = &spec.build
            && !build.matches(&self.build_string)
        {
            return false;
        }

        true
    }
}

pub fn verify_solve_group_satisfiability(
    environments: impl IntoIterator<Item = VerifiedIndividualEnvironment>,
) -> Result<(), SolveGroupUnsat> {
    let mut expected_conda_packages = HashSet::new();
    let mut conda_packages_used_by_pypi = HashSet::new();

    // Group all conda requested packages and pypi requested packages
    for env in environments {
        expected_conda_packages.extend(env.expected_conda_packages);
        conda_packages_used_by_pypi.extend(env.conda_packages_used_by_pypi);
    }

    // Check if all conda packages are also requested by another conda package.
    if let Some(conda_package) = conda_packages_used_by_pypi
        .into_iter()
        .find(|pkg| !expected_conda_packages.contains(pkg))
    {
        return Err(SolveGroupUnsat::CondaPackageShouldBePypi {
            name: conda_package.as_source().to_string(),
        });
    }

    Ok(())
}
