//! Compute-engine Key that pins a source, queries its build backend,
//! and returns the [`CondaOutput`]s matching a given package name.
//! Outputs are not resolved into
//! [`SourceRecord`](pixi_record::SourceRecord)s here;
//! [`crate::keys::SolvePixiEnvironmentKey`] does that after scheduling
//! the per-source build/host solves.

use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

use derive_more::Display;
use pixi_build_types::procedures::conda_outputs::CondaOutput;
use pixi_compute_engine::{ComputeCtx, Key};
use pixi_record::PinnedSourceSpec;
use pixi_spec::SourceLocationSpec;
use rattler_conda_types::PackageName;
use tracing::instrument;

use crate::{
    BuildBackendMetadataKey, BuildBackendMetadataSpec, EnvironmentRef, PackageNotProvidedError,
    build::PinnedSourceCodeLocation, source_checkout::SourceCheckoutExt,
    source_metadata::SourceMetadataError,
};

/// Input to [`SourceMetadataKey`].
///
/// `source_location` is unpinned; the compute body pins via
/// `ctx.pin_and_checkout` as its first step. Each `SourceMetadataKey`
/// runs in its own spawned task, so concurrent fan-out is safe.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct SourceMetadataSpecV2 {
    /// The package whose outputs we want.
    pub package: PackageName,
    /// Unpinned source location; pinned inside the compute.
    pub source_location: SourceLocationSpec,
    /// Optional override for the package's build source.
    pub preferred_build_source: Option<PinnedSourceSpec>,
    /// Environment context (channels, build env, variants,
    /// exclude_newer, channel_priority).
    pub env_ref: EnvironmentRef,
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
pub struct SourceMetadataKey(pub Arc<SourceMetadataSpecV2>);

impl SourceMetadataKey {
    pub fn new(spec: SourceMetadataSpecV2) -> Self {
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

        let checkout = ctx
            .pin_and_checkout(spec.source_location.clone())
            .await
            .map_err(SourceMetadataError::from)?;

        let backend_metadata_spec = BuildBackendMetadataSpec {
            manifest_source: checkout.pinned,
            preferred_build_source: spec.preferred_build_source.clone(),
            env_ref: spec.env_ref.clone(),
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
