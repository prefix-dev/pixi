#![deny(missing_docs)]
//! A crate to deal with glob patterns in Pixi.
//! And the caching thereof.

mod glob_hash;
mod glob_hash_cache;
mod glob_mtime;
mod glob_set;
mod glob_set_ignore;

pub use glob_hash::{GlobHash, GlobHashError};
pub use glob_hash_cache::{GlobHashCache, GlobHashKey};
pub use glob_mtime::{GlobModificationTime, GlobModificationTimeError};
pub use glob_set::{GlobSet, GlobSetError};
pub use glob_set_ignore::{GlobSetIgnore, GlobSetIgnoreError};
