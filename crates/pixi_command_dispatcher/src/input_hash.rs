use ordermap::OrderMap;
use pixi_build_types::{ProjectModel, TargetSelector};
use pixi_stable_hash::{StableHashBuilder, json::StableJson, map::StableMap};
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

/// A hash of the build configuration (from `[package.build.config]` and
/// `[package.build.target.<selector>.config]`).
///
/// This is used to detect when the build configuration changes, which should
/// invalidate the metadata cache even if the project model hasn't changed.
#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct ConfigurationHash(u64);

impl ConfigurationHash {
    /// Computes a hash from the configuration and target configuration.
    pub fn compute(
        configuration: Option<&serde_json::Value>,
        target_configuration: Option<&OrderMap<TargetSelector, serde_json::Value>>,
    ) -> Option<Self> {
        // Only compute a hash if there's any configuration
        if configuration.is_none() && target_configuration.is_none() {
            return None;
        }

        let mut hasher = Xxh3::new();
        StableHashBuilder::new()
            .field("configuration", &configuration.map(StableJson::new))
            .field(
                "target_configuration",
                &target_configuration.map(|config| {
                    StableMap::new(config.iter().map(|(k, v)| (k, StableJson::new(v))))
                }),
            )
            .finish(&mut hasher);
        Some(Self(hasher.finish()))
    }
}
