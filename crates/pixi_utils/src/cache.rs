use std::hash::{DefaultHasher, Hash, Hasher};

use rattler_conda_types::{MatchSpec, Platform};

/// A hash that uniquely identifies an environment.
#[derive(Hash)]
pub struct EnvironmentHash {
    pub command: String,
    pub specs: Vec<MatchSpec>,
    pub channels: Vec<String>,
    pub platform: Platform,
}

impl EnvironmentHash {
    /// Creates a new environment hash.
    pub fn new(
        command: String,
        specs: Vec<MatchSpec>,
        channels: Vec<String>,
        platform: Platform,
    ) -> Self {
        Self {
            command,
            specs,
            channels,
            platform,
        }
    }

    /// Returns the name of the environment.
    pub fn name(&self) -> String {
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        let hash = hasher.finish();
        format!("{}-{:x}", &self.command, hash)
    }
}
