use std::{path::PathBuf, str::FromStr};

use crate::{SystemToolSpec, ToolSpec};

/// A backend override that can be used to override the backend tools.
#[derive(Debug)]
pub enum BackendOverride {
    /// Overwrite the backend with an executable path.
    System(OverriddenBackends),
    // Add more overrides here once require
    // e.g like an isolated spec
}

impl BackendOverride {
    /// Retrieve potential overridden tool for the given name.
    pub(crate) fn overridden_tool(&self, name: &str) -> Option<OverriddenTool> {
        match self {
            Self::System(overridden) => match overridden {
                OverriddenBackends::Specified(overridden) => {
                    overridden.iter().find(|tool| tool.name == name).cloned()
                }
                OverriddenBackends::All => Some(OverriddenTool {
                    name: name.to_string(),
                    path: None,
                }),
            },
        }
    }
}

/// The tool that is being overridden.
#[derive(Debug, Clone)]
pub struct OverriddenTool {
    /// Name of the tool should be mostly equal to the spec that is being overridden
    name: String,
    /// Optional path to the executable that should be used. if this is not set it is assumed
    /// that the tool is available in the root.
    path: Option<PathBuf>,
}

impl OverriddenTool {
    /// Convert the overridden tool into a `ToolSpec`.
    pub(crate) fn as_spec(&self) -> ToolSpec {
        // Take the path if it is set otherwise use the name as the command.
        let command = self
            .path
            .clone()
            .unwrap_or_else(|| PathBuf::from(&self.name));
        ToolSpec::System(SystemToolSpec {
            command: command.as_os_str().to_string_lossy().into_owned(),
        })
    }
}

/// List of overridden backends
#[derive(Debug)]
pub enum OverriddenBackends {
    /// Overrides all backends and assume they are available in the root.
    All,
    /// Specific backend overrides.
    Specified(Vec<OverriddenTool>),
}

#[derive(Debug)]
pub struct ParseError;
impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Failed to parse OverriddenBackends")
    }
}

const EQUALS: &str = "=";
const SEPARATOR: &str = ",";

impl std::error::Error for ParseError {}
impl FromStr for OverriddenBackends {
    type Err = ParseError;
    // This can be in the form of either:
    // 1. pixi-build-python=/some/path/to/custom-build
    // 2. pixi-build-python (just the name of the tool)
    // The separation is done with a '::' between different tools.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut tools = Vec::new();
        if s.is_empty() {
            return Ok(OverriddenBackends::Specified(tools));
        }
        for tool_str in s.split(SEPARATOR) {
            let parts: Vec<&str> = tool_str.split(EQUALS).collect();
            let tool = match parts.as_slice() {
                [name] => OverriddenTool {
                    name: name.to_string(),
                    path: None,
                },
                [name, path] => OverriddenTool {
                    name: name.to_string(),
                    path: Some(PathBuf::from(path)),
                },
                _ => return Err(ParseError),
            };
            tools.push(tool);
        }

        Ok(OverriddenBackends::Specified(tools))
    }
}

impl BackendOverride {
    /// Retrieve the backend override from the environment.
    /// If `PIXI_BUILD_BACKEND_OVERRIDE_ALL` is set it will override all backends.
    /// If not it will check the `PIXI_BUILD_BACKEND_OVERRIDE` environment variable.
    ///
    /// This variable should be in the form of `tool_name=/path/to/executable::tool_name2`.
    /// Where the `::` is used to separate different tools. and the `=` is used to separate the
    /// tool name from the path. If no path is provided the tool is assumed to be available in the
    /// root.
    pub fn from_env() -> Option<Self> {
        match std::env::var("PIXI_BUILD_BACKEND_OVERRIDE_ALL") {
            Ok(_) => {
                tracing::warn!("overriding build backend with system prefixed tools");
                Some(Self::System(OverriddenBackends::All))
            }
            Err(_) => match std::env::var("PIXI_BUILD_BACKEND_OVERRIDE") {
                Ok(spec) => {
                    tracing::warn!("overriding build backend with: {}", spec);
                    Some(Self::System(OverriddenBackends::from_str(&spec).unwrap()))
                }
                Err(_) => None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_single_tool_with_path() {
        let input = format!("pixi-build-python{EQUALS}/some/path/to/custom-build");
        let parsed = OverriddenBackends::from_str(input.as_str()).unwrap();

        if let OverriddenBackends::Specified(tools) = parsed {
            assert_eq!(tools.len(), 1);
            assert_eq!(tools[0].name, "pixi-build-python");
            assert_eq!(
                tools[0].path,
                Some(PathBuf::from("/some/path/to/custom-build"))
            );
        } else {
            panic!("Expected OverriddenBackends::Specified");
        }
    }

    #[test]
    fn test_single_tool_without_path() {
        let input = "pixi-build-python";
        let parsed = OverriddenBackends::from_str(input).unwrap();

        if let OverriddenBackends::Specified(tools) = parsed {
            assert_eq!(tools.len(), 1);
            assert_eq!(tools[0].name, "pixi-build-python");
            assert!(tools[0].path.is_none());
        } else {
            panic!("Expected OverriddenBackends::Specified");
        }
    }

    #[test]
    fn test_multiple_tools_mixed() {
        let input =
            format!("pixi-build-python{EQUALS}/some/path/to/custom-build{SEPARATOR}pixi-test-tool");
        let parsed = OverriddenBackends::from_str(input.as_str()).unwrap();

        if let OverriddenBackends::Specified(tools) = parsed {
            assert_eq!(tools.len(), 2);

            assert_eq!(tools[0].name, "pixi-build-python");
            assert_eq!(
                tools[0].path,
                Some(PathBuf::from("/some/path/to/custom-build"))
            );

            assert_eq!(tools[1].name, "pixi-test-tool");
            assert!(tools[1].path.is_none());
        } else {
            panic!("Expected OverriddenBackends::Specified");
        }
    }

    #[test]
    fn test_empty_input() {
        let input = "";
        let parsed = OverriddenBackends::from_str(input).unwrap();

        if let OverriddenBackends::Specified(tools) = parsed {
            assert!(tools.is_empty());
        } else {
            panic!("Expected OverriddenBackends::Specified");
        }
    }

    #[test]
    fn test_invalid_format() {
        let input = format!(
            "pixi-build-python{EQUALS}/some/path/to/custom-build{SEPARATOR}invalid{EQUALS}tool{EQUALS}extra"
        );
        let parsed = OverriddenBackends::from_str(input.as_str());
        assert!(parsed.is_err());
    }
}
