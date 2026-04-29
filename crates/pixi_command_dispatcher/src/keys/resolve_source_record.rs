//! Helpers for assembling a single [`SourceRecord`] from a backend
//! [`CondaOutput`] plus a pinned source location, called by
//! [`ResolveSourcePackageKey`](super::ResolveSourcePackageKey) once per
//! variant. [`nested_solve`] also installs the cycle guard for the
//! recursive `SolvePixiEnvironmentKey` -> RSP chain and renders any
//! detected cycle as `(package, CycleEnvironment)` frames.

use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use itertools::Either;
use pixi_build_types::procedures::conda_outputs::CondaOutput;
use pixi_compute_engine::ComputeCtx;
use pixi_record::{
    FullSourceRecordData, PinnedSourceSpec, PixiRecord, SourceRecord, UnresolvedPixiRecord,
};
use pixi_spec::{BinarySpec, PixiSpec, SourceAnchor, SourceLocationSpec};
use pixi_spec_containers::DependencyMap;
use pixi_variant::VariantValue;
use rattler_conda_types::{PackageName, PackageRecord, package::RunExportsJson};
use rattler_solve::SolveStrategy;

use crate::{
    BuildBackendMetadataSpec, DerivedEnvKind, EnvironmentRef, InstalledSourceHints, PtrArc,
    Reporter, ReporterContext, SourceRecordSpec,
    build::{Dependencies, PinnedSourceCodeLocation, PixiRunExports},
    compute_data::{HasGateway, HasReporter},
    injected_config::ChannelConfigKey,
    keys::solve_pixi_environment::{SolvePixiEnvironmentKey, SolvePixiEnvironmentSpec},
    reporter::{SourceRecordId, SourceRecordReporter},
    reporter_context::{CURRENT_REPORTER_CONTEXT, current_reporter_context},
    reporter_lifecycle::{Active, LifecycleKind, ReporterLifecycle},
    source_metadata::CycleEnvironment,
    source_record::SourceRecordError,
};

/// Resolve one variant's [`SourceRecord`] from an assembled
/// [`CondaOutput`] + pinned source location.
///
/// Mirrors the legacy `resolve_output` flow: solve the build env,
/// extract build run-exports, solve the host env, extract host
/// run-exports, assemble the final record with merged run
/// dependencies.
///
/// `preferred_build_source` is the FULL pin map, propagated verbatim
/// into the nested build/host env solves so pins for every package
/// in the subtree remain visible.
pub(super) async fn assemble_source_record(
    ctx: &mut ComputeCtx,
    source: &PinnedSourceCodeLocation,
    output: &CondaOutput,
    preferred_build_source: &Arc<BTreeMap<PackageName, PinnedSourceSpec>>,
    env_ref: &EnvironmentRef,
    installed_source_hints: &PtrArc<InstalledSourceHints>,
) -> Result<Arc<SourceRecord>, SourceRecordError> {
    // Reporter lifecycle for this variant's source-record assembly.
    // Build a `SourceRecordSpec` from the data flowing through here so
    // the reporter gets a familiar shape. Dedup fields
    // (`exclude_newer`) are set to `None`: the RSP path doesn't carry
    // that value directly and the reporter uses the spec only for
    // display.
    let reporter_spec = SourceRecordSpec {
        package: output.metadata.name.clone(),
        variants: output
            .metadata
            .variant
            .iter()
            .map(|(k, v)| (k.clone(), VariantValue::from(v.clone())))
            .collect(),
        backend_metadata: BuildBackendMetadataSpec {
            manifest_source: source.manifest_source().clone(),
            preferred_build_source: preferred_build_source.get(&output.metadata.name).cloned(),
            env_ref: env_ref.clone(),
        },
        exclude_newer: None,
    };
    let reporter_arc: Option<Arc<dyn Reporter>> = ctx.global_data().reporter().cloned();
    let parent_reporter_ctx = current_reporter_context();
    let lifecycle = ReporterLifecycle::<SourceRecordReporterLifecycle>::queued(
        reporter_arc.as_deref(),
        parent_reporter_ctx,
        &reporter_spec,
    );
    // Scope nested computes (build/host env `nested_solve`s) under
    // this source-record's reporter context so their events attribute
    // to the record being assembled.
    let scope_ctx = lifecycle
        .id()
        .map(ReporterContext::SourceRecord)
        .or(parent_reporter_ctx);
    let _lifecycle = lifecycle.start();

    let work = assemble_source_record_inner(
        ctx,
        source,
        output,
        preferred_build_source,
        env_ref,
        installed_source_hints,
    );
    match scope_ctx {
        Some(rc) => CURRENT_REPORTER_CONTEXT.scope(Some(rc), work).await,
        None => work.await,
    }
}

