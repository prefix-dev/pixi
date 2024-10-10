mod cache;
mod spec;

use std::path::{Path, PathBuf};

pub use cache::{ToolCache, ToolCacheError};
pub use spec::{IsolatedToolSpec, SystemToolSpec, ToolSpec};

use crate::InProcessBackend;

/// A tool that can be invoked.
#[derive(Debug)]
pub enum Tool {
    Isolated(IsolatedTool),
    System(SystemTool),
    Io(InProcessBackend),
}

#[derive(Debug)]
pub enum ExecutableTool {
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
    pub fn as_executable(&self) -> Option<ExecutableTool> {
        match self {
            Tool::Isolated(tool) => Some(ExecutableTool::Isolated(tool.clone())),
            Tool::System(tool) => Some(ExecutableTool::System(tool.clone())),
            Tool::Io(_) => None,
        }
    }

    pub fn try_into_executable(self) -> Result<ExecutableTool, InProcessBackend> {
        match self {
            Tool::Isolated(tool) => Ok(ExecutableTool::Isolated(tool)),
            Tool::System(tool) => Ok(ExecutableTool::System(tool)),
            Tool::Io(ipc) => Err(ipc),
        }
    }
}

impl ExecutableTool {
    /// Returns the full path to the executable to invoke.
    pub fn executable(&self) -> &Path {
        match self {
            ExecutableTool::Isolated(tool) => &tool.command,
            ExecutableTool::System(tool) => &tool.command,
        }
    }

    /// Construct a new tool that calls another executable.
    pub fn with_executable(&self, executable: impl Into<PathBuf>) -> Self {
        match self {
            ExecutableTool::Isolated(tool) => {
                ExecutableTool::Isolated(IsolatedTool::new(executable, tool.prefix.clone()))
            }
            ExecutableTool::System(_) => ExecutableTool::System(SystemTool::new(executable)),
        }
    }

    /// Construct a new command that enables invocation of the tool.
    pub fn command(&self) -> std::process::Command {
        match self {
            ExecutableTool::Isolated(tool) => std::process::Command::new(&tool.command),
            ExecutableTool::System(tool) => std::process::Command::new(&tool.command),
        }
    }
}
