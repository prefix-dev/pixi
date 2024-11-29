mod cache;
mod installer;
mod spec;

use std::{collections::HashMap, path::PathBuf, sync::Arc};

pub use cache::ToolCacheError;
pub use spec::{IsolatedToolSpec, SystemToolSpec, ToolSpec};

use crate::InProcessBackend;

pub use installer::ToolContext;

/// A tool that can be invoked.
#[derive(Debug)]
pub enum Tool {
    Isolated(Arc<IsolatedTool>),
    System(SystemTool),
    Io(InProcessBackend),
}

impl Tool {
    pub fn as_isolated(&self) -> Option<Arc<IsolatedTool>> {
        match self {
            Tool::Isolated(tool) => Some(tool.clone()),
            Tool::System(_) => None,
            Tool::Io(_) => None,
        }
    }
}

#[derive(Debug)]
pub enum ExecutableTool {
    Isolated(Arc<IsolatedTool>),
    System(SystemTool),
}

/// A tool that is pre-installed on the system.
#[derive(Debug, Clone)]
pub struct SystemTool {
    command: String,
}

impl SystemTool {
    /// Construct a new instance from a command.
    pub(crate) fn new(command: impl Into<String>) -> Self {
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

impl From<Arc<IsolatedTool>> for Tool {
    fn from(value: Arc<IsolatedTool>) -> Self {
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

// impl From<IsolatedTool> for Tool {
//     fn from(value: IsolatedTool) -> Self {
//         Self::Isolated(value)
//     }
// }

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
    pub fn executable(&self) -> &String {
        match self {
            ExecutableTool::Isolated(tool) => &tool.command,
            ExecutableTool::System(tool) => &tool.command,
        }
    }

    /// Construct a new tool that calls another executable.
    pub fn with_executable(&self, executable: impl Into<String>) -> Self {
        match self {
            ExecutableTool::Isolated(tool) => {
                ExecutableTool::Isolated(Arc::new(IsolatedTool::new(
                    executable,
                    tool.prefix.clone(),
                    tool.activation_scripts.clone(),
                )))
            }
            ExecutableTool::System(_) => ExecutableTool::System(SystemTool::new(executable)),
        }
    }

    /// Construct a new command that enables invocation of the tool.
    pub fn command(&self) -> std::process::Command {
        match self {
            ExecutableTool::Isolated(tool) => {
                let mut cmd = std::process::Command::new(&tool.command);
                cmd.envs(tool.activation_scripts.clone());

                cmd
            }
            ExecutableTool::System(tool) => std::process::Command::new(&tool.command),
        }
    }
}
