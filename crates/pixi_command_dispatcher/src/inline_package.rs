//! Inline package definitions.
//!
//! An inline package definition lives in a consuming workspace's manifest
//! rather than in a `pixi.toml` at the source. The build pipeline still checks
//! out the source code, but instead of discovering a manifest on disk it uses
//! the inline [`PackageManifest`] carried here.
//!
//! The definition is threaded directly through the compute specs that need it
//! (the solve, metadata, instantiation and build keys) rather than looked up
//! from a side registry, so the inline content participates in every
//! content-addressed cache key. `content_hash` provides the cheap, hashable
//! identity: it is a deterministic hash of `(dependency name, package
//! manifest)` computed at manifest-assembly time, so it distinguishes two
//! inline definitions that resolve to the same source location and changes
//! whenever the definition is edited.

use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
    path::Path,
    sync::Arc,
};

use pixi_build_discovery::{DiscoveredBackend, DiscoveryError};
use pixi_compute_engine::ComputeCtx;
use pixi_manifest::{InlineContentHash, PackageManifest, WorkspaceManifest};
use pixi_spec::SpecConversionError;
use rattler_conda_types::{ChannelConfig, PackageName};

use crate::{discovered_backend::DiscoveredBackendKey, injected_config::ChannelConfigKey};

/// An inline package definition together with the workspace manifest that
/// declared it. Both are needed to construct a build backend without reading a
/// manifest from disk.
///
/// `Hash` and `Eq` go through [`Self::content_hash`] alone: the manifests behind
/// the `Arc`s are not themselves hashable, and the hash is a faithful content
/// fingerprint, so two `InlinePackage`s with the same hash are treated as
/// identical by the compute engine.
#[derive(Clone, Debug)]
pub struct InlinePackage {
    /// The inline package manifest.
    pub manifest: Arc<PackageManifest>,
    /// The consuming workspace manifest (used for channels and workspace root).
    pub workspace: Arc<WorkspaceManifest>,
    /// Content fingerprint of `(dependency name, package manifest)`.
    pub content_hash: InlineContentHash,
}

impl InlinePackage {
    /// Build a [`DiscoveredBackend`] from the inline manifest, skipping on-disk
    /// discovery. The synthetic manifest is anchored at the source *directory*
    /// (a `path` source may point straight at a file such as a recipe.yaml, in
    /// which case the directory is its parent), so the backend builds the code
    /// that was checked out.
    pub fn discovered_backend(
        &self,
        source_path: &Path,
        channel_config: &ChannelConfig,
    ) -> Result<DiscoveredBackend, SpecConversionError> {
        let source_dir = if source_path.is_file() {
            source_path.parent().unwrap_or(source_path)
        } else {
            source_path
        };
        DiscoveredBackend::from_inline_package_and_workspace(
            &self.manifest,
            &self.workspace,
            source_dir,
            channel_config,
        )
    }
}

impl PartialEq for InlinePackage {
    fn eq(&self, other: &Self) -> bool {
        self.content_hash == other.content_hash
    }
}

impl Eq for InlinePackage {}

impl Hash for InlinePackage {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.content_hash.hash(state);
    }
}

/// Serialize an optional inline definition as just its content hash. Used via
/// `#[serde(serialize_with = ...)]` on the `inline` field of the
/// [`BuildBackendMetadataSpec`](crate::BuildBackendMetadataSpec)-family structs,
/// the only ones that derive `Serialize` for their event JSON. The manifests
/// behind the `Arc`s are not serializable and are never reconstructed from this
/// output (only `Hash`/`Eq` drive cache identity), so the content hash alone is
/// a faithful, self-contained representation.
pub(crate) fn serialize_optional_content_hash<S: serde::Serializer>(
    inline: &Option<InlinePackage>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match inline {
        Some(inline) => serializer.serialize_some(&inline.content_hash.as_u64()),
        None => serializer.serialize_none(),
    }
}

/// Convert the inline package definitions carried on a discovered backend into
/// dispatcher [`InlinePackage`]s, keyed by dependency name.
///
/// These are the definitions the discovered package declares for its *own*
/// dependencies; the caller matches them by name against the dependencies the
/// backend reports (nested build/host env seeds and transitive run deps).
pub(crate) fn inline_packages_from_backend(
    backend: &DiscoveredBackend,
) -> Arc<BTreeMap<PackageName, InlinePackage>> {
    let Some(discovered) = backend.inline_packages.as_ref() else {
        return Arc::new(BTreeMap::new());
    };
    Arc::new(
        discovered
            .packages
            .iter()
            .map(|(name, inline)| {
                (
                    name.clone(),
                    InlinePackage {
                        manifest: Arc::new(inline.manifest.clone()),
                        workspace: discovered.workspace.clone(),
                        content_hash: inline.content_hash,
                    },
                )
            })
            .collect(),
    )
}

/// Discover the build backend for a checked-out source, honoring an inline
/// package definition.
///
/// When `inline` is set the backend is built from the inline manifest with its
/// paths anchored at `source_path`, skipping on-disk discovery entirely.
/// Otherwise discovery falls back to the content-addressed
/// [`DiscoveredBackendKey`], which reads a manifest from the checkout. The
/// returned [`DiscoveryError`] is mapped by each caller into its own error type.
pub(crate) async fn discover_backend(
    ctx: &mut ComputeCtx,
    source_path: &Path,
    inline: Option<&InlinePackage>,
) -> Result<Arc<DiscoveredBackend>, Arc<DiscoveryError>> {
    match inline {
        Some(inline) => {
            let channel_config = ctx.compute(&ChannelConfigKey).await;
            inline
                .discovered_backend(source_path, &channel_config)
                .map(Arc::new)
                .map_err(|err| Arc::new(DiscoveryError::SpecConversionError(err)))
        }
        None => ctx.compute(&DiscoveredBackendKey::new(source_path)).await,
    }
}
