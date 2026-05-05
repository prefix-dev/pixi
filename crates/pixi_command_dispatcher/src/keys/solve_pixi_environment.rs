//! Compute-engine Key that solves a pixi environment. Keyed on `env_ref`
//! plus requirements (not content-addressed); cross-env dedup happens at the
//! content-addressed inner layers ([`SolveCondaKey`]
//! and [`BuildBackendMetadataKey`](crate::BuildBackendMetadataKey)) where the
//! expensive work runs.
//!
//! Recurses via [`ResolveSourcePackageKey`]
//! into build/host envs; cycles are caught by the cycle guard in the
//! private `resolve_source_record` module.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    hash::{Hash, Hasher},
    mem,
    sync::Arc,
    time::Instant,
};

use derive_more::Display;
use futures::stream::{FuturesUnordered, StreamExt};
use ordermap::OrderMap;
use pixi_compute_engine::{ComputeCtx, Demand, Key, ParallelBuilder};
use pixi_record::{DevSourceRecord, PinnedSourceSpec, PixiRecord, UnresolvedPixiRecord};
use pixi_spec::{
    BinarySpec, DevSourceSpec, PixiSpec, ResolvedExcludeNewer, SourceAnchor, SourceLocationSpec,
    SourceSpec,
};
use pixi_spec_containers::DependencyMap;
use pixi_variant::VariantValue;
use rattler_conda_types::{MatchSpec, PackageName, PackageNameMatcher, ParseStrictness};
use rattler_solve::SolveStrategy;
use tracing::instrument;

use crate::{
    BuildBackendMetadataSpec, DerivedEnvKind, DevSourceMetadataKey, DevSourceMetadataSpec,
    EnvironmentRef, HasWorkspaceEnvRegistry, InstalledSourceHints, PixiSolveEnvironmentSpec,
    PixiSolveReporter, PtrArc, Reporter, ReporterContext, SolvePixiEnvironmentError,
    SourceMetadata,
    build::PinnedSourceCodeLocation,
    compute_data::HasReporter,
    injected_config::ChannelConfigKey,
    keys::{
        resolve_source_package::{ResolveSourcePackageKey, ResolveSourcePackageSpec},
        resolve_source_record::{SourceCycleFrame, render_cycle},
        solve_conda::{SolveCondaKey, SolveCondaKeyError, SolveCondaSpec},
    },
    reporter::{PixiSolveId, has_direct_conda_dependency},
    reporter_context::{CURRENT_REPORTER_CONTEXT, current_reporter_context},
    reporter_lifecycle::{Active, LifecycleKind, ReporterLifecycle},
    source_checkout::SourceCheckoutExt,
    source_metadata::CycleEnvironment,
};

/// Input to [`SolvePixiEnvironmentKey`]. The env display label comes
/// from `env_ref`'s `Display` impl.
#[derive(Debug, Clone)]
pub struct SolvePixiEnvironmentSpec {
    pub dependencies: DependencyMap<PackageName, PixiSpec>,
    pub constraints: DependencyMap<PackageName, BinarySpec>,
    pub dev_sources: OrderMap<PackageName, DevSourceSpec>,
    /// Prior-resolution state used as solver-stability hints. Partial
    /// source records are preserved: their `build_packages` /
    /// `host_packages` / pinned source are still useful inputs even
    /// when the `PackageRecord` needs a fresh backend query. Partials
    /// are filtered out only at the `SolveCondaKey` boundary, where
    /// the solver needs a full `PackageRecord` to pin a version.
    pub installed: Arc<[UnresolvedPixiRecord]>,
    /// Deduplicated, depth-unified view of `installed`'s source-record
    /// hints. Built once at the top-level caller via
    /// [`InstalledSourceHints::from_records`] and propagated unchanged
    /// through nested solves so every layer of the recursion agrees on
    /// one canonical hint per `(PackageName, SourceLocationSpec)`.
    ///
    /// [`PtrArc`] (pointer-identity `Hash`/`Eq`) because the map flows
    /// verbatim through the recursive solve; callers that share the
    /// same `Arc` across nested spec construction dedup via
    /// content-free pointer identity.
    pub installed_source_hints: PtrArc<InstalledSourceHints>,
    pub strategy: SolveStrategy,
    /// Invariant: every `PinnedSourceSpec` here must also appear as
    /// the `build_source` of some [`PixiRecord::Source`] in
    /// [`Self::installed`].
    ///
    /// `Arc`-wrapped because the map is propagated verbatim through
    /// the recursive solve (SPEK → RSP → nested SPEK → …) and
    /// deep-cloning a `BTreeMap` at each hop adds up. `Arc<BTreeMap<_,_>>`
    /// is transparent for `Hash`/`Eq` via `Deref`, so dedup keys
    /// stay content-addressed.
    pub preferred_build_source: Arc<BTreeMap<PackageName, PinnedSourceSpec>>,
    pub env_ref: EnvironmentRef,
}

