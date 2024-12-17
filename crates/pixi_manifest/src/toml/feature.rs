use std::collections::HashMap;

use crate::toml::platform::TomlPlatform;
use crate::{
    pypi::{pypi_options::PypiOptions, PyPiPackageName},
    toml::{TomlPrioritizedChannel, TomlTarget},
    utils::{package_map::UniquePackageMap, PixiSpanned},
    workspace::ChannelPriority,
    Activation, Feature, FeatureName, Preview, PyPiRequirement, SystemRequirements, TargetSelector,
    Targets, Task, TaskName, TomlError,
};
use indexmap::{IndexMap, IndexSet};
use pixi_toml::{TomlHashMap, TomlIndexMap, TomlIndexSet, TomlWith};
use rattler_conda_types::Platform;
use toml_span::de_helpers::TableHelper;
use toml_span::{DeserError, Value};

#[derive(Debug)]
pub struct TomlFeature {
    pub platforms: Option<PixiSpanned<IndexSet<Platform>>>,
    pub channels: Option<Vec<TomlPrioritizedChannel>>,
    pub channel_priority: Option<ChannelPriority>,
    pub system_requirements: SystemRequirements,
    pub target: IndexMap<PixiSpanned<TargetSelector>, TomlTarget>,
    pub dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub host_dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub build_dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub pypi_dependencies: Option<IndexMap<PyPiPackageName, PyPiRequirement>>,

    /// Additional information to activate an environment.
    pub activation: Option<Activation>,

    /// Target specific tasks to run in the environment
    pub tasks: HashMap<TaskName, Task>,

    /// Additional options for PyPi dependencies.
    pub pypi_options: Option<PypiOptions>,
}

impl TomlFeature {
    pub fn into_feature(self, name: FeatureName, preview: &Preview) -> Result<Feature, TomlError> {
        let default_target = TomlTarget {
            dependencies: self.dependencies,
            host_dependencies: self.host_dependencies,
            build_dependencies: self.build_dependencies,
            run_dependencies: None,
            pypi_dependencies: self.pypi_dependencies,
            activation: self.activation,
            tasks: self.tasks,
        }
        .into_feature_target(preview)?;

        let mut targets = IndexMap::new();
        for (selector, target) in self.target {
            let target = target.into_feature_target(preview)?;
            targets.insert(selector, target);
        }

        Ok(Feature {
            name,
            platforms: self.platforms,
            channels: self
                .channels
                .map(|channels| channels.into_iter().map(|channel| channel.into()).collect()),
            channel_priority: self.channel_priority,
            system_requirements: self.system_requirements,
            pypi_options: self.pypi_options,
            targets: Targets::from_default_and_user_defined(default_target, targets),
        })
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlFeature {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;

        let platforms = th
            .optional::<TomlWith<_, PixiSpanned<TomlIndexSet<TomlPlatform>>>>("platforms")
            .map(TomlWith::into_inner);
        let channels = th.optional("channels");
        let channel_priority = th.optional("channel-priority");
        let target = th
            .optional::<TomlIndexMap<_, _>>("target")
            .map(TomlIndexMap::into_inner)
            .unwrap_or_default();
        let dependencies = th.optional("dependencies");
        let host_dependencies = th.optional("host-dependencies");
        let build_dependencies = th.optional("build-dependencies");
        let pypi_dependencies = th
            .optional::<TomlIndexMap<_, _>>("pypi-dependencies")
            .map(TomlIndexMap::into_inner);
        let activation = th.optional("activation");
        let tasks = th
            .optional::<TomlHashMap<_, _>>("tasks")
            .map(TomlHashMap::into_inner)
            .unwrap_or_default();
        let pypi_options = th.optional("pypi-options");
        let system_requirements = th.optional("system-requirements").unwrap_or_default();

        th.finalize(None)?;

        Ok(TomlFeature {
            platforms,
            channels,
            channel_priority,
            system_requirements,
            target,
            dependencies,
            host_dependencies,
            build_dependencies,
            pypi_dependencies,
            activation,
            tasks,
            pypi_options,
        })
    }
}
