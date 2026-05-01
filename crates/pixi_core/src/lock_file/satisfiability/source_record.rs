use std::sync::Arc;

use pixi_command_dispatcher::{
    CommandDispatcherError,
    build::{Dependencies, PixiRunExports, dependencies::filter_match_specs, pin_compatible::PinCompatibilityMap},
};
use pixi_record::{
    PinnedBuildSourceSpec, PinnedSourceSpec, PixiRecord, SourceMismatchError, SourceRecordData,
};
use pixi_spec::{SourceAnchor, SourceLocationSpec, SpecConversionError};
use rattler_conda_types::{MatchSpec, Matches, NamelessMatchSpec, PackageName};
use std::collections::HashSet;

use super::errors::{BuildOrHostEnv, PlatformUnsat, SourceRunDepKind};
use super::platform::{
    VerifySatisfiabilityContext, failed_to_parse_match_spec_unsat,
    spec_conversion_to_match_spec_error,
};
use crate::{
    lock_file::PixiRecordsByName,
    workspace::{Environment, HasWorkspaceRef},
};

/// Verify that the current package's build.source in the manifest
/// matches the lock file's `package_build_source` (if applicable).
/// Path-based sources are not represented in the lock file's
/// `package_build_source` and are skipped.
pub(super) fn verify_build_source_matches_manifest(
    environment: &Environment<'_>,
    locked_pixi_records: &PixiRecordsByName,
) -> Result<(), Box<PlatformUnsat>> {
    let Some(pkg_manifest) = environment.workspace().package.as_ref() else {
        return Ok(());
    };
    let Some(pkg_name) = &pkg_manifest.value.package.name else {
        return Ok(());
    };
    let package_name = PackageName::new_unchecked(pkg_name);
    let manifest_source_location = pkg_manifest.value.build.source.clone();

    // Find the source record for the current package in locked conda packages.
    let Some(record) = locked_pixi_records.by_name(&package_name) else {
        return Ok(());
    };

    let PixiRecord::Source(src_record) = record else {
        return Ok(());
    };

    let lockfile_source_location = src_record.build_source.clone();

    let ok = Ok(());
    let error = Err(Box::new(PlatformUnsat::PackageBuildSourceMismatch(
        src_record.name().as_source().to_string(),
        SourceMismatchError::SourceTypeMismatch,
    )));
    let sat_err = |e| {
        Box::new(PlatformUnsat::PackageBuildSourceMismatch(
            src_record.name().as_source().to_string(),
            e,
        ))
    };

    match (
        manifest_source_location,
        lockfile_source_location.map(PinnedBuildSourceSpec::into_pinned),
    ) {
        (None, None) => ok,
        (Some(SourceLocationSpec::Url(murl_spec)), Some(PinnedSourceSpec::Url(lurl_spec))) => {
            lurl_spec.satisfies(&murl_spec).map_err(sat_err)
        }
        (
            Some(SourceLocationSpec::Git(mut mgit_spec)),
            Some(PinnedSourceSpec::Git(mut lgit_spec)),
        ) => {
            // Ignore subdirectory for comparison, they should not
            // trigger lockfile invalidation.
            mgit_spec.subdirectory = Default::default();
            lgit_spec.source.subdirectory = Default::default();

            // Ensure that we always compare references.
            if mgit_spec.rev.is_none() {
                mgit_spec.rev = Some(pixi_spec::GitReference::DefaultBranch);
            }
            lgit_spec.satisfies(&mgit_spec).map_err(sat_err)
        }
        (Some(SourceLocationSpec::Path(mpath_spec)), Some(PinnedSourceSpec::Path(lpath_spec))) => {
            lpath_spec.satisfies(&mpath_spec).map_err(sat_err)
        }
        // If they not equal kind we error-out
        (_, _) => error,
    }
}

