//! Datastructures and functions used for building packages from source.

mod build_environment;
mod work_dir_key;

use std::{
    ffi::OsStr,
    hash::{Hash, Hasher},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
pub use build_environment::BuildEnvironment;
pub(crate) use work_dir_key::WorkDirKey;
use xxhash_rust::xxh3::Xxh3;

use crate::SourceCheckout;

/// Constructs a name for a cache directory for the given source checkout.
pub(crate) fn source_checkout_cache_key(source: &SourceCheckout) -> String {
    let mut hasher = Xxh3::new();

    // If the source is immutable, we use the pinned definition of the source.
    // If the source is mutable, we instead hash the location of the source
    // checkout on disk. This ensures that we get different cache directories
    // for different source checkouts with different edits.
    if source.pinned.is_immutable() {
        source.pinned.to_string().hash(&mut hasher);
    } else {
        source.path.to_string_lossy().hash(&mut hasher);
    }

    let unique_key = URL_SAFE_NO_PAD.encode(hasher.finish().to_ne_bytes());
    match source.path.file_name().and_then(OsStr::to_str) {
        Some(name) => format!("{}-{}", name, unique_key),
        None => unique_key,
    }
}
