use dashmap::{DashMap, Entry};
use itertools::Itertools;
use miette::{Context, IntoDiagnostic};

use crate::tool::{SystemTool, Tool, ToolSpec};

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
    pub fn instantiate(&self, spec: &ToolSpec) -> miette::Result<Tool> {
        let cache_entry = match self.cache.entry(spec.clone()) {
            Entry::Occupied(entry) => return Ok(entry.get().clone()),
            Entry::Vacant(entry) => entry,
        };

        let tool: Tool = match spec {
            ToolSpec::Isolated(spec) => {
                todo!(
                    "requested to instantiate {} but isolated tools are not implemented yet",
                    spec.specs.iter().map(|s| s.to_string()).format(", ")
                )
            }
            ToolSpec::System(spec) => {
                let exec = if spec.command.is_absolute() {
                    spec.command.clone()
                } else {
                    which::which(&spec.command)
                        .into_diagnostic()
                        .with_context(|| format!("failed to find '{}'", spec.command.display()))?
                };
                SystemTool::new(exec).into()
            }
        };

        cache_entry.insert(tool.clone());
        Ok(tool)
    }
}
