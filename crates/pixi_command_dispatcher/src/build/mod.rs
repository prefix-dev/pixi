//! Datastructures and functions used for building packages from source.

mod build_cache;
mod build_environment;
pub mod conversion;
mod dependencies;
mod move_file;
pub(crate) mod source_metadata_cache;
mod work_dir_key;

use std::hash::{Hash, Hasher};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
pub use build_cache::{
    BuildCache, BuildCacheEntry, BuildCacheError, BuildHostEnvironment, BuildHostPackage,
    BuildInput, CachedBuild, CachedBuildSourceInfo, PackageBuildInputHash,
    PackageBuildInputHashBuilder,
};
pub use build_environment::BuildEnvironment;
pub use dependencies::{
    Dependencies, DependenciesError, DependencySource, KnownEnvironment, PixiRunExports, WithSource,
};
pub(crate) use move_file::{MoveError, move_file};
use pixi_record::PinnedSourceSpec;
use url::Url;
pub use work_dir_key::{SourceRecordOrCheckout, WorkDirKey};
use xxhash_rust::xxh3::Xxh3;

const KNOWN_SUFFIXES: [&str; 3] = [".git", ".tar.gz", ".zip"];

/// Try to deduce a name from a url.
fn pretty_url_name(url: &Url) -> String {
    if let Some(last_segment) = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
    {
        // Strip known suffixes
        for suffix in KNOWN_SUFFIXES {
            if let Some(segment) = last_segment.strip_suffix(suffix) {
                return segment.to_string();
            }
        }
        if !last_segment.is_empty() {
            return last_segment.to_string();
        }
    }

    if let Some(host) = url.host_str() {
        // If the URL has no path segments, we can use the host as a fallback
        host.to_string()
    } else {
        url.to_string()
    }
}

/// Constructs a name for a cache directory for the given source checkout.
///
/// For git and url sources, which have been pinned to specific checkouts, the
/// pin is included in the name (e.g. the commit or hash). You could include
/// multiple git sources with different hashes.
///
/// For path sources, only the path is used as there can only be one entry on
/// disk anyway.
pub(crate) fn source_checkout_cache_key(source: &PinnedSourceSpec) -> String {
    match source {
        PinnedSourceSpec::Url(url) => {
            format!("{}-{:x}", pretty_url_name(&url.url), url.sha256)
        }
        PinnedSourceSpec::Git(git) => {
            let name = pretty_url_name(&git.git);
            let hash = git.source.commit.to_short_string();
            if let Some(subdir) = &git.source.subdirectory {
                format!("{name}-{subdir}-{hash}",)
            } else {
                format!("{name}-{hash}",)
            }
        }
        PinnedSourceSpec::Path(path) => {
            let mut hasher = Xxh3::new();
            path.path.hash(&mut hasher);
            let unique_key = URL_SAFE_NO_PAD.encode(hasher.finish().to_ne_bytes());
            if let Some(file_name) = path.path.file_name() {
                format!("{}-{}", file_name, unique_key)
            } else {
                unique_key
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;

    use super::*;

    #[test]
    fn test_source_checkout_cache_key() {
        let urls = [
            "https://example.com/package-1.0.0.tar.gz",
            "https://example.com/package-1.0.0.tar.gz#hash",
            "https://example.com/package-1.0.0.tar.gz?query=param",
            "git://git@github.com/user/repo.git",
            "git://git@github.com/user/repo.git#subdir=path",
            "https://www.google.com",
        ];
        insta::assert_debug_snapshot!(
            urls.into_iter()
                .map(|url| {
                    let parsed_url = Url::parse(url).unwrap();
                    (url, pretty_url_name(&parsed_url))
                })
                .collect::<IndexMap<_, _>>()
        );
    }
}