async fn assemble_source_record_inner(
    ctx: &mut ComputeCtx,
    source: &PinnedSourceCodeLocation,
    output: &CondaOutput,
    preferred_build_source: &Arc<BTreeMap<PackageName, PinnedSourceSpec>>,
    env_ref: &EnvironmentRef,
    installed_source_hints: &PtrArc<InstalledSourceHints>,
) -> Result<Arc<SourceRecord>, SourceRecordError> {
    let source_location = SourceLocationSpec::from(source.manifest_source().clone());
    let source_anchor = SourceAnchor::from(source_location.clone());
    let channel_config = ctx.compute(&ChannelConfigKey).await;
    let pkg_name = output.metadata.name.clone();

    // Look up this `(package, source_location)`'s install hint. The
    // nested build / host solves use it as their prior-resolution
    // seed; the full `installed_source_hints` still flows through
    // for deeper layers.
    let (installed_build_packages, installed_host_packages): (
        Arc<[UnresolvedPixiRecord]>,
        Arc<[UnresolvedPixiRecord]>,
    ) = match installed_source_hints.get(&pkg_name, &source_location) {
        Some(hint) => (
            Arc::clone(&hint.build_packages),
            Arc::clone(&hint.host_packages),
        ),
        None => (Arc::from([]), Arc::from([])),
    };

    let mut compatibility_map = HashMap::new();
    let build_dependencies = output
        .build_dependencies
        .as_ref()
        .map(|deps| Dependencies::new(deps, Some(source_anchor.clone()), &compatibility_map))
        .transpose()
        .map_err(SourceRecordError::from)?
        .unwrap_or_default();

    let mut build_records = nested_solve(
        ctx,
        &pkg_name,
        preferred_build_source,
        env_ref,
        DerivedEnvKind::Build,
        CycleEnvironment::Build,
        build_dependencies.clone(),
        Arc::clone(&installed_build_packages),
        installed_source_hints,
    )
    .await?;

    // Clone the gateway handle so we don't hold an immutable borrow
    // on `ctx` across the subsequent mutable-borrow calls (another
    // nested solve).
    let gateway = ctx.global_data().gateway().clone();
    let build_run_exports = build_dependencies
        .extract_run_exports(
            &mut build_records,
            &output.ignore_run_exports,
            &gateway,
            None,
        )
        .await
        .map_err(|err| {
            SourceRecordError::RunExportsExtraction(String::from("build"), Arc::new(err))
        })?;

    compatibility_map.extend(
        build_records
            .iter()
            .map(|record| (record.package_record().name.clone(), record)),
    );

    let host_dependencies = output
        .host_dependencies
        .as_ref()
        .map(|deps| Dependencies::new(deps, Some(source_anchor.clone()), &compatibility_map))
        .transpose()
        .map_err(SourceRecordError::from)?
        .unwrap_or_default()
        .extend_with_run_exports_from_build(&build_run_exports);

    let mut host_records = nested_solve(
        ctx,
        &pkg_name,
        preferred_build_source,
        env_ref,
        DerivedEnvKind::Host,
        CycleEnvironment::Host,
        host_dependencies.clone(),
        Arc::clone(&installed_host_packages),
        installed_source_hints,
    )
    .await?;

    let host_run_exports = host_dependencies
        .extract_run_exports(
            &mut host_records,
            &output.ignore_run_exports,
            &gateway,
            None,
        )
        .await
        .map_err(|err| {
            SourceRecordError::RunExportsExtraction(String::from("host"), Arc::new(err))
        })?;

    compatibility_map.extend(
        host_records
            .iter()
            .map(|record| (record.package_record().name.clone(), record)),
    );

    let run_dependencies = Dependencies::new(&output.run_dependencies, None, &compatibility_map)
        .map_err(SourceRecordError::from)?
        .extend_with_run_exports_from_build_and_host(
            host_run_exports,
            build_run_exports,
            output.metadata.subdir,
        );

    let mut sources: HashMap<PackageName, SourceLocationSpec> = HashMap::new();

    // Record a source-typed PixiSpec's location into `sources`, erroring
    // if the same (name, location) is registered twice.
    let mut track_source =
        |name: &PackageName, spec: &PixiSpec| -> Result<(), SourceRecordError> {
            if let Either::Left(source) = spec.clone().into_source_or_binary() {
                match sources.entry(name.clone()) {
                    std::collections::hash_map::Entry::Occupied(entry) => {
                        if entry.get() == &source.location {
                            return Err(SourceRecordError::DuplicateSourceDependency {
                                package: name.clone(),
                                source1: Box::new(entry.get().clone()),
                                source2: Box::new(source.location.clone()),
                            });
                        }
                    }
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(source.location.clone());
                    }
                }
            }
            Ok(())
        };

    // Stringify a PixiSpec dep map into a `Vec<String>`, threading source
    // locations through `track_source` for the per-source bookkeeping.
    let stringify_pixi_specs = |specs: DependencyMap<PackageName, PixiSpec>,
                                track_source: &mut dyn FnMut(
        &PackageName,
        &PixiSpec,
    )
        -> Result<(), SourceRecordError>|
     -> Result<Vec<String>, SourceRecordError> {
        specs
            .into_specs()
            .map(|(name, spec)| {
                track_source(&name, &spec)?;
                Ok(spec
                    .to_match_spec(&name, &channel_config)
                    .map_err(SourceRecordError::from)?
                    .to_string())
            })
            .collect()
    };

    let stringify_binary_specs =
        |specs: DependencyMap<PackageName, BinarySpec>| -> Result<Vec<String>, SourceRecordError> {
            specs
                .into_specs()
                .map(|(name, spec)| {
                    Ok(spec
                        .to_match_spec(&name, &channel_config)
                        .map_err(SourceRecordError::from)?
                        .to_string())
                })
                .collect()
        };

    let depends = run_dependencies
        .dependencies
        .clone()
        .into_specs()
        .map(|(name, withspec)| {
            track_source(&name, &withspec.value)?;
            Ok(withspec
                .value
                .to_match_spec(&name, &channel_config)
                .map_err(SourceRecordError::from)?
                .to_string())
        })
        .collect::<Result<Vec<_>, SourceRecordError>>()?;

    let constrains = run_dependencies
        .constraints
        .into_specs()
        .map(|(name, withspec)| {
            Ok(withspec
                .value
                .to_match_spec(&name, &channel_config)
                .map_err(SourceRecordError::from)?
                .to_string())
        })
        .collect::<Result<Vec<_>, SourceRecordError>>()?;

    let run_exports_pixi =
        PixiRunExports::try_from_protocol(&output.run_exports, &compatibility_map)
            .map_err(SourceRecordError::from)?;

    let run_exports = RunExportsJson {
        weak: stringify_pixi_specs(run_exports_pixi.weak, &mut track_source)?,
        strong: stringify_pixi_specs(run_exports_pixi.strong, &mut track_source)?,
        noarch: stringify_pixi_specs(run_exports_pixi.noarch, &mut track_source)?,
        weak_constrains: stringify_binary_specs(run_exports_pixi.weak_constrains)?,
        strong_constrains: stringify_binary_specs(run_exports_pixi.strong_constrains)?,
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
        run_exports: Some(run_exports),
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
    };

    let sources_by_str: BTreeMap<String, SourceLocationSpec> = sources
        .into_iter()
        .map(|(name, source)| (name.as_source().to_string(), source))
        .collect();

    let record = SourceRecord {
        data: FullSourceRecordData {
            package_record,
            sources: sources_by_str,
        },
        variants: output
            .metadata
            .variant
            .iter()
            .map(|(k, v)| (k.clone(), VariantValue::from(v.clone())))
            .collect(),
        manifest_source: source.manifest_source().clone(),
        build_source: source.build_source().cloned(),
        identifier_hash: None,
        // Carry the resolved build / host env package sets forward.
        // Downstream consumers (lock-file writer, installer) need
        // the exact packages this source was built against, not just
        // their aggregated run-exports.
        build_packages: build_records
            .into_iter()
            .map(pixi_record::UnresolvedPixiRecord::from)
            .collect(),
        host_packages: host_records
            .into_iter()
            .map(pixi_record::UnresolvedPixiRecord::from)
            .collect(),
    };

    Ok(Arc::new(record))
}

