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
fn verify_locked_run_deps_against_backend(
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
fn diff_dep_sequences(locked: &[String], expected: &[String]) -> Result<(), DepDiff> {
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
struct DepDiff {
    /// Specs the backend now declares but the lockfile lacks.
    added: Vec<String>,
    /// Specs the lockfile carries but the backend no longer declares.
    removed: Vec<String>,
}

impl DepDiff {
    /// Convert the diff into the `PlatformUnsat` variant.
    fn into_unsat(self, package: &PackageName, kind: SourceRunDepKind) -> Box<PlatformUnsat> {
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
fn variants_equivalent(
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
fn verify_locked_against_backend_specs(
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
fn build_full_source_record_from_output(
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

#[cfg(test)]
mod tests {
    use super::super::BuildOrHostEnv;
    use super::{
        build_full_source_record_from_output, variants_equivalent,
        verify_locked_against_backend_specs,
    };
    use pixi_build_types::{
        BinaryPackageSpec, NamedSpec, PackageSpec, PinCompatibleSpec, SourcePackageName,
        VariantValue,
        procedures::conda_outputs::{
            CondaOutput, CondaOutputDependencies, CondaOutputIgnoreRunExports,
            CondaOutputMetadata, CondaOutputRunExports,
        },
    };
    use pixi_record::{
        PartialSourceRecordData, PinnedPathSpec, PinnedSourceSpec, SourceRecordData,
        UnresolvedPixiRecord, UnresolvedSourceRecord,
    };
    use pixi_spec::{SourceAnchor, SourceLocationSpec};
    use rattler_conda_types::{
        ChannelConfig, NoArchType, PackageName, PackageRecord, Platform, RepoDataRecord,
        VersionSpec, VersionWithSource, package::DistArchiveIdentifier,
    };
    use std::{
        collections::BTreeMap,
        path::PathBuf,
        str::FromStr,
        sync::{Arc, LazyLock},
    };
    use url::Url;

    static CHANNEL_CONFIG: LazyLock<ChannelConfig> =
        LazyLock::new(|| ChannelConfig::default_with_root_dir(PathBuf::from("/workspace")));

    fn make_binary_record(name: &str, version: &str) -> RepoDataRecord {
        let pkg_name = PackageName::from_str(name).expect("valid name");
        let mut pr = PackageRecord::new(
            pkg_name,
            VersionWithSource::from_str(version).expect("valid version"),
            "h0".into(),
        );
        pr.subdir = "linux-64".into();
        let file_name = format!("{name}-{version}-h0.conda");
        RepoDataRecord {
            package_record: pr,
            identifier: DistArchiveIdentifier::from_str(&file_name)
                .expect("valid dist archive identifier"),
            url: Url::parse(&format!(
                "https://example.com/conda-forge/linux-64/{file_name}"
            ))
            .expect("valid url"),
            channel: Some("https://example.com/conda-forge".to_string()),
        }
    }

    fn binary_dep(name: &str, spec_str: &str) -> NamedSpec<PackageSpec> {
        let spec = if spec_str.is_empty() {
            BinaryPackageSpec::default()
        } else {
            BinaryPackageSpec {
                version: Some(
                    VersionSpec::from_str(
                        spec_str,
                        rattler_conda_types::ParseStrictness::Lenient,
                    )
                    .expect("valid spec"),
                ),
                ..Default::default()
            }
        };
        NamedSpec {
            name: SourcePackageName::from(PackageName::from_str(name).expect("valid name")),
            spec: PackageSpec::Binary(spec),
        }
    }

    fn pin_compatible_dep(name: &str) -> NamedSpec<PackageSpec> {
        pin_compatible_dep_with(
            name,
            PinCompatibleSpec {
                lower_bound: None,
                upper_bound: None,
                exact: false,
                build: None,
            },
        )
    }

    fn pin_compatible_dep_with(name: &str, spec: PinCompatibleSpec) -> NamedSpec<PackageSpec> {
        NamedSpec {
            name: SourcePackageName::from(PackageName::from_str(name).expect("valid name")),
            spec: PackageSpec::PinCompatible(spec),
        }
    }

    fn make_partial_source_record(
        name: &str,
        manifest_path: &str,
        build_packages: Vec<UnresolvedPixiRecord>,
        host_packages: Vec<UnresolvedPixiRecord>,
    ) -> UnresolvedSourceRecord {
        UnresolvedSourceRecord {
            data: SourceRecordData::Partial(PartialSourceRecordData {
                name: PackageName::from_str(name).unwrap(),
                depends: Vec::new(),
                constrains: Vec::new(),
                experimental_extra_depends: Default::default(),
                flags: Default::default(),
                purls: None,
                run_exports: None,
                sources: Default::default(),
            }),
            manifest_source: PinnedSourceSpec::Path(PinnedPathSpec {
                path: manifest_path.into(),
            }),
            build_source: None,
            variants: Default::default(),
            identifier_hash: None,
            build_packages,
            host_packages,
        }
    }

    fn make_conda_output(name: &str, build_deps: Vec<NamedSpec<PackageSpec>>) -> CondaOutput {
        CondaOutput {
            metadata: CondaOutputMetadata {
                name: PackageName::from_str(name).unwrap(),
                version: "1.0.0"
                    .parse::<rattler_conda_types::Version>()
                    .unwrap()
                    .into(),
                build: "h0_0".to_string(),
                build_number: 0,
                subdir: Platform::Linux64,
                license: None,
                license_family: None,
                noarch: NoArchType::none(),
                purls: None,
                python_site_packages_path: None,
                variant: BTreeMap::new(),
            },
            build_dependencies: Some(CondaOutputDependencies {
                depends: build_deps,
                constraints: Vec::new(),
            }),
            host_dependencies: None,
            run_dependencies: CondaOutputDependencies {
                depends: Vec::new(),
                constraints: Vec::new(),
            },
            ignore_run_exports: CondaOutputIgnoreRunExports::default(),
            run_exports: CondaOutputRunExports::default(),
            input_globs: None,
        }
    }

    #[test]
    fn variants_equivalent_ignores_target_platform() {
        // Locked record with no variants vs backend output that
        // injected `target_platform=linux-64`: they should still
        // count as equivalent so older lock files (which omit the
        // synthetic key) keep matching.
        let locked = BTreeMap::new();
        let mut backend = BTreeMap::new();
        backend.insert(
            "target_platform".to_string(),
            VariantValue::String("linux-64".to_string()),
        );
        assert!(variants_equivalent(&locked, &backend));
    }

    #[test]
    fn variants_equivalent_real_keys_must_match() {
        let mut locked = BTreeMap::new();
        locked.insert(
            "python".to_string(),
            pixi_record::VariantValue::String("3.11".to_string()),
        );
        let mut backend = BTreeMap::new();
        backend.insert(
            "python".to_string(),
            VariantValue::String("3.10".to_string()),
        );
        assert!(!variants_equivalent(&locked, &backend));
    }

    #[test]
    fn locked_build_satisfies_backend_spec_passes() {
        // Backend declares `numpy >=1`; locked build_packages
        // contains numpy 1.5. Verification should pass.
        let locked: Vec<UnresolvedPixiRecord> = vec![UnresolvedPixiRecord::Binary(Arc::new(
            make_binary_record("numpy", "1.5"),
        ))];
        let deps = CondaOutputDependencies {
            depends: vec![binary_dep("numpy", ">=1")],
            constraints: Vec::new(),
        };
        let anchor = SourceAnchor::from(SourceLocationSpec::from(PinnedSourceSpec::Path(
            PinnedPathSpec {
                path: "./pkg".into(),
            },
        )));
        let result = verify_locked_against_backend_specs(
            &deps,
            &locked,
            &[],
            &CHANNEL_CONFIG,
            &anchor,
            &PackageName::from_str("pkg").unwrap(),
            BuildOrHostEnv::Build,
        );
        assert!(result.is_ok(), "verification should pass: {result:?}");
    }

    #[test]
    fn locked_build_does_not_satisfy_backend_spec_fails() {
        // Backend declares `numpy >=2`; locked has numpy 1.5. Must
        // surface `SourceBuildHostUnsat` so the caller knows which
        // spec drifted.
        let locked: Vec<UnresolvedPixiRecord> = vec![UnresolvedPixiRecord::Binary(Arc::new(
            make_binary_record("numpy", "1.5"),
        ))];
        let deps = CondaOutputDependencies {
            depends: vec![binary_dep("numpy", ">=2")],
            constraints: Vec::new(),
        };
        let anchor = SourceAnchor::from(SourceLocationSpec::from(PinnedSourceSpec::Path(
            PinnedPathSpec {
                path: "./pkg".into(),
            },
        )));
        let err = verify_locked_against_backend_specs(
            &deps,
            &locked,
            &[],
            &CHANNEL_CONFIG,
            &anchor,
            &PackageName::from_str("pkg").unwrap(),
            BuildOrHostEnv::Build,
        )
        .expect_err("locked numpy=1.5 must not satisfy >=2");
        assert!(
            matches!(
                *err,
                super::super::PlatformUnsat::SourceBuildHostUnsat { .. }
            ),
            "expected SourceBuildHostUnsat, got: {err}"
        );
    }

    /// Regression: an early version of `LockedConda::satisfies_binary`
    /// matched a `NamelessMatchSpec` against a `RepoDataRecord`
    /// without checking the package name first. With a wildcard
    /// spec like `bar *`, every locked binary record (including
    /// `numpy 1.5`) was reported as satisfying it. The check now
    /// requires the record's name to match the spec's caller-
    /// supplied name.
    #[test]
    fn wrong_name_record_does_not_satisfy_binary_spec() {
        // Backend wants `bar *`. Locked has only `foo 1.5`.
        // A name-blind matcher would falsely accept `foo 1.5`.
        let locked: Vec<UnresolvedPixiRecord> = vec![UnresolvedPixiRecord::Binary(Arc::new(
            make_binary_record("foo", "1.5"),
        ))];
        let deps = CondaOutputDependencies {
            depends: vec![binary_dep("bar", "")],
            constraints: Vec::new(),
        };
        let anchor = SourceAnchor::from(SourceLocationSpec::from(PinnedSourceSpec::Path(
            PinnedPathSpec {
                path: "./pkg".into(),
            },
        )));
        let err = verify_locked_against_backend_specs(
            &deps,
            &locked,
            &[],
            &CHANNEL_CONFIG,
            &anchor,
            &PackageName::from_str("pkg").unwrap(),
            BuildOrHostEnv::Build,
        )
        .expect_err("name mismatch must surface as unsat");
        assert!(
            matches!(
                *err,
                super::super::PlatformUnsat::SourceBuildHostUnsat { .. }
            ),
            "expected SourceBuildHostUnsat, got: {err}"
        );
    }

    #[test]
    fn missing_required_record_in_locked_build_fails() {
        // Backend wants `cmake` in build env; locked build is
        // empty. Must report `SourceBuildHostUnsat` rather than
        // silently passing.
        let locked: Vec<UnresolvedPixiRecord> = Vec::new();
        let deps = CondaOutputDependencies {
            depends: vec![binary_dep("cmake", "")],
            constraints: Vec::new(),
        };
        let anchor = SourceAnchor::from(SourceLocationSpec::from(PinnedSourceSpec::Path(
            PinnedPathSpec {
                path: "./pkg".into(),
            },
        )));
        let err = verify_locked_against_backend_specs(
            &deps,
            &locked,
            &[],
            &CHANNEL_CONFIG,
            &anchor,
            &PackageName::from_str("pkg").unwrap(),
            BuildOrHostEnv::Build,
        )
        .expect_err("missing record must surface as unsat");
        assert!(
            matches!(
                *err,
                super::super::PlatformUnsat::SourceBuildHostUnsat { .. }
            ),
            "expected SourceBuildHostUnsat, got: {err}"
        );
    }

    #[test]
    fn build_full_source_record_preserves_locked_depends_and_pin() {
        // Locked partial record with non-trivial depends. The
        // backend output reports a fresh version/build but no
        // run_deps. The synthesized full record must keep the
        // locked depends (which carries previously-resolved
        // run-exports) and the locked manifest pin verbatim.
        let mut partial =
            make_partial_source_record("mypkg", "./mypkg", Vec::new(), Vec::new());
        // Hand-set locked depends so the assertion has something
        // distinctive to compare.
        if let SourceRecordData::Partial(p) = &mut partial.data {
            p.depends = vec!["numpy >=1".to_string(), "openssl 3.0.*".to_string()];
        }

        let output = make_conda_output("mypkg", Vec::new());
        let full = build_full_source_record_from_output(&partial, &output);
        assert_eq!(
            full.data.package_record.depends,
            vec!["numpy >=1".to_string(), "openssl 3.0.*".to_string()],
            "locked depends must survive into the synthesized full record"
        );
        assert_eq!(full.manifest_source, partial.manifest_source);
    }

    /// `pin_compatible(foo)` in *host* dependencies pins against
    /// the version of `foo` resolved in the *build* environment.
    /// If the locked build env has no `foo`, no re-solve can
    /// succeed (the resolver would fail with
    /// `PinCompatibleError::PackageNotFound`), so the lock must
    /// be rejected even when the host env happens to carry a
    /// `foo` from another dep.
    #[test]
    fn pin_compatible_host_dep_rejects_when_build_lacks_package() {
        let host_locked: Vec<UnresolvedPixiRecord> = vec![UnresolvedPixiRecord::Binary(
            Arc::new(make_binary_record("numpy", "1.5")),
        )];
        let build_locked: Vec<UnresolvedPixiRecord> = Vec::new();

        let host_deps = CondaOutputDependencies {
            depends: vec![pin_compatible_dep("numpy")],
            constraints: Vec::new(),
        };
        let anchor = SourceAnchor::from(SourceLocationSpec::from(PinnedSourceSpec::Path(
            PinnedPathSpec {
                path: "./pkg".into(),
            },
        )));

        let err = verify_locked_against_backend_specs(
            &host_deps,
            &host_locked,
            &build_locked,
            &CHANNEL_CONFIG,
            &anchor,
            &PackageName::from_str("pkg").unwrap(),
            BuildOrHostEnv::Host,
        )
        .expect_err(
            "pin_compatible(numpy) must resolve against the (empty) build env, \
             not the host env that happens to contain numpy",
        );
        assert!(
            matches!(
                *err,
                super::super::PlatformUnsat::SourceBuildHostUnsat { .. }
            ),
            "expected SourceBuildHostUnsat, got: {err}"
        );
    }

    /// Happy path: locked build env has `numpy 1.5`, locked host
    /// env also has `numpy 1.5`, host dep is `pin_compatible(numpy)`
    /// with no bounds (resolves to `*`). Verification passes.
    #[test]
    fn pin_compatible_host_dep_satisfied() {
        let host_locked: Vec<UnresolvedPixiRecord> = vec![UnresolvedPixiRecord::Binary(
            Arc::new(make_binary_record("numpy", "1.5")),
        )];
        let build_locked: Vec<UnresolvedPixiRecord> = vec![UnresolvedPixiRecord::Binary(
            Arc::new(make_binary_record("numpy", "1.5")),
        )];

        let host_deps = CondaOutputDependencies {
            depends: vec![pin_compatible_dep("numpy")],
            constraints: Vec::new(),
        };
        let anchor = SourceAnchor::from(SourceLocationSpec::from(PinnedSourceSpec::Path(
            PinnedPathSpec {
                path: "./pkg".into(),
            },
        )));

        let result = verify_locked_against_backend_specs(
            &host_deps,
            &host_locked,
            &build_locked,
            &CHANNEL_CONFIG,
            &anchor,
            &PackageName::from_str("pkg").unwrap(),
            BuildOrHostEnv::Host,
        );
        assert!(result.is_ok(), "verification should pass: {result:?}");
    }

    /// Resolution-then-verification: build env has `numpy 2.0`, the
    /// pin is `exact=true`, and host env still carries `numpy 1.5`
    /// from before the user bumped the build env. The resolved
    /// spec is `numpy ==2.0`, which the locked host record does
    /// not satisfy.
    #[test]
    fn pin_compatible_host_dep_rejects_version_drift() {
        use pixi_build_types::PinCompatibleSpec;

        let host_locked: Vec<UnresolvedPixiRecord> = vec![UnresolvedPixiRecord::Binary(
            Arc::new(make_binary_record("numpy", "1.5")),
        )];
        let build_locked: Vec<UnresolvedPixiRecord> = vec![UnresolvedPixiRecord::Binary(
            Arc::new(make_binary_record("numpy", "2.0")),
        )];

        let host_deps = CondaOutputDependencies {
            depends: vec![pin_compatible_dep_with(
                "numpy",
                PinCompatibleSpec {
                    lower_bound: None,
                    upper_bound: None,
                    exact: true,
                    build: None,
                },
            )],
            constraints: Vec::new(),
        };
        let anchor = SourceAnchor::from(SourceLocationSpec::from(PinnedSourceSpec::Path(
            PinnedPathSpec {
                path: "./pkg".into(),
            },
        )));

        let err = verify_locked_against_backend_specs(
            &host_deps,
            &host_locked,
            &build_locked,
            &CHANNEL_CONFIG,
            &anchor,
            &PackageName::from_str("pkg").unwrap(),
            BuildOrHostEnv::Host,
        )
        .expect_err(
            "host's locked numpy 1.5 cannot satisfy pin_compatible(numpy, exact) \
             against build's numpy 2.0",
        );
        assert!(
            matches!(
                *err,
                super::super::PlatformUnsat::SourceBuildHostUnsat { .. }
            ),
            "expected SourceBuildHostUnsat, got: {err}"
        );
    }

    // -- Unit tests for run-dependency / run-constraint drift -----------

    use super::super::SourceRunDepKind;
    use super::{diff_dep_sequences, verify_locked_run_deps_against_backend};
    use pixi_record::FullSourceRecordData;

    #[test]
    fn diff_sequences_passes_when_equal() {
        let result = diff_dep_sequences(
            &["a >=1".to_string(), "b ==2".to_string()],
            &["a >=1".to_string(), "b ==2".to_string()],
        );
        assert!(
            result.is_ok(),
            "identical sequences should not drift: {result:?}"
        );
    }

    #[test]
    fn diff_sequences_ignores_reorder() {
        // Same multiset, different order. Order is not semantically
        // meaningful; only the symmetric multiset difference matters.
        let result = diff_dep_sequences(
            &["a >=1".to_string(), "b ==2".to_string()],
            &["b ==2".to_string(), "a >=1".to_string()],
        );
        assert!(
            result.is_ok(),
            "permutations must not surface as drift: {result:?}"
        );
    }

    #[test]
    fn diff_sequences_reports_only_addition() {
        let diff = diff_dep_sequences(
            &["a >=1".to_string()],
            &["a >=1".to_string(), "b ==2".to_string()],
        )
        .expect_err("expected drift");
        assert_eq!(diff.added, vec!["b ==2".to_string()]);
        assert!(diff.removed.is_empty());
    }

    #[test]
    fn diff_sequences_reports_only_removal() {
        let diff = diff_dep_sequences(
            &["a >=1".to_string(), "b ==2".to_string()],
            &["a >=1".to_string()],
        )
        .expect_err("expected drift");
        assert!(diff.added.is_empty());
        assert_eq!(diff.removed, vec!["b ==2".to_string()]);
    }

    #[test]
    fn diff_sequences_reports_both_directions() {
        let diff = diff_dep_sequences(
            &["a >=1".to_string(), "b ==2".to_string()],
            &["a >=1".to_string(), "c <=3".to_string()],
        )
        .expect_err("expected drift");
        assert_eq!(diff.added, vec!["c <=3".to_string()]);
        assert_eq!(diff.removed, vec!["b ==2".to_string()]);
    }

    #[test]
    fn diff_sequences_treats_duplicates_as_distinct() {
        // Locked carries the same spec twice but the expected set
        // only carries it once; the extra copy must surface as a
        // removal.
        let diff = diff_dep_sequences(
            &["a >=1".to_string(), "a >=1".to_string()],
            &["a >=1".to_string()],
        )
        .expect_err("expected drift");
        assert!(diff.added.is_empty());
        assert_eq!(diff.removed, vec!["a >=1".to_string()]);
    }

    /// Build a Full source record with the supplied `depends` and
    /// `constrains` strings. Build/host packages are empty, which
    /// is enough for the constrains-only test cases below.
    fn make_full_source_record(
        name: &str,
        depends: Vec<String>,
        constrains: Vec<String>,
    ) -> UnresolvedSourceRecord {
        let pkg_name = PackageName::from_str(name).unwrap();
        let mut pr = PackageRecord::new(
            pkg_name.clone(),
            "1.0.0"
                .parse::<rattler_conda_types::VersionWithSource>()
                .unwrap(),
            "h0_0".into(),
        );
        pr.subdir = "linux-64".into();
        pr.depends = depends;
        pr.constrains = constrains;
        UnresolvedSourceRecord {
            data: SourceRecordData::Full(FullSourceRecordData {
                package_record: pr,
                sources: Default::default(),
            }),
            manifest_source: PinnedSourceSpec::Path(PinnedPathSpec {
                path: "./pkg".into(),
            }),
            build_source: None,
            variants: Default::default(),
            identifier_hash: None,
            build_packages: Vec::new(),
            host_packages: Vec::new(),
        }
    }

    /// Helper to build a `CondaOutput` whose `run_dependencies` has
    /// the given `depends` and `constraints`. Other fields default
    /// to empty.
    fn make_conda_output_with_run_deps(
        name: &str,
        depends: Vec<NamedSpec<PackageSpec>>,
        constraints: Vec<NamedSpec<pixi_build_types::ConstraintSpec>>,
    ) -> CondaOutput {
        let mut output = make_conda_output(name, Vec::new());
        output.build_dependencies = None;
        output.run_dependencies = CondaOutputDependencies {
            depends,
            constraints,
        };
        output
    }

    fn binary_constraint(
        name: &str,
        spec_str: &str,
    ) -> NamedSpec<pixi_build_types::ConstraintSpec> {
        NamedSpec {
            name: SourcePackageName::from(PackageName::from_str(name).unwrap()),
            spec: pixi_build_types::ConstraintSpec::Binary(BinaryPackageSpec {
                version: Some(
                    VersionSpec::from_str(
                        spec_str,
                        rattler_conda_types::ParseStrictness::Lenient,
                    )
                    .unwrap(),
                ),
                ..Default::default()
            }),
        }
    }

    #[test]
    fn verify_locked_run_deps_passes_when_match() {
        // Backend declares run_deps `numpy >=1` and constrains
        // `openssl ==3.0`; locked record has the same. No drift.
        let record = make_full_source_record(
            "pkg",
            vec!["numpy >=1".to_string()],
            vec!["openssl ==3.0".to_string()],
        );
        let output = make_conda_output_with_run_deps(
            "pkg",
            vec![binary_dep("numpy", ">=1")],
            vec![binary_constraint("openssl", "==3.0")],
        );

        let result = verify_locked_run_deps_against_backend(&record, &output, &CHANNEL_CONFIG);
        assert!(result.is_ok(), "expected no drift: {result:?}");
    }

    #[test]
    fn verify_locked_run_deps_detects_constrain_addition() {
        // Backend declares a new constrain `bar <2` that the locked
        // record does not carry. Drift surfaces with `kind =
        // RunConstrains` and `added = ["bar <2"]`.
        let record = make_full_source_record("pkg", Vec::new(), Vec::new());
        let output = make_conda_output_with_run_deps(
            "pkg",
            Vec::new(),
            vec![binary_constraint("bar", "<2")],
        );

        let err = verify_locked_run_deps_against_backend(&record, &output, &CHANNEL_CONFIG)
            .expect_err("backend declared a new constraint, locked has none");
        match *err {
            super::super::PlatformUnsat::SourceRunDependenciesChanged {
                kind: SourceRunDepKind::RunConstrains,
                added,
                removed,
                ..
            } => {
                assert_eq!(added, vec!["bar <2".to_string()]);
                assert!(removed.is_empty());
            }
            other => panic!("expected RunConstrains drift, got: {other}"),
        }
    }

    #[test]
    fn verify_locked_run_deps_detects_constrain_removal() {
        // Locked record carries a constrain that the backend no
        // longer declares. Drift surfaces with `removed`.
        let record = make_full_source_record("pkg", Vec::new(), vec!["bar <2".to_string()]);
        let output = make_conda_output_with_run_deps("pkg", Vec::new(), Vec::new());

        let err = verify_locked_run_deps_against_backend(&record, &output, &CHANNEL_CONFIG)
            .expect_err("backend dropped a constraint that's still locked");
        match *err {
            super::super::PlatformUnsat::SourceRunDependenciesChanged {
                kind: SourceRunDepKind::RunConstrains,
                added,
                removed,
                ..
            } => {
                assert!(added.is_empty());
                assert_eq!(removed, vec!["bar <2".to_string()]);
            }
            other => panic!("expected RunConstrains drift, got: {other}"),
        }
    }
}
