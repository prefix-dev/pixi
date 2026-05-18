//! Engine-tracked cache-directory lookup.
//!
//! Every cache root resolves to `<base>/<name>`, where `base` is either
//! the global cache root or the workspace root, and `name` is a fixed
//! path component. Each cache type also declares an optional
//! environment variable that overrides the path entirely. Resolution
//! flows through the compute engine so dependencies on cache
//! configuration and on individual environment variables are recorded
//! in the dependency graph.
//!
//! Two Keys split the responsibilities:
//!
//! - [`CacheDirsKey`] holds the two anchor paths and the registry of
//!   programmatic overrides. Single-write at engine construction.
//! - [`CacheDirKey<L>`] is the computed Key that resolves one cache
//!   type's path; it reads [`CacheDirsKey`] and (when applicable) one
//!   `EnvVar` from `pixi_compute_env_vars`.
//!
//! # Example
//!
//! Declare a marker for the cache, inject the anchors, then resolve
//! through the engine:
//!
//! ```
//! use std::{collections::HashMap, sync::Arc};
//! use pixi_compute_engine::ComputeEngine;
//! use pixi_compute_env_vars::EnvVarsKey;
//! use pixi_compute_cache_dirs::{
//!     CacheBase, CacheDirKey, CacheDirs, CacheDirsKey, CacheLocation,
//! };
//! use pixi_path::AbsPathBuf;
//!
//! struct GitCache;
//! impl CacheLocation for GitCache {
//!     fn name() -> &'static str { "git-cache-v0" }
//!     fn base() -> CacheBase { CacheBase::Root }
//!     fn env_override() -> Option<&'static str> { Some("PIXI_GIT_CACHE_DIR") }
//! }
//!
//! let engine = ComputeEngine::new();
//! let root = AbsPathBuf::new(std::env::temp_dir()).unwrap().into_assume_dir();
//! engine.inject(CacheDirsKey, Arc::new(CacheDirs::new(root.clone())));
//! engine.inject(EnvVarsKey, Arc::new(HashMap::new()));
//!
//! # tokio_test::block_on(async {
//! let dir = engine.compute(&CacheDirKey::<GitCache>::new()).await.unwrap();
//! assert_eq!(dir.as_std_path(), root.join("git-cache-v0").as_std_path());
//! # });
//! ```

mod cache_dirs;
mod location;

pub use cache_dirs::{CacheBase, CacheDirs, CacheDirsKey};
pub use location::{CacheDirKey, CacheDirsExt, CacheLocation};