/// Verify that the locked build/host packages of a partial / mutable
/// source record still satisfy the build backend's declared specs for
/// the matching output, then return a freshly-assembled full source
/// record built from the backend output.
///
/// Returns the freshly-resolved record on success. Returns a specific
/// [`PlatformUnsat`] variant on the first mismatch so the caller can
/// surface a useful diagnostic and trigger a re-lock that carries the
/// locked build/host packages forward as solver hints.
pub(super) async fn verify_partial_source_record_against_backend(
    ctx: &VerifySatisfiabilityContext<'_>,
    platform_setup: &crate::lock_file::platform_setup::PlatformSetup,
    record: &pixi_record::UnresolvedSourceRecord,
) -> Result<Arc<pixi_record::SourceRecord>, CommandDispatcherError<Box<PlatformUnsat>>> {
    use pixi_command_dispatcher::BuildBackendMetadataSpec;

    let pkg_name = record.name().clone();

    // Query fresh backend metadata for the source's manifest checkout.
    let backend_metadata = ctx
        .command_dispatcher
        .build_backend_metadata(BuildBackendMetadataSpec {
            manifest_source: record.manifest_source.clone(),
            preferred_build_source: record
                .build_source
                .as_ref()
                .map(|bs| bs.clone().into_pinned()),
            env_ref: pixi_command_dispatcher::EnvironmentRef::Workspace(
                platform_setup.workspace_env_ref.clone(),
            ),
        })
        .await
        .map_err(|e| match e {
            CommandDispatcherError::Cancelled => CommandDispatcherError::Cancelled,
            CommandDispatcherError::Failed(err) => CommandDispatcherError::Failed(Box::new(
                PlatformUnsat::SourcePackageMetadataChanged(
                    pkg_name.as_source().to_string(),
                    err.to_string(),
                ),
            )),
        })?;

    // Pick the matching output by (name, variants). Variants are
    // compared after normalizing both sides through `pixi_variant`'s
    // `VariantValue` so backend-side `pixi_build_types::VariantValue`
    // entries align with the locked record's stored representation.
    let locked_variants = &record.variants;
    let matching_output = backend_metadata
        .metadata
        .outputs
        .iter()
        .find(|o| {
            o.metadata.name == pkg_name && variants_equivalent(locked_variants, &o.metadata.variant)
        })
        .ok_or_else(|| {
            CommandDispatcherError::Failed(Box::new(PlatformUnsat::SourceVariantNotInBackend {
                package: pkg_name.as_source().to_string(),
                manifest_source: record.manifest_source.to_string(),
                variants: format_variants(locked_variants),
            }))
        })?;

    // Verify that every backend-declared build dep is satisfied by the
    // locked build_packages, and same for host. Source-spec deps must
    // map to a locked source record at a compatible location; PyPI
    // shapes are rejected (impossible at the type level today, but the
    // check guards against future shape additions).
    //
    // Empty `record.build_packages` / `record.host_packages` is not a
    // pass: it just means every backend-declared spec has nothing to
    // satisfy it, so verification fails and a re-lock is forced. This
    // is intentional. Lock files written before build/host packages
    // were round-tripped carry empty slices regardless of what the
    // backend actually declares, and for mutable sources (the only
    // shape that reaches this code path) we cannot tell whether those
    // empty slices reflect the truth or a stale snapshot. Forcing one
    // re-lock populates the canonical slices so subsequent runs hit
    // the fast verification path.
    let source_anchor =
        SourceAnchor::from(SourceLocationSpec::from(record.manifest_source.clone()));
    if let Some(build_deps) = matching_output.build_dependencies.as_ref() {
        verify_locked_against_backend_specs(
            build_deps,
            &record.build_packages,
            &[],
            &platform_setup.channel_config,
            &source_anchor,
            &pkg_name,
            BuildOrHostEnv::Build,
        )
        .map_err(CommandDispatcherError::Failed)?;
    }
    if let Some(host_deps) = matching_output.host_dependencies.as_ref() {
        verify_locked_against_backend_specs(
            host_deps,
            &record.host_packages,
            &record.build_packages,
            &platform_setup.channel_config,
            &source_anchor,
            &pkg_name,
            BuildOrHostEnv::Host,
        )
        .map_err(CommandDispatcherError::Failed)?;
    }

    // Verify that the locked record's runtime `depends` and `constrains`
    // still match what the backend would re-derive from its declared
    // run-dependencies plus the resolved build/host packages'
    // run-exports. This catches manifest edits to
    // `[package.run-dependencies]` (and its constraints sibling) that
    // the build/host check above can't see, because changes there don't
    // necessarily perturb the build/host envs at all.
    verify_locked_run_deps_against_backend(record, matching_output, &platform_setup.channel_config)
        .map_err(CommandDispatcherError::Failed)?;

    // Synthesize a full record from the matching output. We use the
    // backend's freshly-computed PackageRecord (version, build,
    // depends, etc.) but keep the locked build/host packages so the
    // downstream verification observes the same env the solver
    // previously chose. The pinned source / build_source come from the
    // locked record so paths and commits don't drift.
    Ok(Arc::new(build_full_source_record_from_output(
        record,
        matching_output,
    )))
}

