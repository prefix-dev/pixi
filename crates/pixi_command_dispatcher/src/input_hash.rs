use pixi_build_types::ProjectModelV1;
use serde::{Deserialize, Serialize};
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
