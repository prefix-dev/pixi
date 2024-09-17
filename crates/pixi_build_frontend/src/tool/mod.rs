mod cache;
mod spec;

use std::path::{Path, PathBuf};

pub use cache::ToolCache;
pub use spec::{IsolatedToolSpec, SystemToolSpec, ToolSpec};

/// A tool that can be invoked.
#[derive(Debug, Clone)]
pub enum Tool {
    Isolated(IsolatedTool),
    System(SystemTool),
}

/// A tool that is pre-installed on the system.
#[derive(Debug, Clone)]
pub struct SystemTool {
    command: PathBuf,
}

impl SystemTool {
    /// Construct a new instance from a command.
    pub(crate) fn new(command: impl Into<PathBuf>) -> Self {
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

/// A tool that is installed in its own isolated environment.
#[derive(Debug, Clone)]
pub struct IsolatedTool {
    command: PathBuf,
    prefix: PathBuf,
}

impl IsolatedTool {
    /// Construct a new instance from a command and prefix.
    pub(crate) fn new(command: impl Into<PathBuf>, prefix: impl Into<PathBuf>) -> Self {
        Self {
            command: command.into(),
            prefix: prefix.into(),
        }
    }
}

impl From<IsolatedTool> for Tool {
    fn from(value: IsolatedTool) -> Self {
        Self::Isolated(value)
    }
}

impl Tool {
    /// Returns the full path to the executable to invoke.
    pub fn executable(&self) -> &Path {
        match self {
            Tool::Isolated(tool) => &tool.command,
            Tool::System(tool) => &tool.command,
        }
    }

    /// Construct a new tool that calls another executable.
    pub fn with_executable(&self, executable: impl Into<PathBuf>) -> Self {
        match self {
            Tool::Isolated(tool) => {
                Tool::Isolated(IsolatedTool::new(executable, tool.prefix.clone()))
            }
            Tool::System(_) => Tool::System(SystemTool::new(executable)),
        }
    }

    /// Construct a new command that enables invocation of the tool.
    pub fn command(&self) -> std::process::Command {
        match self {
            Tool::Isolated(_tool) => {
                todo!("invocation of isolated tools is not implemented yet");
            }
            Tool::System(tool) => std::process::Command::new(&tool.command),
        }
    }
}
