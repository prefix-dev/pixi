use rattler_conda_types::VersionWithSource;
use std::{collections::HashMap, path::PathBuf};

/// Verbosity flags to pass to a build backend process.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct BackendVerbosity {
    quiet: u8,
    verbose: u8,
}

impl BackendVerbosity {
    /// Construct backend verbosity from Pixi's CLI flag counts.
    pub fn from_cli(quiet: u8, verbose: u8) -> Self {
        Self { quiet, verbose }
    }

    fn args(self) -> Vec<&'static str> {
        if self.quiet > 0 {
            return vec!["-q"; 3];
        }

        match self.verbose {
            // Preserve the backend's environment/default when Pixi received no
            // explicit verbosity option.
            0 => Vec::new(),
            1 => Vec::new(),
            2 => vec!["-v"],
            _ => vec!["-v"; 2],
        }
    }
}

/// A tool that can be invoked.
#[derive(Debug)]
pub enum Tool {
    Isolated(Box<IsolatedTool>),
    System(SystemTool),
}

/// A tool that is pre-installed on the system.
#[derive(Debug, Clone)]
pub struct SystemTool {
    command: String,
}

impl SystemTool {
    /// Construct a new instance from a command.
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
        }
    }
}

impl From<SystemTool> for Tool {
    fn from(value: SystemTool) -> Self {
        Self::System(value)
    }
}

impl From<IsolatedTool> for Tool {
    fn from(value: IsolatedTool) -> Self {
        Self::Isolated(Box::new(value))
    }
}

/// A tool that is installed in its own isolated environment.
#[derive(Debug, Clone)]
pub struct IsolatedTool {
    /// The command to invoke.
    command: String,
    /// The version of the tool that was installed.
    version: Option<VersionWithSource>,
    /// The prefix to use for the isolated environment.
    prefix: PathBuf,
    /// Activation scripts
    activation_scripts: HashMap<String, String>,
}

impl IsolatedTool {
    /// Construct a new instance from a command and prefix.
    pub fn new(
        command: impl Into<String>,
        version: Option<VersionWithSource>,
        prefix: impl Into<PathBuf>,
        activation: HashMap<String, String>,
    ) -> Self {
        Self {
            command: command.into(),
            version,
            prefix: prefix.into(),
            activation_scripts: activation,
        }
    }

    /// Get the prefix of the isolated tool.
    pub fn prefix(&self) -> &PathBuf {
        &self.prefix
    }
}

impl Tool {
    pub fn as_isolated(&self) -> Option<&IsolatedTool> {
        match self {
            Tool::Isolated(tool) => Some(tool),
            Tool::System(_) => None,
        }
    }

    /// Returns the full path to the executable to invoke.
    pub fn executable(&self) -> &String {
        match self {
            Tool::Isolated(tool) => &tool.command,
            Tool::System(tool) => &tool.command,
        }
    }

    /// Returns the version of the tool, if available.
    pub fn version(&self) -> Option<&VersionWithSource> {
        match self {
            Tool::Isolated(tool) => tool.version.as_ref(),
            Tool::System(_) => None,
        }
    }

    /// Construct a new tool that calls another executable.
    pub fn with_executable(&self, executable: impl Into<String>) -> Self {
        match self {
            Tool::Isolated(tool) => Tool::Isolated(Box::new(IsolatedTool::new(
                executable,
                tool.version.clone(),
                tool.prefix.clone(),
                tool.activation_scripts.clone(),
            ))),
            Tool::System(_) => Tool::System(SystemTool::new(executable)),
        }
    }

    /// Construct a new command that enables invocation of the tool.
    /// TODO: whether to inject proxy config
    pub fn command(&self) -> std::process::Command {
        self.command_with_verbosity(BackendVerbosity::default())
    }

    /// Construct a new command with explicit backend verbosity.
    pub fn command_with_verbosity(&self, verbosity: BackendVerbosity) -> std::process::Command {
        let mut command = match self {
            Tool::Isolated(tool) => {
                let mut cmd = std::process::Command::new(&tool.command);
                cmd.envs(tool.activation_scripts.clone());

                cmd
            }
            Tool::System(tool) => std::process::Command::new(&tool.command),
        };

        command.args(verbosity.args());
        command
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, ffi::OsStr};

    use super::{BackendVerbosity, IsolatedTool, SystemTool, Tool};

    #[test]
    fn backend_verbosity_matches_pixi_cli_levels() {
        assert!(BackendVerbosity::from_cli(0, 0).args().is_empty());
        assert!(BackendVerbosity::from_cli(0, 1).args().is_empty());
        assert_eq!(BackendVerbosity::from_cli(0, 2).args(), ["-v"]);
        assert_eq!(BackendVerbosity::from_cli(0, 3).args(), ["-v", "-v"]);
        assert_eq!(BackendVerbosity::from_cli(0, 4).args(), ["-v", "-v"]);
        assert_eq!(BackendVerbosity::from_cli(1, 4).args(), ["-q", "-q", "-q"]);
        assert_eq!(BackendVerbosity::from_cli(4, 1).args(), ["-q", "-q", "-q"]);
    }

    #[test]
    fn tool_commands_include_verbosity() {
        let tool = Tool::from(SystemTool::new("backend"));
        assert_eq!(
            tool.command_with_verbosity(BackendVerbosity::from_cli(0, 3))
                .get_args()
                .collect::<Vec<_>>(),
            [OsStr::new("-v"), OsStr::new("-v")]
        );

        let tool = Tool::from(IsolatedTool::new(
            "backend",
            None,
            "/prefix",
            HashMap::new(),
        ));
        assert_eq!(
            tool.command_with_verbosity(BackendVerbosity::from_cli(1, 4))
                .get_args()
                .collect::<Vec<_>>(),
            [OsStr::new("-q"), OsStr::new("-q"), OsStr::new("-q")]
        );

        assert!(
            Tool::from(SystemTool::new("backend"))
                .command()
                .get_args()
                .next()
                .is_none()
        );
    }
}