/// Reassemble what the locked source record's `depends` (and
/// `constrains`) would look like if produced by a fresh backend
/// resolution, then assert it matches what's actually locked. The
/// reconstruction goes through the same `Dependencies` machinery the
/// solve path uses, so the resulting strings line up byte-for-byte
/// without any `MatchSpec::from_str` parse on the locked side.
pub(crate) fn verify_locked_run_deps_against_backend(
    record: &pixi_record::UnresolvedSourceRecord,
    matching_output: &pixi_build_types::procedures::conda_outputs::CondaOutput,
    channel_config: &rattler_conda_types::ChannelConfig,
) -> Result<(), Box<PlatformUnsat>> {
    // Resolve build/host package slices into PixiRecords for the pin
    // compatibility map and the run-export collection. Partial source
    // records get dropped: pin_compatible against a record whose
    // version isn't yet materialised would just fail downstream, and
    // their run_exports are necessarily absent.
    let resolved_build = resolved_records(&record.build_packages);
    let resolved_host = resolved_records(&record.host_packages);

    // Build the pin compatibility map from build records first, then
    // host records, mirroring the order in `resolve_source_record` so
    // pin_compatible(host_dep) finds host entries when both envs name
    // the same package.
    let mut compat_map: PinCompatibilityMap = std::collections::HashMap::new();
    compat_map.extend(
        resolved_build
            .iter()
            .map(|r| (r.package_record().name.clone(), r)),
    );
    compat_map.extend(
        resolved_host
            .iter()
            .map(|r| (r.package_record().name.clone(), r)),
    );

    // Pull run_exports off direct host/build deps. Indirect (transitive)
    // entries don't contribute run-exports per conda-build semantics,
    // so we filter by the names the backend actually declares.
    let host_run_exports = collect_direct_run_exports(
        matching_output.host_dependencies.as_ref(),
        &record.host_packages,
        &matching_output.ignore_run_exports,
    );
    let build_run_exports = collect_direct_run_exports(
        matching_output.build_dependencies.as_ref(),
        &record.build_packages,
        &matching_output.ignore_run_exports,
    );

    // Reassemble the typed Dependencies the same way `resolve_source_record`
    // does at solve time: bare run-deps + run-export merge.
    let assembled = Dependencies::new(&matching_output.run_dependencies, None, &compat_map)
        .map_err(|err| {
            Box::new(PlatformUnsat::SourcePackageMetadataChanged(
                record.name().as_source().to_string(),
                err.to_string(),
            ))
        })?
        .extend_with_run_exports_from_build_and_host(
            host_run_exports,
            build_run_exports,
            matching_output.metadata.subdir,
        );

    // Stringify both halves with the same pipeline that produces
    // `locked.depends` / `locked.constrains` at solve time, then
    // compare as ordered sequences (no `MatchSpec::from_str` on the
    // locked side, and a reorder is real drift since `DependencyMap`
    // iteration is the source of locked order).
    let expected_depends = assembled
        .dependencies
        .iter_specs()
        .map(|(name, withspec)| {
            withspec
                .value
                .clone()
                .to_match_spec(name, channel_config)
                .map(|m| m.to_string())
                .map_err(|err| spec_unsat(name, &err))
        })
        .collect::<Result<Vec<_>, _>>()?;
    diff_dep_sequences(record.depends(), &expected_depends)
        .map_err(|diff| diff.into_unsat(record.name(), SourceRunDepKind::RunDepends))?;

    let expected_constrains = assembled
        .constraints
        .iter_specs()
        .map(|(name, withspec)| {
            withspec
                .value
                .clone()
                .to_match_spec(name, channel_config)
                .map(|m| m.to_string())
                .map_err(|err| spec_unsat(name, &err))
        })
        .collect::<Result<Vec<_>, _>>()?;
    diff_dep_sequences(record.constrains(), &expected_constrains)
        .map_err(|diff| diff.into_unsat(record.name(), SourceRunDepKind::RunConstrains))?;

    Ok(())
}

