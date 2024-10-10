use std::path::PathBuf;

use dashmap::{DashMap, Entry};

use crate::tool::{SystemTool, Tool, ToolSpec};

use super::IsolatedTool;

/// A [`ToolCache`] maintains a cache of environments for build tools.
///
/// This is useful to ensure that if we need to build multiple packages that use
/// the same tool, we can reuse their environments.
pub struct ToolCache {
    cache: DashMap<ToolSpec, Tool>,
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
    pub fn instantiate(&self, spec: &ToolSpec) -> which::Result<Tool> {
        let cache_entry = match self.cache.entry(spec.clone()) {
            Entry::Occupied(entry) => return Ok(entry.get().clone()),
            Entry::Vacant(entry) => entry,
        };

        let tool: Tool = match spec {
            ToolSpec::Isolated(spec) => {
                // Don't isolate yet we are just pretending
                // TODO: add isolation
                let found = which::which(&spec.command)?;
                IsolatedTool::new(found, PathBuf::new()).into()
            }
            ToolSpec::System(spec) => {
                let exec = if spec.command.is_absolute() {
                    spec.command.clone()
                } else {
                    which::which(&spec.command)?
                };
                SystemTool::new(exec).into()
            }
        };

        cache_entry.insert(tool.clone());
        Ok(tool)
    }
}
