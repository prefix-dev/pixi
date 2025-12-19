use pixi_build_types::ProjectModel;
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};
use xxhash_rust::xxh3::Xxh3;

#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct ProjectModelHash(u64);

impl From<&'_ ProjectModel> for ProjectModelHash {
    fn from(value: &'_ ProjectModel) -> Self {
        let mut hasher = Xxh3::new();
        value.hash(&mut hasher);
        Self(hasher.finish())
    }
}
