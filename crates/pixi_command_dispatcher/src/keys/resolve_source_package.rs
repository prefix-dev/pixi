//! Compute-engine Key that resolves every variant output of a single
//! source package into fully assembled
//! [`SourceRecord`]s. Calls
//! [`SourceMetadataKey`] once and fans
//! out per variant. Keyed on
//! `(package, source_location, preferred_build_source, env_ref)`.

use std::{collections::BTreeMap, hash::Hash, sync::Arc};

use derive_more::Display;
use pixi_build_types::procedures::conda_outputs::CondaOutput;
use pixi_compute_engine::{ComputeCtx, Key};
use pixi_compute_reporters::OperationId;
use pixi_record::{PinnedSourceSpec, SourceRecord};
use pixi_spec::SourceLocationSpec;
use rattler_conda_types::PackageName;
use tracing::instrument;

use crate::{
    BuildBackendMetadataSpec, EnvironmentRef, InstalledSourceHints, PtrArc, SourceMetadataError,
    SourceMetadataReporterSpec, SourceRecordError,
    build::PinnedSourceCodeLocation,
    compute_data::HasSourceMetadataReporter,
    keys::{
        resolve_source_record::assemble_source_record,
        source_metadata::{SourceMetadataKey, SourceMetadataSpec},
    },
    reporter::SourceMetadataReporter,
    source_checkout::SourceCheckoutExt,
};
use pixi_compute_reporters::{Active, LifecycleKind, ReporterLifecycle};

/// Input to [`ResolveSourcePackageKey`]. `preferred_build_source` is the
/// full pin map inherited from the enclosing
/// [`SolvePixiEnvironmentKey`](super::solve_pixi_environment::SolvePixiEnvironmentKey)
/// (not just this package's entry) so nested solves see pins for every
/// package they transitively reference.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct ResolveSourcePackageSpec {
    pub package: PackageName,
    /// Unpinned; SMK pins it.
    pub source_location: SourceLocationSpec,
    pub preferred_build_source: Arc<BTreeMap<PackageName, PinnedSourceSpec>>,
    pub env_ref: EnvironmentRef,
    /// Source-record hints inherited from the enclosing SPEK; forwarded
    /// verbatim into nested build/host solves so every layer of the
    /// recursion agrees on one canonical hint per
    /// `(PackageName, SourceLocationSpec)`. The hint matching
    /// `(package, source_location)` seeds this RSP's nested build / host
    /// solves with the previous resolution's `build_packages` /
    /// `host_packages`; all other hints flow through for deeper layers.
    pub installed_source_hints: PtrArc<InstalledSourceHints>,
}

/// Compute-engine Key returning every variant's assembled
/// `SourceRecord` for one source package.
///
/// Reports progress via `Arc<dyn SourceMetadataReporter>` set on the engine `DataStore`, if any.
#[derive(Clone, Debug, Display, Eq, Hash, PartialEq)]
#[display("{}@{} in {}", _0.package.as_source(), _0.source_location, _0.env_ref)]
pub struct ResolveSourcePackageKey(pub Arc<ResolveSourcePackageSpec>);

impl ResolveSourcePackageKey {
    pub fn new(spec: ResolveSourcePackageSpec) -> Self {
        Self(Arc::new(spec))
    }
}

impl Key for ResolveSourcePackageKey {
    type Value = Result<Arc<Vec<Arc<SourceRecord>>>, SourceRecordError>;