/// Build a `SourcePackageMetadataChanged` unsat from a per-spec
/// conversion error, so the spec stringify call sites stay one-liners.
fn spec_unsat(name: &PackageName, err: &SpecConversionError) -> Box<PlatformUnsat> {
    Box::new(PlatformUnsat::SourcePackageMetadataChanged(
        name.as_source().to_string(),
        err.to_string(),
    ))
}

/// Drop unresolvable partial source records and clone the rest into a
/// `Vec<PixiRecord>` whose lifetime can back the
/// [`PinCompatibilityMap`].
fn resolved_records(unresolved: &[pixi_record::UnresolvedPixiRecord]) -> Vec<PixiRecord> {
    unresolved
        .iter()
        .filter_map(|r| r.clone().try_into_resolved().ok())
        .collect()
}

/// Pick the records that match a backend-declared dependency name, drop
/// anything in `ignore_run_exports.from_package`, and turn each
/// surviving record's `RunExportsJson` into the typed
/// [`PixiRunExports`] flavour the dispatcher's run-export merger
/// expects.
fn collect_direct_run_exports(
    direct: Option<&pixi_build_types::procedures::conda_outputs::CondaOutputDependencies>,
    locked: &[pixi_record::UnresolvedPixiRecord],
    ignore: &pixi_build_types::procedures::conda_outputs::CondaOutputIgnoreRunExports,
) -> Vec<(PackageName, PixiRunExports)> {
    let direct_names: HashSet<PackageName> = direct
        .into_iter()
        .flat_map(|d| {
            d.depends
                .iter()
                .filter_map(|named| PackageName::try_from(named.name.as_str()).ok())
        })
        .collect();
    let mut out = Vec::new();
    for record in locked {
        let name = record.name();
        if !direct_names.contains(name) || ignore.from_package.contains(name) {
            continue;
        }
        let Some(re_json) = record
            .as_binary()
            .and_then(|b| b.package_record.run_exports.as_ref())
        else {
            continue;
        };
        let pixi_re = PixiRunExports {
            noarch: filter_match_specs(&re_json.noarch, ignore),
            strong: filter_match_specs(&re_json.strong, ignore),
            weak: filter_match_specs(&re_json.weak, ignore),
            strong_constrains: filter_match_specs(&re_json.strong_constrains, ignore),
            weak_constrains: filter_match_specs(&re_json.weak_constrains, ignore),
        };
        out.push((name.clone(), pixi_re));
    }
    out
}

