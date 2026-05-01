//! Backwards-compat shim for source records loaded from pre-v7 lock
//! files. Pre-v7 lock files do not store the resolved build/host
//! environments of source packages, so any
//! [`UnresolvedSourceRecord`](pixi_record::UnresolvedSourceRecord)
//! produced by [`LockFileResolver`](pixi_record::LockFileResolver) from
//! such a file carries empty `build_packages` / `host_packages`. The
//! current v7 satisfiability path treats empty slices as a re-lock
//! signal; this module provides a way to compute those envs lazily
//! from the build backend instead, so v6 lock files keep working
//! without forcing a re-lock.

mod cache;
mod key;
mod reify;

pub use reify::reify_legacy_source_envs;
