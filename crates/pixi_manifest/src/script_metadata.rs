//! Parser for conda-script metadata embedded in script files.
//!
//! This module implements parsing of script metadata following the conda-script
//! specification. Metadata is embedded in comment blocks like:
//!
//! ```python
//! # /// conda-script
//! # [dependencies]
//! # python = "3.12.*"
//! # requests = "*"
//! # [script]
//! # channels = ["conda-forge"]
//! # entrypoint = "python"
//! # /// end-conda-script
//! ```

use indexmap::IndexMap;
use rattler_conda_types::{MatchSpec, NamedChannelOrUrl, Platform};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::str::FromStr;
use thiserror::Error;

/// Errors that can occur when parsing script metadata
#[derive(Debug, Error, miette::Diagnostic)]
pub enum ScriptMetadataError {
    #[error("Failed to read script file: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Failed to parse TOML metadata: {0}")]
    TomlError(#[from] toml_edit::de::Error),

    #[error("No conda-script metadata block found in script")]
    NoMetadataFound,

    #[error("Invalid matchspec '{0}': {1}")]
    InvalidMatchSpec(String, String),

    #[error("Invalid channel URL '{0}': {1}")]
    InvalidChannel(String, String),

    #[error("Malformed metadata block: {0}")]
    MalformedBlock(String),
}

/// Represents the complete conda-script metadata
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScriptMetadata {
    /// Package dependencies
    #[serde(default)]
    pub dependencies: DependenciesTable,

    /// Script configuration
    pub script: ScriptTable,
}

/// Dependencies table with platform-specific support
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct DependenciesTable {
    /// Default dependencies (applies to all platforms)
    #[serde(flatten)]
    pub default: IndexMap<String, String>,

    /// Platform-specific dependencies
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<IndexMap<String, IndexMap<String, String>>>,
}

/// Script configuration table
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScriptTable {
    /// List of conda channels
    pub channels: Vec<String>,

    /// Command to run the script
    #[serde(default)]
    pub entrypoint: Option<String>,

    /// Platform-specific script configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<IndexMap<String, PlatformScriptConfig>>,
}

/// Platform-specific script configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlatformScriptConfig {
    /// Platform-specific entrypoint
    #[serde(default)]
    pub entrypoint: Option<String>,
}

impl ScriptMetadata {
    /// Parse metadata from a script file
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ScriptMetadataError> {
        let file = std::fs::File::open(path.as_ref())?;
        let reader = BufReader::new(file);
        Self::from_reader(reader)
    }

    /// Parse metadata from a reader
    pub fn from_reader<R: BufRead>(reader: R) -> Result<Self, ScriptMetadataError> {
        let toml_content = extract_metadata_block(reader)?;
        let metadata: ScriptMetadata = toml_edit::de::from_str(&toml_content)?;
        Ok(metadata)
    }

    /// Get all dependencies for the current platform
    pub fn get_dependencies(
        &self,
        platform: Platform,
    ) -> Result<Vec<MatchSpec>, ScriptMetadataError> {
        let mut specs = Vec::new();

        // Add default dependencies
        for (name, version) in &self.dependencies.default {
            let spec_str = if version == "*" || version.is_empty() {
                name.clone()
            } else {
                format!("{}={}", name, version)
            };
            let spec =
                MatchSpec::from_str(&spec_str, rattler_conda_types::ParseStrictness::Lenient)
                    .map_err(|e| {
                        ScriptMetadataError::InvalidMatchSpec(spec_str.clone(), e.to_string())
                    })?;
            specs.push(spec);
        }

        // Add platform-specific dependencies
        if let Some(ref targets) = self.dependencies.target {
            for (platform_selector, deps) in targets {
                if Self::platform_matches(&platform_selector, platform) {
                    for (name, version) in deps {
                        let spec_str = if version == "*" || version.is_empty() {
                            name.clone()
                        } else {
                            format!("{}={}", name, version)
                        };
                        let spec = MatchSpec::from_str(
                            &spec_str,
                            rattler_conda_types::ParseStrictness::Lenient,
                        )
                        .map_err(|e| {
                            ScriptMetadataError::InvalidMatchSpec(spec_str.clone(), e.to_string())
                        })?;
                        specs.push(spec);
                    }
                }
            }
        }

        Ok(specs)
    }