/// Compare two `Vec<String>` as multisets. Order is not semantically
/// meaningful for `depends` / `constrains`: the solver consumes the
/// list as a set of specs, and the order in the locked record only
/// reflects the iteration order of the producing `DependencyMap` at
/// solve time. Two paths (live solve vs lock-file readback) can land
/// on different iteration orders for the same set of run-exports,
/// so requiring an exact sequence match would surface spurious drift
/// every time a record-source iterator returned a different order.
///
/// Returns `Ok(())` when the two sides cover the same multiset of
/// specs, otherwise a [`DepDiff`] naming the symmetric difference.
pub(crate) fn diff_dep_sequences(locked: &[String], expected: &[String]) -> Result<(), DepDiff> {
    if locked.len() == expected.len() && locked == expected {
        return Ok(());
    }
    let mut counts: std::collections::HashMap<&str, isize> = std::collections::HashMap::new();
    for s in expected {
        *counts.entry(s.as_str()).or_default() += 1;
    }
    for s in locked {
        *counts.entry(s.as_str()).or_default() -= 1;
    }
    let mut added: Vec<String> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    for (spec, delta) in counts {
        if delta > 0 {
            for _ in 0..delta {
                added.push(spec.to_string());
            }
        } else if delta < 0 {
            for _ in 0..(-delta) {
                removed.push(spec.to_string());
            }
        }
    }
    if added.is_empty() && removed.is_empty() {
        return Ok(());
    }
    added.sort();
    removed.sort();
    Err(DepDiff { added, removed })
}

/// Symmetric multiset diff between locked and re-derived dependency
/// sequences.
#[derive(Debug)]
pub(crate) struct DepDiff {
    /// Specs the backend now declares but the lockfile lacks.
    pub(crate) added: Vec<String>,
    /// Specs the lockfile carries but the backend no longer declares.
    pub(crate) removed: Vec<String>,
}

impl DepDiff {
    /// Convert the diff into the `PlatformUnsat` variant.
    pub(crate) fn into_unsat(self, package: &PackageName, kind: SourceRunDepKind) -> Box<PlatformUnsat> {
        Box::new(PlatformUnsat::SourceRunDependenciesChanged {
            package: package.as_source().to_string(),
            kind,
            added: self.added,
            removed: self.removed,
        })
    }
}

/// Compare a locked record's variants (`pixi_record::VariantValue`)
/// against a backend output's variants (`pixi_build_types::VariantValue`).
///
/// The synthetic `target_platform` key is ignored on both sides:
/// older lock files often omit it and backends sometimes auto-inject
/// it, so requiring an exact match would invalidate locks that the
/// solver previously produced. Every other key must agree on both
/// sides; equality goes through `pixi_variant`'s `From` impl so the
/// two representations align.
pub(crate) fn variants_equivalent(
    locked: &std::collections::BTreeMap<String, pixi_record::VariantValue>,
    backend: &std::collections::BTreeMap<String, pixi_build_types::VariantValue>,
) -> bool {
    let is_real = |k: &str| k != "target_platform";
    let locked_count = locked.keys().filter(|k| is_real(k)).count();
    let backend_count = backend.keys().filter(|k| is_real(k)).count();
    if locked_count != backend_count {
        return false;
    }
    locked.iter().filter(|(k, _)| is_real(k)).all(|(k, v)| {
        backend
            .get(k)
            .is_some_and(|other| v == &pixi_record::VariantValue::from(other.clone()))
    })
}

fn format_variants(
    variants: &std::collections::BTreeMap<String, pixi_record::VariantValue>,
) -> String {
    if variants.is_empty() {
        return "<none>".to_string();
    }
    variants
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Name-indexed view over a slice of `UnresolvedPixiRecord`. Three of
/// the four queries the satisfiability path runs (`satisfies_binary`,
/// `satisfies_source`, `find_package_record`) start by filtering on
/// name, so we build the index once and let each query do an O(1)
/// HashMap lookup followed by a per-candidate predicate.
///
/// The same surface area is what the main-environment walker
/// re-implements over `PixiRecordsByName` (for the already-resolved
/// case): when that walker is one day rewritten to speak directly to
/// `UnresolvedPixiRecord`, both call sites can share this view.
struct LockedConda<'a> {
    /// Multiple records may share a name (different builds of the
    /// same package), hence the `Vec`.
    by_name: std::collections::HashMap<&'a PackageName, Vec<&'a pixi_record::UnresolvedPixiRecord>>,
}

