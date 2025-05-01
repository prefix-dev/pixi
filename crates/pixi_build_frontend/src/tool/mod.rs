mod cache;
mod installer;
mod spec;

use std::{collections::HashMap, path::PathBuf};

pub use cache::ToolCacheError;
pub use spec::{IsolatedToolSpec, SystemToolSpec, ToolSpec};

pub use installer::ToolContext;

/// A tool that can be invoked.
#[derive(Debug)]
pub enum Tool {
    Isolated(IsolatedTool),
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
        Self::Isolated(value)
    }
}

/// A tool that is installed in its own isolated environment.
#[derive(Debug, Clone)]
pub struct IsolatedTool {
    /// The command to invoke.
    command: String,
    /// The prefix to use for the isolated environment.
    prefix: PathBuf,
    /// Activation scripts
    activation_scripts: HashMap<String, String>,
}

impl IsolatedTool {
    /// Construct a new instance from a command and prefix.
    pub(crate) fn new(
        command: impl Into<String>,
        prefix: impl Into<PathBuf>,
        activation: HashMap<String, String>,
    ) -> Self {
        Self {
            command: command.into(),
            prefix: prefix.into(),
            activation_scripts: activation,
        }
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

    /// Construct a new tool that calls another executable.
    pub fn with_executable(&self, executable: impl Into<String>) -> Self {
        match self {
            Tool::Isolated(tool) => Tool::Isolated(IsolatedTool::new(
                executable,
                tool.prefix.clone(),
                tool.activation_scripts.clone(),
            )),
            Tool::System(_) => Tool::System(SystemTool::new(executable)),
        }
    }

    /// Construct a new command that enables invocation of the tool.
    /// TODO: whether to inject proxy config
    pub fn command(&self) -> std::process::Command {
        match self {
            Tool::Isolated(tool) => {
                let mut cmd = std::process::Command::new(&tool.command);
                cmd.envs(tool.activation_scripts.clone());

                cmd
            }
            Tool::System(tool) => std::process::Command::new(&tool.command),
        }
    }
}
