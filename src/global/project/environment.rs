use miette::Diagnostic;
use regex::Regex;
use serde::{self, Deserialize, Deserializer, Serialize};
use std::{fmt, str::FromStr};
use thiserror::Error;

/// Represents the name of an environment.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize)]
pub(crate) struct EnvironmentName(String);

impl EnvironmentName {
    /// Returns the name of the environment.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EnvironmentName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl PartialEq<str> for EnvironmentName {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl<'de> Deserialize<'de> for EnvironmentName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let name = String::deserialize(deserializer)?;
        name.parse().map_err(serde::de::Error::custom)
    }
}

impl FromStr for EnvironmentName {
    type Err = ParseEnvironmentNameError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        static REGEX: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        let regex = REGEX
            .get_or_init(|| Regex::new(r"^[a-z0-9-]+$").expect("Regex should be able to compile"));

        if !regex.is_match(s) {
            // Return an error if the string does not match the regex
            return Err(ParseEnvironmentNameError {
                attempted_parse: s.to_string(),
            });
        }
        Ok(EnvironmentName(s.to_string()))
    }
}

/// Represents an error that occurs when parsing an environment name.
///
/// This error is returned when a string fails to be parsed as an environment name.
#[derive(Debug, Clone, Error, Diagnostic, PartialEq)]
#[error("Failed to parse environment name '{attempted_parse}', please use only lowercase letters, numbers and dashes")]
pub struct ParseEnvironmentNameError {
    /// The string that was attempted to be parsed.
    pub attempted_parse: String,
}
