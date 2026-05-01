//! On-disk caches used across the dispatcher.
//!
//! - [`common`] — generic [`MetadataCache`] trait framework with locking,
//!   JSON serialization, and optimistic-concurrency writes.
//! - [`backend_metadata`] — caches the result of `conda/outputs` calls so
//!   the build backend isn't re-invoked for unchanged source.
//! - [`artifact`] — content-addressed cache of built `.conda` artifacts.
//!   A hit short-circuits the entire backend build.
//! - [`workspace`] — per-package backend workspace (CMake/autotools state)
//!   keyed on the full dep set, giving each backend a stable incremental
//!   build directory across runs.
//! - [`dirs`] — [`CacheDirs`] enumerates every on-disk cache root the
//!   dispatcher knows about.

pub mod artifact;
pub mod backend_metadata;
pub mod common;
pub mod dirs;
pub mod workspace;

pub use artifact::{
    ArtifactCache, ArtifactCacheError, ArtifactCacheKey, ArtifactSidecar, CachedArtifact,
    compute_artifact_cache_key,
};
pub use backend_metadata::{
    BuildBackendMetadataCache, BuildBackendMetadataCacheEntry, BuildBackendMetadataCacheError,
    BuildBackendMetadataCacheKey,
};
pub use common::{
    CacheEntry, CacheError, CacheKey, CacheKeyString, CacheRevision, MetadataCache,
    MetadataCacheEntry, MetadataCacheKey, VersionedCacheEntry, WriteResult,
};
pub use dirs::CacheDirs;
pub use workspace::{WorkspaceCache, WorkspaceGuard, WorkspaceKey, compute_workspace_key};
