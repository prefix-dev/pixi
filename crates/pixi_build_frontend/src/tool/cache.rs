use std::path::PathBuf;

use dashmap::{DashMap, Entry};

use super::IsolatedTool;
use crate::{
    tool::{SystemTool, Tool, ToolSpec},
    IsolatedToolSpec, SystemToolSpec,
};

/// A [`ToolCache`] maintains a cache of environments for build tools.
///
/// This is useful to ensure that if we need to build multiple packages that use
/// the same tool, we can reuse their environments.
pub struct ToolCache {
    cache: DashMap<CacheableToolSpec, CachedTool>,
}

#[derive(thiserror::Error, Debug)]
pub enum ToolCacheError {
    #[error("could not resolve '{}', {1}", .0.display())]
    Instantiate(PathBuf, which::Error),
}

/// Describes the specification of the tool. This can be used to cache tool
/// information.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
enum CacheableToolSpec {
    Isolated(IsolatedToolSpec),
    System(SystemToolSpec),
}

/// A tool that can be invoked.
#[derive(Debug, Clone)]
enum CachedTool {
    Isolated(IsolatedTool),
    System(SystemTool),
}

impl From<CachedTool> for Tool {
    fn from(value: CachedTool) -> Self {
        match value {
            CachedTool::Isolated(tool) => Tool::Isolated(tool),
            CachedTool::System(tool) => Tool::System(tool),
        }
    }
}

impl From<IsolatedTool> for CachedTool {
    fn from(value: IsolatedTool) -> Self {
        Self::Isolated(value)
    }
}

impl From<SystemTool> for CachedTool {
    fn from(value: SystemTool) -> Self {
        Self::System(value)
    }
}

impl ToolCache {
    /// Construct a new tool cache.
    pub fn new() -> Self {
        Self {
            cache: DashMap::default(),
        }
    }

    /// Instantiate a tool from a specification.
    ///
    /// If the tool is not already cached, it will be created and cached.
    pub fn instantiate(&self, spec: ToolSpec) -> Result<Tool, ToolCacheError> {
        let spec = match spec {
            ToolSpec::Io(ipc) => return Ok(Tool::Io(ipc)),
            ToolSpec::Isolated(isolated) => CacheableToolSpec::Isolated(isolated),
            ToolSpec::System(system) => CacheableToolSpec::System(system),
        };

        let cache_entry = match self.cache.entry(spec.clone()) {
            Entry::Occupied(entry) => return Ok(entry.get().clone().into()),
            Entry::Vacant(entry) => entry,
        };

        let tool: CachedTool = match spec {
            CacheableToolSpec::Isolated(spec) => {
                // Don't isolate yet we are just pretending
                // TODO: add isolation
                let found = which::which(&spec.command)
                    .map_err(|e| ToolCacheError::Instantiate(spec.command.clone().into(), e))?;
                IsolatedTool::new(found, PathBuf::new()).into()
            }
            CacheableToolSpec::System(spec) => {
                let exec = if spec.command.is_absolute() {
                    spec.command.clone()
                } else {
                    which::which(&spec.command)
                        .map_err(|e| ToolCacheError::Instantiate(spec.command.clone(), e))?
                };
                SystemTool::new(exec).into()
            }
        };

        cache_entry.insert(tool.clone());
        Ok(tool.into())
    }
}