    /// Get channels
    pub fn get_channels(&self) -> Result<Vec<NamedChannelOrUrl>, ScriptMetadataError> {
        self.script
            .channels
            .iter()
            .map(|s| {
                NamedChannelOrUrl::from_str(s)
                    .map_err(|e| ScriptMetadataError::InvalidChannel(s.clone(), e.to_string()))
            })
            .collect()
    }

    /// Get the entrypoint command for the current platform
    pub fn get_entrypoint(&self, platform: Platform) -> Option<String> {
        // Check platform-specific entrypoint first
        if let Some(ref targets) = self.script.target {
            for (platform_selector, config) in targets {
                if Self::platform_matches(platform_selector, platform) {
                    if let Some(ref entrypoint) = config.entrypoint {
                        return Some(entrypoint.clone());
                    }
                }
            }
        }

        // Fall back to default entrypoint
        self.script.entrypoint.clone()
    }

    /// Check if a platform selector matches the given platform
    fn platform_matches(selector: &str, platform: Platform) -> bool {
        match selector {
            "unix" => matches!(
                platform,
                Platform::Linux64
                    | Platform::LinuxAarch64
                    | Platform::LinuxPpc64le
                    | Platform::Osx64
                    | Platform::OsxArm64
            ),
            "linux" => matches!(
                platform,
                Platform::Linux64 | Platform::LinuxAarch64 | Platform::LinuxPpc64le
            ),
            "osx" => matches!(platform, Platform::Osx64 | Platform::OsxArm64),
            "win" => matches!(platform, Platform::Win64 | Platform::WinArm64),
            specific => {
                // Try to parse as a specific platform
                if let Ok(specific_platform) = Platform::from_str(specific) {
                    specific_platform == platform
                } else {
                    false
                }
            }
        }
    }
}

/// Extract the conda-script metadata block from a reader
fn extract_metadata_block<R: BufRead>(reader: R) -> Result<String, ScriptMetadataError> {
    let mut in_block = false;
    let mut toml_lines = Vec::new();
    let mut comment_prefix: Option<String> = None;

    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim_start();

        // Detect the start of the block
        if !in_block && trimmed.contains("/// conda-script") {
            in_block = true;
            // Detect comment syntax (e.g., "#", "//", "--")
            comment_prefix = detect_comment_prefix(&line);
            continue;
        }

        // Detect the end of the block
        if in_block && trimmed.contains("/// end-conda-script") {
            return Ok(toml_lines.join("\n"));
        }

        // Extract TOML content inside the block
        if in_block {
            if let Some(ref prefix) = comment_prefix {
                if let Some(content) = line.strip_prefix(prefix) {
                    // Remove the comment prefix and add to TOML content
                    toml_lines.push(content.trim_start().to_string());
                } else if trimmed.is_empty() {
                    // Allow empty lines
                    toml_lines.push(String::new());
                } else {
                    return Err(ScriptMetadataError::MalformedBlock(format!(
                        "Expected line to start with '{}', got: {}",
                        prefix, line
                    )));
                }
            }
        }
    }

    if in_block {
        return Err(ScriptMetadataError::MalformedBlock(
            "Metadata block started but never closed with '/// end-conda-script'".to_string(),
        ));
    }

    Err(ScriptMetadataError::NoMetadataFound)
}