/// Solve a nested build/host environment for a source record.
///
/// Constructs a [`Derived`](EnvironmentRef::Derived) env_ref off of
/// `env_ref` and delegates to [`SolvePixiEnvironmentKey`]. `installed`
/// is the per-source build/host package set from the outer record
/// (lockfile state), used as the solver's pinning hint so previously
/// recorded versions stay stable across re-resolutions.
///
/// `preferred_build_source` is the full pin map (propagated verbatim
/// from the outer solve) so pins for source deps reachable from the
/// nested build/host env are honoured.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
async fn nested_solve(
    ctx: &mut ComputeCtx,
    pkg_name: &PackageName,
    preferred_build_source: &Arc<BTreeMap<PackageName, PinnedSourceSpec>>,
    env_ref: &EnvironmentRef,
    kind: DerivedEnvKind,
    cycle_env: CycleEnvironment,
    dependencies: Dependencies,
    installed: Arc<[UnresolvedPixiRecord]>,
    installed_source_hints: &PtrArc<InstalledSourceHints>,
) -> Result<Vec<PixiRecord>, SourceRecordError> {
    if dependencies.dependencies.is_empty() {
        return Ok(vec![]);
    }

    let nested_spec = SolvePixiEnvironmentSpec {
        dependencies: dependencies
            .dependencies
            .into_specs()
            .map(|(name, withspec)| (name, withspec.value))
            .collect(),
        constraints: dependencies
            .constraints
            .into_specs()
            .map(|(name, withspec)| (name, withspec.value))
            .collect(),
        dev_sources: Default::default(),
        // Per-source build / host env `installed` hint: the packages
        // the outer record's [`SourceRecord::build_packages`] /
        // [`SourceRecord::host_packages`] captured on a previous
        // resolution. Passing these keeps solver output stable when
        // the graph is unchanged; empty on first-ever resolution.
        installed,
        installed_source_hints: installed_source_hints.clone(),
        strategy: SolveStrategy::default(),
        preferred_build_source: Arc::clone(preferred_build_source),
        env_ref: env_ref.derived(pkg_name.clone(), kind),
    };

    // Wrap the nested SolvePixiEnvironmentKey call in a cycle
    // guard. If resolving this build/host env forms a cycle back to
    // a source we're already resolving, the engine's detector fires
    // this guard and we get `Err(CycleError)` rather than hanging
    // or surfacing a raw `ComputeError::Cycle` at the top-level
    // `engine.compute` boundary. The cycle path carries type-erased
    // keys; we pull `(PackageName, CycleEnvironment)` frames out of
    // `ResolveSourcePackageKey`s along the path via `Key::provide`
    // (see that key's impl), and fall back to a one-frame Cycle
    // rooted at this call site if the path turns up no such frames.
    let pkg_name_for_cycle = pkg_name.clone();
    let guarded = ctx
        .with_cycle_guard(async |cctx| {
            cctx.compute(&SolvePixiEnvironmentKey::new(nested_spec))
                .await
        })
        .await;

    let compute_result = match guarded {
        Ok(value) => value,
        Err(cycle) => {
            return Err(SourceRecordError::Cycle(render_cycle(
                &cycle,
                Some((pkg_name_for_cycle, cycle_env)),
            )));
        }
    };

    let records = compute_result.map_err(|err| match err {
        // A cycle caught inside SPEK's body surfaces here as
        // `SolvePixiEnvironmentError::Cycle`; preserve its identity
        // so callers (and tests) see `SourceRecordError::Cycle` and
        // not a wrapping "solve the build/host environment" error.
        crate::SolvePixiEnvironmentError::Cycle(cycle) => SourceRecordError::Cycle(cycle),
        other => match cycle_env {
            CycleEnvironment::Build => SourceRecordError::SolveBuildEnvironment(Box::new(other)),
            CycleEnvironment::Host | CycleEnvironment::Run => {
                SourceRecordError::SolveHostEnvironment(Box::new(other))
            }
        },
    })?;

    Ok((*records).clone())
}

