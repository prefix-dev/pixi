mod build_cache;
mod source_metadata_cache;

use std::{
    ffi::OsStr,
    hash::{DefaultHasher, Hash, Hasher},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
pub use build_cache::{BuildCache, BuildCacheError, BuildInput, CachedBuild, SourceInfo};
pub use source_metadata_cache::{
    CachedCondaMetadata, SourceMetadataCache, SourceMetadataError, SourceMetadataInput,
};

use crate::build::SourceCheckout;

/// Constructs a name for a cache directory for the given source checkout.
fn source_checkout_cache_key(source: &SourceCheckout) -> String {
    let mut hasher = DefaultHasher::new();
    source.pinned.to_string().hash(&mut hasher);
    let unique_key = URL_SAFE_NO_PAD.encode(hasher.finish().to_ne_bytes());
    match source.path.file_name().and_then(OsStr::to_str) {
        Some(name) => format!("{}-{}", name, unique_key),
        None => unique_key,
    }
}
