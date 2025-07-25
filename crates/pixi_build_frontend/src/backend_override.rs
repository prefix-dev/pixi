use std::{path::PathBuf, str::FromStr};

use pixi_build_discovery::{CommandSpec, SystemCommandSpec};

use crate::in_memory::{BoxedInMemoryBackend, InMemoryBackendInstantiator};

/// A backend override that can be used to override the backend tools.
#[derive(Debug)]
pub enum BackendOverride {
    /// Overwrite the backend with an executable path.
    System(OverriddenBackends),

    /// Use an in-memory backend instantiator to create the backend.
    InMemory(BoxedInMemoryBackend),
}

/// Default implementation for the backend override were no tools are
/// overridden.
impl Default for BackendOverride {
    fn default() -> Self {
        Self::System(OverriddenBackends::Specified(Vec::new()))
    }
}

impl BackendOverride {
    /// Constructs a backend override that uses the specified
    /// [`InMemoryBackendInstantiator`] to create an in-memory backend.
    ///
    /// Using this method allows you to create a backend that runs completely in
    /// memory.
    pub fn from_memory<T: InMemoryBackendInstantiator + Send + Sync + 'static>(
        instantiator: T,
    ) -> Self {
        Self::InMemory(BoxedInMemoryBackend::from(instantiator))
    }
}

impl OverriddenBackends {
    /// Returns a new backend spec for a backend with the given name.
    pub fn named_backend_override(&self, name: &str) -> Option<CommandSpec> {
        let tool = match self {
            OverriddenBackends::Specified(overridden) => {
                overridden.iter().find(|tool| tool.name == name)?
            }
            OverriddenBackends::All => &OverriddenTool {
                name: name.to_string(),
                path: None,
            },
        };

        Some(CommandSpec::System(SystemCommandSpec {
            command: Some(
                tool.path
                    .as_ref()
                    .and_then(|p| p.to_str())
                    .unwrap_or(tool.name.as_str())
                    .to_string(),
            ),
        }))
    }
}

/// The tool that is being overridden.
#[derive(Debug, Clone)]
pub struct OverriddenTool {
    /// Name of the tool should be mostly equal to the spec that is being
    /// overridden
    name: String,
    /// Optional path to the executable that should be used. if this is not set
    /// it is assumed that the tool is available in the root.
    path: Option<PathBuf>,
}

/// List of overridden backends
#[derive(Debug)]
pub enum OverriddenBackends {
    /// Overrides all backends and assume they are available in the root.
    All,
    /// Specific backend overrides.
    Specified(Vec<OverriddenTool>),
}

#[derive(Debug, thiserror::Error, miette::Diagnostic)]
#[error("Failed to parse OverriddenBackends")]
pub struct ParseError;

const EQUALS: &str = "=";
const SEPARATOR: &str = ",";

impl FromStr for OverriddenBackends {
    type Err = ParseError;
    // This can be in the form of either:
    // 1. pixi-build-python=/some/path/to/custom-build
    // 2. pixi-build-python (just the name of the tool)
    // The separation is done with a ',' between different tools.
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
    /// If `PIXI_BUILD_BACKEND_OVERRIDE_ALL` is set it will override all
    /// backends. If not it will check the `PIXI_BUILD_BACKEND_OVERRIDE`
    /// environment variable.
    ///
    /// This variable should be in the form of
    /// `tool_name=/path/to/executable,tool_name2`. Where the `,` is used
    /// to separate different tools. and the `=` is used to separate the
    /// tool name from the path. If no path is provided the tool is assumed to
    /// be available in the root.
    pub fn from_env() -> miette::Result<Option<Self>> {
        let backend_override = match std::env::var("PIXI_BUILD_BACKEND_OVERRIDE_ALL") {
            Ok(_) => {
                tracing::warn!("overriding build backend with system prefixed tools");
                Some(Self::System(OverriddenBackends::All))
            }
            Err(_) => match std::env::var("PIXI_BUILD_BACKEND_OVERRIDE") {
                Ok(spec) => {
                    tracing::warn!("overriding build backend with: {}", spec);
                    Some(Self::System(OverriddenBackends::from_str(&spec)?))
                }
                Err(_) => None,
            },
        };

        Ok(backend_override)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

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