/// Detect the comment prefix used in the script
fn detect_comment_prefix(line: &str) -> Option<String> {
    // Common comment prefixes
    let prefixes = ["# ", "// ", "-- ", "/* "];

    for prefix in &prefixes {
        if line.trim_start().starts_with(prefix) {
            return Some(prefix.to_string());
        }
    }

    // Also support without trailing space
    let prefixes_no_space = ["#", "//", "--"];
    for prefix in &prefixes_no_space {
        if line.trim_start().starts_with(prefix) {
            return Some(prefix.to_string());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_parse_python_metadata() {
        let script = r#"#!/usr/bin/env python
# Some header comment
# /// conda-script
# [dependencies]
# python = "3.12.*"
# requests = "*"
# [script]
# channels = ["conda-forge"]
# entrypoint = "python"
# /// end-conda-script

import requests
print("Hello!")
"#;

        let cursor = Cursor::new(script);
        let metadata = ScriptMetadata::from_reader(cursor).unwrap();

        assert_eq!(metadata.dependencies.default.len(), 2);
        assert_eq!(
            metadata.dependencies.default.get("python").unwrap(),
            "3.12.*"
        );
        assert_eq!(metadata.dependencies.default.get("requests").unwrap(), "*");
        assert_eq!(metadata.script.channels, vec!["conda-forge"]);
        assert_eq!(metadata.script.entrypoint, Some("python".to_string()));
    }

    #[test]
    fn test_parse_rust_metadata() {
        let script = r#"// Rust script
// /// conda-script
// [dependencies]
// gcc = "*"
// [script]
// channels = ["conda-forge"]
// entrypoint = "cargo run"
// /// end-conda-script

fn main() {
    println!("Hello!");
}
"#;

        let cursor = Cursor::new(script);
        let metadata = ScriptMetadata::from_reader(cursor).unwrap();

        assert_eq!(metadata.dependencies.default.len(), 1);
        assert_eq!(metadata.dependencies.default.get("gcc").unwrap(), "*");
    }

    #[test]
    fn test_parse_platform_specific_deps() {
        let script = r#"# /// conda-script
# [dependencies]
# python = "3.12.*"
# [dependencies.target.unix]
# gcc = "*"
# [dependencies.target.win]
# msvc = "*"
# [script]
# channels = ["conda-forge"]
# /// end-conda-script
"#;

        let cursor = Cursor::new(script);
        let metadata = ScriptMetadata::from_reader(cursor).unwrap();

        // Test Unix platform
        let deps_linux = metadata.get_dependencies(Platform::Linux64).unwrap();
        assert_eq!(deps_linux.len(), 2); // python + gcc

        // Test Windows platform
        let deps_win = metadata.get_dependencies(Platform::Win64).unwrap();
        assert_eq!(deps_win.len(), 2); // python + msvc
    }

    #[test]
    fn test_platform_specific_entrypoint() {
        let script = r#"# /// conda-script
# [dependencies]
# gcc = "*"
# [script]
# channels = ["conda-forge"]
# entrypoint = "bash default.sh"
# [script.target.win]
# entrypoint = "cmd.exe /c default.bat"
# /// end-conda-script
"#;

        let cursor = Cursor::new(script);
        let metadata = ScriptMetadata::from_reader(cursor).unwrap();

        let entrypoint_linux = metadata.get_entrypoint(Platform::Linux64);
        assert_eq!(entrypoint_linux, Some("bash default.sh".to_string()));

        let entrypoint_win = metadata.get_entrypoint(Platform::Win64);
        assert_eq!(entrypoint_win, Some("cmd.exe /c default.bat".to_string()));
    }

    #[test]
    fn test_no_metadata() {
        let script = r#"#!/usr/bin/env python
print("No metadata here!")
"#;

        let cursor = Cursor::new(script);
        let result = ScriptMetadata::from_reader(cursor);
        assert!(matches!(result, Err(ScriptMetadataError::NoMetadataFound)));
    }

    #[test]
    fn test_unclosed_block() {
        let script = r#"# /// conda-script
# [dependencies]
# python = "3.12.*"
"#;

        let cursor = Cursor::new(script);
        let result = ScriptMetadata::from_reader(cursor);
        assert!(matches!(
            result,
            Err(ScriptMetadataError::MalformedBlock(_))
        ));
    }
}