    #[instrument(
        skip_all,
        name = "resolve-source-package",
        fields(
            name = %self.0.package.as_source(),
            source = %self.0.source_location,
            platform = %self.0.env_ref.display_platform(),
        )
    )]
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let spec = self.0.clone();

        // Reuse the previously-locked manifest pin when it is still
        // compatible with the manifest's source spec. Without this,
        // a `git+url?rev=main` spec would re-resolve `main` to its
        // current HEAD on every solve, drifting commits even when
        // nothing material changed.
        let manifest_pin_override = spec
            .installed_source_hints
            .find_for_location(&spec.package, &spec.source_location)
            .map(|hint| hint.manifest_source.clone());

        // Pin the source up front so the reporter's `on_queued` can
        // label the event with the resolved manifest source. SMK will
        // re-run the checkout internally; the checkout layer caches,
        // so the second call is effectively free.
        let checkout = match manifest_pin_override
            .as_ref()
            .filter(|pin| pin.matches_source_spec(&spec.source_location))
        {
            Some(pin) => ctx
                .checkout_pinned_source(pin.clone())
                .await
                .map_err(SourceRecordError::SourceCheckout)?,
            None => ctx
                .pin_and_checkout(spec.source_location.clone())
                .await
                .map_err(SourceRecordError::SourceCheckout)?,
        };
        let own_pin = spec.preferred_build_source.get(&spec.package).cloned();

        // Reporter lifecycle for this package's source-metadata work.
        // The SMK call below fires `Build backend metadata` and each
        // `assemble_source_record` fires `Source record`; scoping
        // both under this lifecycle nests them as children of
        // `Source metadata` in the event tree.
        let reporter_spec = SourceMetadataReporterSpec {
            package: spec.package.clone(),
            backend_metadata: BuildBackendMetadataSpec {
                manifest_source: checkout.pinned.clone(),
                preferred_build_source: own_pin.clone(),
                env_ref: spec.env_ref.clone(),
            },
            exclude_newer: None,
        };
        let reporter_arc: Option<Arc<dyn SourceMetadataReporter>> =
            ctx.global_data().source_metadata_reporter().cloned();
        let lifecycle = ReporterLifecycle::<SourceMetadataReporterLifecycle>::queued(
            reporter_arc.as_deref(),
            &reporter_spec,
        );
        let active_id = lifecycle.id();
        let _lifecycle = lifecycle.start();

        let work = resolve_source_package_inner(ctx, spec, own_pin, manifest_pin_override);
        match active_id {
            Some(id) => id.scope_active(work).await,
            None => work.await,
        }
    }
}

/// Core of [`ResolveSourcePackageKey::compute`], separated so the
/// wrapper can run it under `OperationId::scope_active`. The
/// caller has already fired `on_queued` / `on_started`; this just
/// runs the metadata fetch + per-variant assembly.
async fn resolve_source_package_inner(
    ctx: &mut ComputeCtx,
    spec: Arc<ResolveSourcePackageSpec>,
    own_pin: Option<PinnedSourceSpec>,
    manifest_pin_override: Option<PinnedSourceSpec>,
) -> Result<Arc<Vec<Arc<SourceRecord>>>, SourceRecordError> {
    // SMK only needs this package's pin as a checkout override;
    // the full pin map flows through assembly for recursion.
    let outputs = ctx
        .compute(&SourceMetadataKey::new(SourceMetadataSpec {
            package: spec.package.clone(),
            source_location: spec.source_location.clone(),
            preferred_build_source: own_pin,
            manifest_pin_override,
            env_ref: spec.env_ref.clone(),
        }))
        .await
        .map_err(map_source_metadata_error)?;

    // Fan out per variant; `try_compute_join` short-circuits on the
    // first error. Arc-cloned maps flow into each branch so nested
    // build/host solves see the full pins and installed hints.
    let source: PinnedSourceCodeLocation = outputs.source.clone();
    let preferred = Arc::clone(&spec.preferred_build_source);
    let env_ref = spec.env_ref.clone();
    let source_hints = spec.installed_source_hints.clone();
    let mapper = ComputeCtx::declare_join_closure(
        async move |bctx: &mut ComputeCtx, output: CondaOutput| {
            assemble_source_record(bctx, &source, &output, &preferred, &env_ref, &source_hints)
                .await
        },
    );
    let records = ctx
        .try_compute_join(outputs.outputs.iter().cloned(), mapper)
        .await?;

    Ok(Arc::new(records))
}

/// Map a `SourceMetadataError` into `SourceRecordError`.
fn map_source_metadata_error(err: SourceMetadataError) -> SourceRecordError {
    match err {
        SourceMetadataError::BuildBackendMetadata(e) => SourceRecordError::BuildBackendMetadata(e),
        SourceMetadataError::SourceRecord(e) => e,
        SourceMetadataError::PackageNotProvided(e) => SourceRecordError::PackageNotProvided(e),
        SourceMetadataError::SourceCheckout(e) => SourceRecordError::SourceCheckout(e),
    }
}

/// `LifecycleKind` for [`ResolveSourcePackageKey`].
struct SourceMetadataReporterLifecycle;

impl LifecycleKind for SourceMetadataReporterLifecycle {
    type Reporter<'r> = dyn SourceMetadataReporter + 'r;
    type Id = OperationId;
    type Env = SourceMetadataReporterSpec;

    fn queue<'r>(
        reporter: Option<&'r Self::Reporter<'r>>,
        env: &Self::Env,
    ) -> Option<Active<'r, Self::Reporter<'r>, Self::Id>> {
        reporter.map(|r| Active {
            reporter: r,
            id: r.on_queued(env),
        })
    }

    fn on_started<'r>(active: &Active<'r, Self::Reporter<'r>, Self::Id>) {
        active.reporter.on_started(active.id);
    }

    fn on_finished<'r>(active: Active<'r, Self::Reporter<'r>, Self::Id>) {
        active.reporter.on_finished(active.id);
    }
}
