use crate::Project;
use itertools::Itertools;
use std::collections::HashMap;

use super::{manifest::EnvironmentName, Environment};

// Setting a base prefix for the pixi package
const PROJECT_PREFIX: &str = "PIXI_PROJECT_";

impl Project {
    /// Returns environment variables and their values that should be injected when running a command.
    pub fn get_metadata_env(&self) -> HashMap<String, String> {
        HashMap::from_iter([
            (
                format!("{PROJECT_PREFIX}ROOT"),
                self.root().to_string_lossy().into_owned(),
            ),
            (format!("{PROJECT_PREFIX}NAME"), self.name().to_string()),
            (
                format!("{PROJECT_PREFIX}MANIFEST"),
                self.manifest_path().to_string_lossy().into_owned(),
            ),
            (
                format!("{PROJECT_PREFIX}VERSION"),
                self.version()
                    .as_ref()
                    .map_or("NO_VERSION_SPECIFIED".to_string(), |version| {
                        version.to_string()
                    }),
            ),
        ])
    }
}

const ENV_PREFIX: &str = "PIXI_ENVIRONMENT_";

impl Environment<'_> {
    /// Returns environment variables and their values that should be injected when running a command.
    pub fn get_metadata_env(&self) -> HashMap<String, String> {
        let env_name = match self.name() {
            EnvironmentName::Named(name) => {
                format!("{}:{}", self.project().name(), name)
            }
            EnvironmentName::Default => self.project().name().to_string(),
        };
        HashMap::from_iter([
            (format!("{ENV_PREFIX}NAME"), self.name().to_string()),
            (
                format!("{ENV_PREFIX}PLATFORMS"),
                self.platforms().iter().map(|plat| plat.as_str()).join(","),
            ),
            ("PIXI_PROMPT".to_string(), format!("({}) ", env_name)),
        ])
    }
}