impl Hash for SolvePixiEnvironmentSpec {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // TODO: collapse to `derive(Hash)` once `SolveStrategy`
        // implements Hash (rattler PR
        // https://github.com/conda/rattler/pull/2377).
        let Self {
            dependencies,
            constraints,
            dev_sources,
            installed,
            installed_source_hints,
            strategy,
            preferred_build_source,
            env_ref,
        } = self;
        dependencies.hash(state);
        constraints.hash(state);
        dev_sources.hash(state);
        installed.hash(state);
        installed_source_hints.hash(state);
        mem::discriminant(strategy).hash(state);
        preferred_build_source.hash(state);
        env_ref.hash(state);
    }
}

impl PartialEq for SolvePixiEnvironmentSpec {
    // TODO: collapse to `derive(PartialEq)` once rattler PR above lands.
    fn eq(&self, other: &Self) -> bool {
        self.dependencies == other.dependencies
            && self.constraints == other.constraints
            && self.dev_sources == other.dev_sources
            && self.installed == other.installed
            && self.installed_source_hints == other.installed_source_hints
            && mem::discriminant(&self.strategy) == mem::discriminant(&other.strategy)
            && self.preferred_build_source == other.preferred_build_source
            && self.env_ref == other.env_ref
    }
}

impl Eq for SolvePixiEnvironmentSpec {}

#[derive(Clone, Debug, Display)]
#[display("{}", _0.env_ref)]
pub struct SolvePixiEnvironmentKey(pub Arc<SolvePixiEnvironmentSpec>);

impl SolvePixiEnvironmentKey {
    pub fn new(spec: SolvePixiEnvironmentSpec) -> Self {
        Self(Arc::new(spec))
    }
}

impl Hash for SolvePixiEnvironmentKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl PartialEq for SolvePixiEnvironmentKey {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || *self.0 == *other.0
    }
}

impl Eq for SolvePixiEnvironmentKey {}

impl Key for SolvePixiEnvironmentKey {
    type Value = Result<Arc<Vec<PixiRecord>>, SolvePixiEnvironmentError>;

    #[instrument(
        skip_all,
        name = "solve-pixi-environment",
        fields(env = %self.0.env_ref, platform = %self.0.env_ref.display_platform()),
    )]
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let spec = self.0.clone();

        let env_spec = spec
            .env_ref
            .resolve(ctx.global_data().workspace_env_registry());
        let channel_config = ctx.compute(&ChannelConfigKey).await;

        // Reporter lifecycle: fire `on_queued` now, `on_started`
        // when we call `.start()` below, `on_finished` when the
        // returned handle is dropped at end of scope. Build a small
        // reporter view instead of cloning the full solve spec.
        //
        // Clone the reporter `Arc` out of `global_data` so the
        // lifecycle reference doesn't keep an immutable borrow on
        // `ctx` alive across later mutable-borrow calls in this
        // function.
        let reporter_spec = reporter_view_spec(spec.env_ref.to_string(), &spec);
        let reporter_arc: Option<Arc<dyn Reporter>> = ctx.global_data().reporter().cloned();
        let parent_reporter_ctx = current_reporter_context();
        let lifecycle = ReporterLifecycle::<PixiSolveReporterLifecycle>::queued(
            reporter_arc.as_deref(),
            parent_reporter_ctx,
            &reporter_spec,
        );
        // Scope nested Keys under our reporter context so downstream
        // computes (source metadata, build backend metadata, nested
        // solves, etc.) attribute to this solve rather than the caller,
        // producing a nested trace. Falls back to the parent's context
        // when no reporter is attached.
        let scope_ctx = lifecycle
            .id()
            .map(ReporterContext::SolvePixi)
            .or(parent_reporter_ctx);
        let _lifecycle = lifecycle.start();

        // Scope nested Keys under our reporter context so downstream
        // computes (dev-source metadata, source metadata, build
        // backend metadata, nested SPEK solves, top-level conda solve)
        // attribute to this solve and render a nested event tree.
        let work = compute_inner(ctx, spec, env_spec, channel_config);
        match scope_ctx {
            Some(rc) => CURRENT_REPORTER_CONTEXT.scope(Some(rc), work).await,
            None => work.await,
        }
    }

    /// Expose a `SourceCycleFrame` on this SPEK key. When a cycle on
    /// the compute-engine ring passes through a SPEK of a derived
    /// env, the `(package, kind)` pair carried in its env_ref is
    /// exactly the `(source_pkg, outgoing_dep_kind)` edge the frame
    /// should report: "the package whose build/host env we are
    /// solving has a `kind`-dep on the next frame".
    ///
    /// Top-level SPEKs (`Workspace` / `Ephemeral` env_refs) don't
    /// correspond to an outgoing dep edge on any cycle, so they
    /// provide nothing and are skipped during cycle rendering.
    fn provide<'a>(&'a self, demand: &mut Demand<'a, '_>) {
        if let EnvironmentRef::Derived { package, kind, .. } = &self.0.env_ref {
            let env = match kind {
                DerivedEnvKind::Build => CycleEnvironment::Build,
                DerivedEnvKind::Host => CycleEnvironment::Host,
            };
            demand.provide_value(SourceCycleFrame {
                package: package.clone(),
                env,
            });
        }
    }
}

