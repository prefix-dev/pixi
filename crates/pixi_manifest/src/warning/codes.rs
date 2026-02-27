use std::fmt::Display;
use serde::Deserialize;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WarningCode {
    /// A task input file is missing
    TaskInputMissing,
    /// A project feature is deprecated
    ProjectDeprecated,
}

impl WarningCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            WarningCode::TaskInputMissing => "task-input-missing",
            WarningCode::ProjectDeprecated => "project-deprecated",
        }
    }

    pub fn short_code(&self) -> &'static str {
        match self {
            WarningCode::TaskInputMissing => "TI001",
            WarningCode::ProjectDeprecated => "PD001", // Example short code
        }
    }
}

impl Display for WarningCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