impl<'a> LockedConda<'a> {
    fn new(records: &'a [pixi_record::UnresolvedPixiRecord]) -> Self {
        let mut by_name: std::collections::HashMap<
            &'a PackageName,
            Vec<&'a pixi_record::UnresolvedPixiRecord>,
        > = std::collections::HashMap::new();
        for record in records {
            by_name.entry(record.name()).or_default().push(record);
        }
        Self { by_name }
    }

    fn records_for(&self, name: &PackageName) -> &[&'a pixi_record::UnresolvedPixiRecord] {
        self.by_name.get(name).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Returns `true` if any locked record with `name` satisfies
    /// `spec` (matched against the record's `PackageRecord`).
    ///
    /// `NamelessMatchSpec` doesn't carry the package name, so the
    /// caller's `name` is the source of truth.
    ///
    /// Locked partial source records match by name only: their
    /// version/build aren't materialized in the lockfile, but they
    /// will be re-evaluated when the solver runs, so accepting them
    /// here avoids spurious unsat for the deferred case.
    fn satisfies_binary(&self, name: &PackageName, spec: &NamelessMatchSpec) -> bool {
        self.records_for(name).iter().any(|r| match r {
            pixi_record::UnresolvedPixiRecord::Binary(b) => spec.matches(b.as_ref()),
            pixi_record::UnresolvedPixiRecord::Source(s) => match &s.data {
                SourceRecordData::Full(full) => spec.matches(&full.package_record),
                SourceRecordData::Partial(_) => true,
            },
        })
    }

    /// Returns `true` if any locked source record with the given
    /// `name` has a pinned manifest source compatible with
    /// `location` (per [`PinnedSourceSpec::matches_source_spec`]).
    fn satisfies_source(&self, name: &PackageName, location: &SourceLocationSpec) -> bool {
        self.records_for(name).iter().any(|r| match r {
            pixi_record::UnresolvedPixiRecord::Source(s) => {
                s.manifest_source.matches_source_spec(location)
            }
            _ => false,
        })
    }

    /// Returns the resolved `PackageRecord` of the first locked record
    /// whose name matches. Returns `None` for partial source records,
    /// which carry no version/build material the satisfiability check
    /// can apply `pin_compatible` against.
    fn find_package_record(
        &self,
        name: &PackageName,
    ) -> Option<&rattler_conda_types::PackageRecord> {
        self.records_for(name)
            .iter()
            .find_map(|r| r.package_record())
    }
}

/// Verify that every backend-declared dep in `deps` is satisfied by
/// some record in `locked`. Spec kinds are handled as follows:
///
/// - `Binary`: convert to a `NamelessMatchSpec` and accept any locked
///   record whose `PackageRecord` matches.
/// - `Source`: accept the first locked source record with the same
///   name whose pinned manifest source matches the resolved location.
/// - `PinCompatible`: resolve the pin against `pin_compatible_locked`
///   (the env that pin_compatible looks up — the build env when
///   verifying host deps, empty when verifying build deps), then
///   verify the resolved spec against `locked`.
///
/// Returns `Box<PlatformUnsat>` directly so the caller can choose how
/// to wrap (`CommandDispatcherError::Failed`, or propagated as part of
/// a larger result).
pub(crate) fn verify_locked_against_backend_specs(
    deps: &pixi_build_types::procedures::conda_outputs::CondaOutputDependencies,
    locked: &[pixi_record::UnresolvedPixiRecord],
    pin_compatible_locked: &[pixi_record::UnresolvedPixiRecord],
    channel_config: &rattler_conda_types::ChannelConfig,
    source_anchor: &SourceAnchor,
    package: &PackageName,
    env: BuildOrHostEnv,
) -> Result<(), Box<PlatformUnsat>> {
    use pixi_build_types::PackageSpec;
    use pixi_command_dispatcher::build::conversion::{from_binary_spec_v1, from_source_spec_v1};
    use pixi_spec::Pin;

    let locked_view = LockedConda::new(locked);
    let pin_view = LockedConda::new(pin_compatible_locked);
    let unsat = |spec: String| -> Box<PlatformUnsat> {
        Box::new(PlatformUnsat::SourceBuildHostUnsat {
            package: package.as_source().to_string(),
            env,
            spec,
            locked: format_locked_summary(locked),
        })
    };

    for dep in &deps.depends {
        // Backend-supplied package names are unchecked strings; trust
        // them via `new_unchecked` so we don't need a parse-error
        // PlatformUnsat variant for shapes that should never occur in
        // practice (the backend wouldn't have produced them).
        let dep_name = PackageName::new_unchecked(dep.name.as_str().to_string());

        match &dep.spec {
            PackageSpec::Binary(binary) => {
                let nameless = from_binary_spec_v1(binary.clone())
                    .try_into_nameless_match_spec(channel_config)
                    .map_err(|e| {
                        failed_to_parse_match_spec_unsat(
                            dep.name.as_str(),
                            spec_conversion_to_match_spec_error(e),
                        )
                    })?;
                if !locked_view.satisfies_binary(&dep_name, &nameless) {
                    let match_spec = MatchSpec::from_nameless(nameless, dep_name.clone().into());
                    return Err(unsat(match_spec.to_string()));
                }
            }
            PackageSpec::Source(source) => {
                let resolved = from_source_spec_v1(source.clone()).resolve(source_anchor);
                if !locked_view.satisfies_source(&dep_name, &resolved.location) {
                    return Err(Box::new(PlatformUnsat::SourceBuildHostSourceMissing {
                        package: package.as_source().to_string(),
                        env,
                        name: dep_name.as_source().to_string(),
                        location: resolved.location.to_string(),
                    }));
                }
            }
            PackageSpec::PinCompatible(pin) => {
                // pin_compatible's compatibility env is the build env
                // when verifying host deps, and empty when verifying
                // build deps; either way the resolved version comes
                // from `pin_compatible_locked`, not `locked`.
                let Some(pin_record) = pin_view.find_package_record(&dep_name) else {
                    return Err(unsat(format!(
                        "{} (pin_compatible: not resolved in build env)",
                        dep_name.as_source()
                    )));
                };
                let pin = Pin::try_from(pin.clone()).map_err(|err| {
                    unsat(format!("{} (pin_compatible: {err})", dep_name.as_source()))
                })?;
                let resolved = pin
                    .resolve(&pin_record.version, &pin_record.build)
                    .map_err(|err| {
                        unsat(format!("{} (pin_compatible: {err})", dep_name.as_source()))
                    })?;
                let nameless = resolved
                    .try_into_nameless_match_spec(channel_config)
                    .map_err(|e| {
                        failed_to_parse_match_spec_unsat(
                            dep.name.as_str(),
                            spec_conversion_to_match_spec_error(e),
                        )
                    })?
                    .expect("pin_compatible always produces a binary spec");
                if !locked_view.satisfies_binary(&dep_name, &nameless) {
                    let match_spec = MatchSpec::from_nameless(nameless, dep_name.clone().into());
                    return Err(unsat(format!("{match_spec} (pin_compatible)")));
                }
            }
        }
    }

    Ok(())
}

fn format_locked_summary(locked: &[pixi_record::UnresolvedPixiRecord]) -> String {
    if locked.is_empty() {
        return "<empty>".to_string();
    }
    let names: Vec<String> = locked
        .iter()
        .map(|r| match r {
            pixi_record::UnresolvedPixiRecord::Binary(b) => {
                let pr = &b.package_record;
                format!("{}={}={}", pr.name.as_source(), pr.version, pr.build)
            }
            pixi_record::UnresolvedPixiRecord::Source(s) => match &s.data {
                SourceRecordData::Full(full) => format!(
                    "{}={}={}",
                    full.package_record.name.as_source(),
                    full.package_record.version,
                    full.package_record.build
                ),
                SourceRecordData::Partial(p) => format!("{}=<partial>", p.name.as_source()),
            },
        })
        .collect();
    names.join(", ")
}

/// Build a fresh [`pixi_record::SourceRecord`] for downstream
/// verification.
///
/// The backend output supplies the parts that change with a re-build
/// (version, build string / number, license metadata, subdir, etc.),
/// while the locked record supplies its already-resolved run-time
/// `depends` and `constrains`: those carry run-export contributions
/// from the previous solve's build/host envs that the bare
/// `output.run_dependencies` cannot reproduce here. The locked
/// `manifest_source`, `build_source`, `build_packages`, and
/// `host_packages` are preserved verbatim so re-locking sees the same
/// pinned source and the same build/host snapshot.
pub(crate) fn build_full_source_record_from_output(
    record: &pixi_record::UnresolvedSourceRecord,
    output: &pixi_build_types::procedures::conda_outputs::CondaOutput,
) -> pixi_record::SourceRecord {
    use pixi_record::{FullSourceRecordData, SourceRecord as FullRecord};
    use rattler_conda_types::PackageRecord;

    // Reuse the locked record's resolved depends/constrains when
    // available. For a partial-only record the lockfile carried
    // `depends` but not `constrains`, so default constrains to empty.
    let (depends, constrains): (Vec<String>, Vec<String>) = match &record.data {
        SourceRecordData::Full(full) => (
            full.package_record.depends.clone(),
            full.package_record.constrains.clone(),
        ),
        SourceRecordData::Partial(partial) => (partial.depends.clone(), Vec::new()),
    };
    let package_record = PackageRecord {
        size: None,
        sha256: None,
        md5: None,
        timestamp: None,
        platform: output
            .metadata
            .subdir
            .only_platform()
            .map(ToString::to_string),
        arch: output
            .metadata
            .subdir
            .arch()
            .as_ref()
            .map(ToString::to_string),
        name: output.metadata.name.clone(),
        build: output.metadata.build.clone(),
        version: output.metadata.version.clone(),
        build_number: output.metadata.build_number,
        license: output.metadata.license.clone(),
        subdir: output.metadata.subdir.to_string(),
        license_family: output.metadata.license_family.clone(),
        noarch: output.metadata.noarch,
        constrains,
        depends,
        run_exports: None,
        purls: output
            .metadata
            .purls
            .as_ref()
            .map(|purls| purls.iter().cloned().collect()),
        python_site_packages_path: output.metadata.python_site_packages_path.clone(),
        features: None,
        track_features: vec![],
        legacy_bz2_md5: None,
        legacy_bz2_size: None,
        experimental_extra_depends: Default::default(),
        flags: Default::default(),
    };
    let sources: std::collections::BTreeMap<String, SourceLocationSpec> = match &record.data {
        SourceRecordData::Full(full) => full.sources.clone(),
        SourceRecordData::Partial(partial) => partial.sources.clone(),
    };
    FullRecord {
        data: FullSourceRecordData {
            package_record,
            sources,
        },
        manifest_source: record.manifest_source.clone(),
        build_source: record.build_source.clone(),
        variants: record.variants.clone(),
        identifier_hash: record.identifier_hash.clone(),
        build_packages: record.build_packages.clone(),
        host_packages: record.host_packages.clone(),
    }
}
