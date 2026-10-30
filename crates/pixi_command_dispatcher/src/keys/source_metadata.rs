//! Compute-engine Key that pins a source, queries its build backend,
//! and returns the [`CondaOutput`]s matching a given package name.
//! Outputs are not resolved into
//! [`SourceRecord`]s here;
//! [`crate::keys::SolvePixiEnvironmentKey`] does that after scheduling
//! the per-source build/host solves.

use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

use derive_more::Display;
use pixi_build_types::procedures::conda_outputs::CondaOutput;
use pixi_compute_engine::{ComputeCtx, Key};
use pixi_record::{PinnedSourceSpec, SourceRecord};
use pixi_spec::SourceLocationSpec;
use rattler_conda_types::PackageName;
use tracing::instrument;

use crate::{
    BuildBackendMetadataKey, BuildBackendMetadataSpec, EnvironmentRef, InlinePackage,
    PackageNotProvidedError, SourceMetadataError, build::PinnedSourceCodeLocation,
};
use pixi_compute_sources::SourceCheckoutExt;

/// The result of resolving source metadata for all variants of a package.
///
/// `Hash`/`Eq` use pointer identity for `records` rather than recursing
/// into each `SourceRecord`. `SourceRecord` derives `Hash` over its
/// full `PackageRecord` plus `build_packages` / `host_packages`, which
/// recurse into more `SourceRecord`s — a single `dyn_hash` on
/// `SolveCondaSpec::source_repodata` was walking O(metadata × records ×
/// record_fields) of data on every compute-graph lookup. Producers
/// already maintain Arc identity: each unique `Arc<SourceRecord>` is
/// created once inside `assemble_source_record_inner`, cached as the
/// value of `ResolveSourcePackageKey`, and `Arc::clone`d through every
/// consumer (including the `walk_and_resolve` push into `source_repodata`).
#[derive(Clone, Debug)]
pub struct SourceMetadata {
    /// Manifest and optional build source location for this metadata.
    pub source: PinnedSourceCodeLocation,

    /// The metadata that was acquired from the build backend.
    pub records: Vec<Arc<SourceRecord>>,
}

impl Hash for SourceMetadata {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.source.hash(state);
        self.records.len().hash(state);
        for r in &self.records {
            (Arc::as_ptr(r) as usize).hash(state);
        }
    }
}

impl PartialEq for SourceMetadata {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
            && self.records.len() == other.records.len()
            && self
                .records
                .iter()
                .zip(&other.records)
                .all(|(a, b)| Arc::ptr_eq(a, b))
    }
}

impl Eq for SourceMetadata {}

/// Input to [`SourceMetadataKey`].
///
/// `source_location` is unpinned; the compute body pins via
/// `ctx.pin_and_checkout` as its first step. Each `SourceMetadataKey`
/// runs in its own spawned task, so concurrent fan-out is safe.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct SourceMetadataSpec {
    /// The package whose outputs we want.
    pub package: PackageName,
    /// Unpinned source location; pinned inside the compute unless
    /// `manifest_pin_override` carries a compatible pin (see below).
    pub source_location: SourceLocationSpec,
    /// Optional override for the package's build source.
    pub preferred_build_source: Option<PinnedSourceSpec>,
    /// Optional caller-supplied pin for the manifest source. When set
    /// and compatible with `source_location` (see
    /// [`PinnedSourceSpec::matches_source_spec`]), the compute body
    /// uses [`checkout_pinned_source`](crate::SourceCheckoutExt::checkout_pinned_source)
    /// at this exact pin instead of resolving `source_location` afresh.
    /// Used to thread a previously-locked git/url commit through a
    /// re-lock so commits don't drift when the manifest still points
    /// at the same branch / ref.
    pub manifest_pin_override: Option<PinnedSourceSpec>,
    /// Environment context (channels, build env, variants,
    /// exclude_newer, channel_priority).
    pub env_ref: EnvironmentRef,
    /// Inline package definition for this dependency. When set,
    /// the backend is built from this manifest instead of on-disk discovery.
    pub inline: Option<InlinePackage>,
}

/// What [`SourceMetadataKey`] returns: the pinned source location
/// plus the outputs matching the requested package name.
#[derive(Debug, Clone)]
pub struct SourceOutputs {
    /// Pinned manifest + optional build source location.
    pub source: PinnedSourceCodeLocation,
    /// Outputs for `package` in backend order. Each carries declared
    /// build/host/run deps; none are resolved yet.
    pub outputs: Vec<CondaOutput>,
}

/// Compute-engine Key for "outputs of this package from this source".
/// Dedups on `(package, source_location, preferred_build_source, env_ref)`.
#[derive(Clone, Debug, Display)]
#[display("{}/{}", _0.package.as_source(), _0.source_location)]
pub struct SourceMetadataKey(pub Arc<SourceMetadataSpec>);

impl SourceMetadataKey {
    pub fn new(spec: SourceMetadataSpec) -> Self {
        Self(Arc::new(spec))
    }
}

impl Hash for SourceMetadataKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl PartialEq for SourceMetadataKey {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || *self.0 == *other.0
    }
}

impl Eq for SourceMetadataKey {}

impl Key for SourceMetadataKey {
    type Value = Result<Arc<SourceOutputs>, SourceMetadataError>;

    #[instrument(
        skip_all,
        name = "source-metadata",
        fields(
            source = %self.0.source_location,
            name = %self.0.package.as_source(),
            platform = %self.0.env_ref.display_platform(),
        )
    )]
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let spec = self.0.clone();

        // Use the caller-supplied manifest pin when it is compatible
        // with the requested source location; otherwise resolve the
        // location fresh. The override is the path that lets a re-lock
        // reuse a previously-locked git commit instead of drifting to
        // whatever the branch points at today.
        let checkout = match spec
            .manifest_pin_override
            .as_ref()
            .filter(|pin| pin.matches_source_spec(&spec.source_location))
        {
            Some(pin) => ctx
                .checkout_pinned_source(pin.clone())
                .await
                .map_err(SourceMetadataError::from)?,
            None => ctx
                .pin_and_checkout(spec.source_location.clone())
                .await
                .map_err(SourceMetadataError::from)?,
        };

        let backend_metadata_spec = BuildBackendMetadataSpec {
            manifest_source: checkout.pinned,
            preferred_build_source: spec.preferred_build_source.clone(),
            env_ref: spec.env_ref.clone(),
            build_string_prefix: None,
            build_number: None,
            // Carry the inline package definition so backend
            // discovery uses it instead of reading a manifest from the checkout.
            inline: spec.inline.clone(),
        };
        let build_backend_metadata = ctx
            .compute(&BuildBackendMetadataKey::new(backend_metadata_spec))
            .await
            .map_err(SourceMetadataError::BuildBackendMetadata)?;

        let matching: Vec<CondaOutput> = build_backend_metadata
            .metadata
            .outputs
            .iter()
            .filter(|o| o.metadata.name == spec.package)
            .cloned()
            .collect();

        if matching.is_empty() {
            let available_names = build_backend_metadata
                .metadata
                .outputs
                .iter()
                .map(|output| output.metadata.name.clone());
            return Err(SourceMetadataError::PackageNotProvided(
                PackageNotProvidedError::new(
                    spec.package.clone(),
                    build_backend_metadata.source.manifest_source().clone(),
                    available_names,
                ),
            ));
        }

        Ok(Arc::new(SourceOutputs {
            source: build_backend_metadata.source.clone(),
            outputs: matching,
        }))
    }
}