/// Core body of [`SolvePixiEnvironmentKey::compute`], separated so
/// the caller can run it inside a [`CURRENT_REPORTER_CONTEXT`] scope
/// keyed on this solve's reporter id.
async fn compute_inner(
    ctx: &mut ComputeCtx,
    spec: Arc<SolvePixiEnvironmentSpec>,
    env_spec: Arc<crate::EnvironmentSpec>,
    channel_config: Arc<rattler_conda_types::ChannelConfig>,
) -> Result<Arc<Vec<PixiRecord>>, SolvePixiEnvironmentError> {
    // Derive a common exclude_newer cutoff so every transitive
    // source dep uses the same value.
    let exclude_newer = env_spec
        .exclude_newer
        .clone()
        .unwrap_or_else(|| ResolvedExcludeNewer::from_datetime(chrono::Utc::now()));

    let dev_source_records = process_dev_sources(ctx, &spec).await?;

    // Split explicit requirements into source and binary halves;
    // same for dev-source-contributed deps.
    let (dev_source_source_specs, dev_source_binary_specs) =
        DevSourceRecord::split_into_source_and_binary_requirements(
            DevSourceRecord::dev_source_dependencies(&dev_source_records),
        );
    let (source_specs, binary_specs) = DevSourceRecord::split_into_source_and_binary_requirements(
        spec.dependencies.clone().into_specs(),
    );

    crate::solve_pixi::check_missing_channels(
        binary_specs.clone(),
        &env_spec.channels,
        &channel_config,
    )
    .map_err(|b| *b)?;

    // BFS over (package, source_location) pairs. Driving the walk off
    // assembled records (not raw `CondaOutput.run_dependencies`) is
    // essential: source deps introduced purely via build-env or host-env
    // run-exports only appear on the assembled record.
    let seeds: Vec<(PackageName, SourceSpec)> = source_specs
        .iter_specs()
        .map(|(n, s)| (n.clone(), s.clone()))
        .chain(dev_source_source_specs.into_specs())
        .collect();

    // Source-record hints for this solve. Keyed on
    // `(PackageName, SourceLocationSpec)`; the same `Arc` flows
    // through every nested solve so a given source package gets the
    // same hint regardless of which branch of the recursion reached
    // it.
    let resolved = walk_and_resolve(
        ctx,
        seeds,
        &spec.env_ref,
        &spec.preferred_build_source,
        &spec.installed_source_hints,
    )
    .await?;

    // Group resolved records by source location for SolveCondaKey.
    // Ordering matters: SolveCondaKey hashes `source_repodata`
    // positionally and the `resolved` Vec inherits FuturesUnordered
    // completion order, so sort into a stable order (records by
    // (name, variants); groups by source Display string).
    let mut groups: HashMap<PinnedSourceCodeLocation, Vec<Arc<pixi_record::SourceRecord>>> =
        HashMap::new();
    for record in resolved {
        let loc = PinnedSourceCodeLocation::new(
            record.manifest_source().clone(),
            record.build_source().cloned(),
        );
        groups.entry(loc).or_default().push(record);
    }
    let mut grouped: Vec<(
        PinnedSourceCodeLocation,
        Vec<Arc<pixi_record::SourceRecord>>,
    )> = groups.into_iter().collect();
    for (_loc, records) in grouped.iter_mut() {
        records.sort_by(|a, b| {
            a.package_record()
                .name
                .as_normalized()
                .cmp(b.package_record().name.as_normalized())
                .then_with(|| cmp_variants(&a.variants, &b.variants))
        });
    }
    grouped.sort_by_key(|(source, _)| source.to_string());
    let source_repodata: Vec<Arc<SourceMetadata>> = grouped
        .into_iter()
        .map(|(source, records)| Arc::new(SourceMetadata { source, records }))
        .collect();

    // Transitive binary deps are derived inside SolveCondaKey from the
    // assembled source + dev source records, so the orchestrator drops
    // these split halves.
    let _ = dev_source_binary_specs;

    let started = Instant::now();
    let result = ctx
        .compute(&SolveCondaKey::new(SolveCondaSpec {
            source_specs,
            binary_specs,
            constraints: spec.constraints.clone(),
            dev_source_records,
            source_repodata,
            // SolveCondaKey feeds the rattler solver, which needs a
            // full `PackageRecord` to pin a version. Drop partials
            // here; any previously-partial source that is still a
            // dep of this env has been freshly resolved via the
            // walk and is already present in `source_repodata`.
            installed: spec
                .installed
                .iter()
                .filter_map(|r| r.clone().try_into_resolved().ok())
                .collect(),
            platform: env_spec.build_environment.host_platform,
            channels: env_spec.channels.clone(),
            virtual_packages: env_spec.build_environment.host_virtual_packages.clone(),
            strategy: spec.strategy,
            channel_priority: env_spec.channel_priority,
            exclude_newer: Some(exclude_newer),
        }))
        .await
        .map_err(|e| match e {
            SolveCondaKeyError::Solve(a) => SolvePixiEnvironmentError::SolveError(a),
            SolveCondaKeyError::SpecConversion(a) => {
                SolvePixiEnvironmentError::SpecConversionError(a)
            }
            SolveCondaKeyError::Gateway(a) => SolvePixiEnvironmentError::QueryError(a),
        })?;
    tracing::debug!("top-level solve completed in {:?}", started.elapsed());

    Ok(Arc::new((*result).clone()))
}

