use pixi_build_types::ProjectModelV1;
use rattler_digest::Sha256Hash;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};
use xxhash_rust::xxh3::Xxh3;

#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct ProjectModelHash(u64);

impl From<&'_ ProjectModelV1> for ProjectModelHash {
    fn from(value: &'_ ProjectModelV1) -> Self {
        let mut hasher = Xxh3::new();
        value.hash(&mut hasher);
        Self(hasher.finish())
    }
}

/// Describes the combined hashes of a set of files  
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GlobHash {
    pub globs: BTreeSet<String>,
    pub hash: Sha256Hash,
}