/// Build a [`crate::Cycle`] (the existing box-drawing error type) from a
/// compute-engine [`pixi_compute_engine::CycleError`]. Walks the ring of
/// `AnyKey` frames and asks each one for a [`SourceCycleFrame`] (provided
/// by `ResolveSourcePackageKey::provide`). Frames that don't supply one
/// (e.g. `SolvePixiEnvironmentKey` frames between RSP frames) are
/// skipped.
///
/// If `fallback` is `Some` and no RSP frames were found on the ring,
/// it is appended so even a path with no RSP frames produces a
/// non-empty rendering.
pub(crate) fn render_cycle(
    cycle: &pixi_compute_engine::CycleError,
    fallback: Option<(PackageName, CycleEnvironment)>,
) -> crate::Cycle {
    let mut stack = Vec::new();
    for key in &cycle.path {
        // Non-RSP frames (SPEK, SMK, etc.) don't provide
        // SourceCycleFrame; skip.
        if let Some(frame) = key.request_value::<SourceCycleFrame>() {
            stack.push((frame.package, frame.env));
        }
    }
    if stack.is_empty()
        && let Some(fb) = fallback
    {
        stack.push(fb);
    }
    crate::Cycle { stack }
}

/// Per-frame cycle metadata exposed by
/// `ResolveSourcePackageKey::provide`. The cycle-path walker in
/// [`render_cycle`] pulls one of these per RSP frame on the ring.
#[derive(Clone, Debug)]
pub(crate) struct SourceCycleFrame {
    pub package: PackageName,
    pub env: CycleEnvironment,
}

/// [`LifecycleKind`] wiring [`SourceRecordReporter`] events for a
/// per-variant [`assemble_source_record`] call.
struct SourceRecordReporterLifecycle;

impl LifecycleKind for SourceRecordReporterLifecycle {
    type Reporter<'r> = dyn SourceRecordReporter + 'r;
    type Id = SourceRecordId;
    type Env = SourceRecordSpec;

    fn queue<'r>(
        reporter: Option<&'r dyn Reporter>,
        parent: Option<ReporterContext>,
        env: &Self::Env,
    ) -> Option<Active<'r, Self::Reporter<'r>, Self::Id>> {
        reporter
            .and_then(|r| r.as_source_record_reporter())
            .map(|r| Active {
                reporter: r,
                id: r.on_queued(parent, env),
            })
    }

    fn on_started<'r>(active: &Active<'r, Self::Reporter<'r>, Self::Id>) {
        active.reporter.on_started(active.id);
    }

    fn on_finished<'r>(active: Active<'r, Self::Reporter<'r>, Self::Id>) {
        active.reporter.on_finished(active.id);
    }
}