/// Discover + resolve every source package reachable from the
/// seed specs.
///
/// Dynamic BFS over `(package, source_location)` pairs. A
/// [`ParallelBuilder`] mints one branch future per push; each branch
/// runs on a sub-[`ComputeCtx`] with a branch-local cycle-guard stack
/// chained to the parent's, and wraps the
/// `ctx.compute(&ResolveSourcePackageKey)` call in
/// [`ctx.with_cycle_guard`](ComputeCtx::with_cycle_guard) so a cycle
/// closing back to that RSP fires the branch's user guard instead of
/// the task's synthetic fallback (which would escape as
/// `ComputeError::Cycle` even when the cycle is logically
/// recoverable). Pushing branches onto a [`FuturesUnordered`] lets the
/// walk feed new pairs as earlier branches complete.
///
/// When the branch-local guard fires, we know the `(parent, child)`
/// walk frame that pushed this RSP, so we can extend the rendered
/// cycle with the `(parent, Run)` frame the compute-engine ring
/// cannot carry (parent's RSP finishes successfully and its edge is
/// dropped before the cycle closes, so it is not present in the
/// engine's cycle path).
///
/// Returns every resolved `SourceRecord` transitively reachable from
/// the seeds. Binary match specs are NOT collected here; they're
/// derived downstream inside [`SolveCondaKey`] from the same
/// assembled records (see `derive_fetch_specs_from_source_repodata`).
async fn walk_and_resolve(
    ctx: &mut ComputeCtx,
    seeds: Vec<(PackageName, SourceSpec)>,
    env_ref: &EnvironmentRef,
    preferred_build_source: &Arc<BTreeMap<PackageName, PinnedSourceSpec>>,
    installed_source_hints: &PtrArc<InstalledSourceHints>,
) -> Result<Vec<Arc<pixi_record::SourceRecord>>, SolvePixiEnvironmentError> {
    let mut all_records: Vec<Arc<pixi_record::SourceRecord>> = Vec::new();
    let mut seen_sources: HashSet<(PackageName, SourceLocationSpec)> = HashSet::new();

    // Fallback cycle frame used when a pushed RSP's guard fires but
    // the engine's cycle ring carried no RSP frames (e.g. the cycle
    // was a self-loop on a Key that doesn't provide SourceCycleFrame).
    // Mirrors RSP::provide's kind→CycleEnvironment mapping for the
    // enclosing env_ref.
    let mut p = ctx.parallel();
    let mut pending: FuturesUnordered<_> = FuturesUnordered::new();

    // Push one branch per (package, source_location, Option<parent>).
    // `parent` is `Some(pkg)` when this RSP was discovered through a
    // transitive run-dep on `pkg`'s assembled record; `None` for seeds.
    // The parent is what we prepend to a cycle's stack as
    // `(parent, Run)` when this RSP's guard fires, reconstructing the
    // `parent ↔ child` dep chain the engine's active-edge graph
    // cannot preserve across completed peers.
    let push = |p: &mut ParallelBuilder<'_>,
                pending: &mut FuturesUnordered<_>,
                seen: &mut HashSet<(PackageName, SourceLocationSpec)>,
                name: PackageName,
                location: SourceLocationSpec,
                parent: Option<PackageName>| {
        if !seen.insert((name.clone(), location.clone())) {
            return;
        }
        let key = ResolveSourcePackageKey::new(ResolveSourcePackageSpec {
            package: name.clone(),
            source_location: location,
            preferred_build_source: Arc::clone(preferred_build_source),
            env_ref: env_ref.clone(),
            installed_source_hints: installed_source_hints.clone(),
        });
        pending.push(p.compute(async move |sub_ctx: &mut ComputeCtx| {
            // Per-push cycle guard. `sub_ctx` has a branch-local
            // guard stack; installing a guard here means edges
            // established by the `ctx.compute(&key)` below capture
            // this guard as notify (not the task's synthetic
            // fallback), so a cycle that closes on this RSP fires
            // here and we return `Err(CycleError)` rather than
            // escaping as `ComputeError::Cycle`.
            let guarded = sub_ctx
                .with_cycle_guard(async |cctx| cctx.compute(&key).await)
                .await;
            match guarded {
                Ok(Ok(records)) => Ok((name, parent, records)),
                Ok(Err(e)) => Err(SolvePixiEnvironmentError::from(
                    crate::SourceMetadataError::SourceRecord(e),
                )),
                Err(cycle) => {
                    let fallback_env = match env_ref {
                        EnvironmentRef::Derived { kind, .. } => match kind {
                            DerivedEnvKind::Build => CycleEnvironment::Build,
                            DerivedEnvKind::Host => CycleEnvironment::Host,
                        },
                        EnvironmentRef::Workspace(_) | EnvironmentRef::Ephemeral(_) => {
                            CycleEnvironment::Run
                        }
                    };

                    let mut rendered =
                        render_cycle(&cycle, Some((name.clone(), fallback_env.clone())));

                    // Prepend `(parent, Run)` so the rendered
                    // cycle reads `parent ↔ child` rather than
                    // `child ↔ child` for a 2-cycle that closes
                    // through a peer we just resolved.
                    if let Some(parent_pkg) = parent {
                        rendered
                            .stack
                            .insert(0, (parent_pkg, CycleEnvironment::Run));
                    }

                    Err(SolvePixiEnvironmentError::Cycle(rendered))
                }
            }
        }));
    };

    for (name, spec) in seeds {
        push(
            &mut p,
            &mut pending,
            &mut seen_sources,
            name,
            spec.location,
            None,
        );
    }

    while let Some(result) = pending.next().await {
        let (parent_pkg, _parent_of_parent, records) = result?;

        // Walk the assembled records' `.depends` (post run-exports)
        // for source refs and queue newly-seen pairs, tagging each
        // with the parent package that pulled it in.
        for record in records.iter() {
            let anchor =
                SourceAnchor::from(SourceLocationSpec::from(record.manifest_source().clone()));
            for depend_str in &record.package_record().depends {
                let Ok(match_spec) = MatchSpec::from_str(depend_str, ParseStrictness::Lenient)
                else {
                    continue;
                };
                let (name_matcher, _) = match_spec.into_nameless();
                let PackageNameMatcher::Exact(child_name) = name_matcher else {
                    continue;
                };
                if let Some(source_location) = record.sources().get(child_name.as_normalized()) {
                    let resolved_location = anchor.resolve(source_location.clone());
                    push(
                        &mut p,
                        &mut pending,
                        &mut seen_sources,
                        child_name,
                        resolved_location,
                        Some(parent_pkg.clone()),
                    );
                }
            }
        }

        all_records.extend(records.iter().cloned());
    }

    Ok(all_records)
}

