use std::{borrow::Borrow, fmt, str::FromStr};

use miette::Diagnostic;
use regex::Regex;
use serde_with::serde_derive::Deserialize;
use thiserror::Error;

#[derive(Deserialize, Debug, Clone, Eq, PartialEq, Hash)]
pub(crate) struct EnvironmentName(String);

impl EnvironmentName {
    /// Returns the name of the environment.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for EnvironmentName {
    fn borrow(&self) -> &str {
        self.as_str()
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

#[derive(Debug, Clone, Error, Diagnostic, PartialEq)]
#[error("Failed to parse environment name '{attempted_parse}', please use only lowercase letters, numbers and dashes")]
pub struct ParseEnvironmentNameError {
    /// The string that was attempted to be parsed.
    pub attempted_parse: String,
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
