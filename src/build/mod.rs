use std::collections::{BTreeMap, HashMap};

use miette::IntoDiagnostic;
use pixi_command_dispatcher::CommandDispatcher;
pub use pixi_glob::{GlobHashCache, GlobHashError};
use pixi_manifest::Targets;
use rattler_conda_types::{ChannelConfig, Platform};

use crate::Workspace;

/// The [`BuildContext`] is used to build packages from source.
#[derive(Clone)]
pub struct BuildContext {
    channel_config: ChannelConfig,
    variant_config: Targets<Option<HashMap<String, Vec<String>>>>,
    command_dispatcher: CommandDispatcher,
}

impl BuildContext {
    pub fn new(
        channel_config: ChannelConfig,
        variant_config: Targets<Option<HashMap<String, Vec<String>>>>,
        command_dispatcher: CommandDispatcher,
    ) -> Result<Self, std::io::Error> {
        Ok(Self {
            channel_config,
            variant_config,
            command_dispatcher,
        })
    }

    pub fn from_workspace(
        workspace: &Workspace,
        command_dispatcher: CommandDispatcher,
    ) -> miette::Result<Self> {
        let variant = workspace.workspace.value.workspace.build_variants.clone();
        Self::new(workspace.channel_config(), variant, command_dispatcher).into_diagnostic()
    }

    pub fn command_dispatcher(&self) -> &CommandDispatcher {
        &self.command_dispatcher
    }

    pub fn channel_config(&self) -> &ChannelConfig {
        &self.channel_config
    }

    pub fn resolve_variant(&self, platform: Platform) -> BTreeMap<String, Vec<String>> {
        let mut result = BTreeMap::new();

        // Resolves from most specific to least specific.
        for variants in self.variant_config.resolve(Some(platform)).flatten() {
            // Update the hash map, but only items that are not already in the map.
            for (key, value) in variants {
                result.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }

        tracing::trace!("resolved variant configuration: {:?}", result);

        result
    }
}