/// Pin + metadata for every dev source, concurrently.
async fn process_dev_sources(
    ctx: &mut ComputeCtx,
    spec: &SolvePixiEnvironmentSpec,
) -> Result<Vec<DevSourceRecord>, SolvePixiEnvironmentError> {
    if spec.dev_sources.is_empty() {
        return Ok(Vec::new());
    }

    let mut checkout_futs: FuturesUnordered<_> = spec
        .dev_sources
        .iter()
        .map(|(name, dev_spec)| {
            let name = name.clone();
            let env_ref = spec.env_ref.clone();
            let preferred_build_source = spec.preferred_build_source.get(&name).cloned();
            let fut = ctx.pin_and_checkout(dev_spec.source.location.clone());
            async move {
                let checkout = fut
                    .await
                    .map_err(SolvePixiEnvironmentError::SourceCheckoutError)?;
                Ok::<_, SolvePixiEnvironmentError>((
                    name,
                    checkout.pinned,
                    preferred_build_source,
                    env_ref,
                ))
            }
        })
        .collect();

    let mut metadata_futs: FuturesUnordered<_> = FuturesUnordered::new();
    while let Some(res) = checkout_futs.next().await {
        let (name, pinned, preferred_build_source, env_ref) = res?;
        let key = DevSourceMetadataKey::new(DevSourceMetadataSpec {
            package_name: name,
            backend_metadata: BuildBackendMetadataSpec {
                manifest_source: pinned,
                preferred_build_source,
                env_ref,
            },
        });
        metadata_futs.push(ctx.compute(&key));
    }

    let mut records = Vec::new();
    while let Some(res) = metadata_futs.next().await {
        let metadata = res.map_err(SolvePixiEnvironmentError::DevSourceMetadataError)?;
        records.extend(metadata.records.iter().cloned());
    }
    Ok(records)
}

