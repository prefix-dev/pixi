use crate::Project;
use itertools::Itertools;
use std::collections::HashMap;

// Setting a base prefix for the pixi package
const ENV_PREFIX: &str = "PIXI_PACKAGE_";

impl Project {
    /// Returns environment variables and their values that should be injected when running a command.
    pub fn get_metadata_env(&self) -> HashMap<String, String> {
        HashMap::from_iter([
            (
                format!("{ENV_PREFIX}ROOT"),
                self.root().to_string_lossy().into_owned(),
            ),
            (format!("{ENV_PREFIX}NAME"), self.name().to_string()),
            (
                format!("{ENV_PREFIX}MANIFEST"),
                self.manifest_path().to_string_lossy().into_owned(),
            ),
            (
                format!("{ENV_PREFIX}PLATFORMS"),
                self.platforms().iter().map(|plat| plat.as_str()).join(","),
            ),
            (
                format!("{ENV_PREFIX}VERSION"),
                self.version()
                    .as_ref()
                    .map_or("NO_VERSION_SPECIFIED".to_string(), |version| {
                        version.to_string()
                    }),
            ),
            ("PIXI_PROMPT".to_string(), format!("({}) ", self.name())),
        ])
    }
}
