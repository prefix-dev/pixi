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
    /// Returns `None` if both are empty or not provided.
    pub fn compute(
        config: Option<&serde_json::Value>,
        target_config: Option<&OrderMap<TargetSelector, serde_json::Value>>,
    ) -> Option<Self> {
        // Treat None and empty the same - only compute a hash if there's actual configuration
        let has_config = config.is_some();
        let has_target_config = target_config.is_some_and(|c| !c.is_empty());

        if !has_config && !has_target_config {
            return None;
        }

        // Use empty JSON object for None/empty values to ensure consistent hashing
        let empty_json = serde_json::Value::Object(Default::default());
        let empty_target: OrderMap<TargetSelector, serde_json::Value> = OrderMap::default();

        let config = config.unwrap_or(&empty_json);
        let target_config = target_config
            .filter(|c| !c.is_empty())
            .unwrap_or(&empty_target);

        let mut hasher = Xxh3::new();
        StableHashBuilder::new()
            .field("config", &StableJson::new(config))
            .field(
                "target_config",
                &StableMap::new(target_config.iter().map(|(k, v)| (k, StableJson::new(v)))),
            )
            .finish(&mut hasher);
        Some(Self(hasher.finish()))
    }
}
