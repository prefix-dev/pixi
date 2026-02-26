//! See [`WorkDirKey`] for more information.

use std::{
    collections::BTreeMap,
    ffi::OsStr,
    hash::{Hash, Hasher},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pixi_record::PinnedSourceSpec;
use rattler_conda_types::{PackageName, Platform};
use xxhash_rust::xxh3::Xxh3;

use crate::SourceCheckout;

#[derive(derive_more::From)]
pub enum SourceRecordOrCheckout {
    /// A source record that has not been checked out yet.
    Record {
        pinned: PinnedSourceSpec,
        package_name: PackageName,
    },

    /// A source checkout that has already been checked out.
    Checkout { checkout: SourceCheckout },
}

impl SourceRecordOrCheckout {
    pub fn pinned(&self) -> &PinnedSourceSpec {
        match self {
            SourceRecordOrCheckout::Record { pinned, .. } => pinned,
            SourceRecordOrCheckout::Checkout { checkout } => &checkout.pinned,
        }
    }
}

/// A key to uniquely identify a work directory. If there is a source build with
/// the same key, they will share the same working directory.
///
/// TODO: Should we use references instead of owned types here?
pub struct WorkDirKey {
    /// The location of the source
    pub source: SourceRecordOrCheckout,

    /// The platform the dependency will run on
    pub host_platform: Platform,

    /// The build backend name
    /// TODO: Maybe we should also include the version?
    pub build_backend: String,
}

impl WorkDirKey {
    pub fn key(&self) -> String {
        let mut hasher = Xxh3::new();
        self.source.pinned().to_string().hash(&mut hasher);
        self.host_platform.to_string().hash(&mut hasher);
        self.build_backend.hash(&mut hasher);
        let unique_key = URL_SAFE_NO_PAD.encode(hasher.finish().to_ne_bytes());

        let name: Option<String> = match &self.source {
            SourceRecordOrCheckout::Record { package_name, .. } => {
                Some(package_name.as_normalized().to_string())
            }
            SourceRecordOrCheckout::Checkout { checkout } => checkout
                .path
                .as_std_path()
                .file_name()
                .and_then(OsStr::to_str)
                .map(|s| s.to_lowercase()),
        };

        match name {
            Some(name) => format!("{name}-{unique_key}"),
            None => unique_key,
        }
    }

    pub fn variant_key(variant_key: &BTreeMap<String, impl Hash>) -> String {
        let mut hasher = Xxh3::new();
        for (key, value) in variant_key {
            key.hash(&mut hasher);
            value.hash(&mut hasher);
        }
        URL_SAFE_NO_PAD.encode(hasher.finish().to_ne_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixi_git::sha::GitSha;
    use pixi_path::AbsPathBuf;
    use pixi_record::{PinnedGitCheckout, PinnedGitSpec, PinnedSourceSpec};
    use pixi_spec::GitReference;
    use rattler_conda_types::Platform;
    use url::Url;

    #[test]
    fn test_work_dir_key_case_insensitive() {
        // Test that work directory keys are consistent regardless of path casing
        // This is important on macOS where the filesystem is case-insensitive

        let platform = Platform::Linux64;
        let backend = "pixi-build-backend".to_string();

        // Use a valid 40-character commit hash for testing
        let commit_hash = "0123456789abcdef0123456789abcdef01234567";
        let commit: GitSha = commit_hash.parse().unwrap();

        // Create a checkout with uppercase path
        let uppercase_checkout = SourceCheckout {
            path: AbsPathBuf::new("/tmp/IGoR").unwrap(),
            pinned: PinnedSourceSpec::Git(PinnedGitSpec {
                git: Url::parse("https://example.com/repo.git").unwrap(),
                source: PinnedGitCheckout {
                    commit: commit.clone(),
                    subdirectory: None,
                    reference: GitReference::DefaultBranch,
                },
            }),
        };

        let key_uppercase = WorkDirKey {
            source: SourceRecordOrCheckout::Checkout {
                checkout: uppercase_checkout,
            },
            host_platform: platform,
            build_backend: backend.clone(),
        };

        // Create a checkout with lowercase path
        let lowercase_checkout = SourceCheckout {
            path: AbsPathBuf::new("/tmp/igor").unwrap(),
            pinned: PinnedSourceSpec::Git(PinnedGitSpec {
                git: Url::parse("https://example.com/repo.git").unwrap(),
                source: PinnedGitCheckout {
                    commit,
                    subdirectory: None,
                    reference: GitReference::DefaultBranch,
                },
            }),
        };

        let key_lowercase = WorkDirKey {
            source: SourceRecordOrCheckout::Checkout {
                checkout: lowercase_checkout,
            },
            host_platform: platform,
            build_backend: backend,
        };

        // Both should generate the same key (with lowercase directory name)
        assert_eq!(key_uppercase.key(), key_lowercase.key());
        assert!(key_uppercase.key().starts_with("igor-"));
    }
}