/// Stable ordering for variant maps. `VariantValue` is not `Ord`, so
/// we project each value through its `Display` impl before
/// comparing. The map itself is already sorted (BTreeMap), so this
/// gives a total order on the pairwise projection.
fn cmp_variants(
    a: &BTreeMap<String, VariantValue>,
    b: &BTreeMap<String, VariantValue>,
) -> std::cmp::Ordering {
    a.iter()
        .map(|(k, v)| (k.as_str(), v.to_string()))
        .cmp(b.iter().map(|(k, v)| (k.as_str(), v.to_string())))
}

fn reporter_view_spec(name: String, spec: &SolvePixiEnvironmentSpec) -> PixiSolveEnvironmentSpec {
    PixiSolveEnvironmentSpec {
        name,
        platform: spec.env_ref.display_platform(),
        has_direct_conda_dependency: has_direct_conda_dependency(&spec.dependencies),
    }
}

/// [`LifecycleKind`] wiring [`PixiSolveReporter`] events for a
/// [`SolvePixiEnvironmentKey`] compute.
struct PixiSolveReporterLifecycle;

impl LifecycleKind for PixiSolveReporterLifecycle {
    type Reporter<'r> = dyn PixiSolveReporter + 'r;
    type Id = PixiSolveId;
    type Env = PixiSolveEnvironmentSpec;

    fn queue<'r>(
        reporter: Option<&'r dyn Reporter>,
        parent: Option<ReporterContext>,
        env: &Self::Env,
    ) -> Option<Active<'r, Self::Reporter<'r>, Self::Id>> {
        reporter
            .and_then(|r| r.as_pixi_solve_reporter())
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
