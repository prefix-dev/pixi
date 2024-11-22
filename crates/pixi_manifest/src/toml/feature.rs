use std::collections::HashMap;

use indexmap::{IndexMap, IndexSet};
use pixi_spec::PixiSpec;
use rattler_conda_types::Platform;
use rattler_solve::ChannelPriority;
use serde::Deserialize;
use serde_with::serde_as;

use crate::{
    pypi::{pypi_options::PypiOptions, PyPiPackageName},
    toml::{TomlPrioritizedChannel, TomlTarget},
    utils::PixiSpanned,
    Activation, Feature, FeatureName, PyPiRequirement, SpecType, SystemRequirements, Target,
    TargetSelector, Targets, Task, TaskName,
};

#[serde_as]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct TomlFeature {
    #[serde(default)]
    platforms: Option<PixiSpanned<IndexSet<Platform>>>,
    #[serde(default)]
    channels: Option<Vec<TomlPrioritizedChannel>>,
    #[serde(default)]
    channel_priority: Option<ChannelPriority>,
    #[serde(default)]
    system_requirements: SystemRequirements,
    #[serde(default)]
    target: IndexMap<PixiSpanned<TargetSelector>, TomlTarget>,

    #[serde(
        default,
        deserialize_with = "crate::utils::package_map::deserialize_package_map"
    )]
    dependencies: IndexMap<rattler_conda_types::PackageName, PixiSpec>,

    #[serde(
        default,
        deserialize_with = "crate::utils::package_map::deserialize_opt_package_map"
    )]
    host_dependencies: Option<IndexMap<rattler_conda_types::PackageName, PixiSpec>>,

    #[serde(
        default,
        deserialize_with = "crate::utils::package_map::deserialize_opt_package_map"
    )]
    build_dependencies: Option<IndexMap<rattler_conda_types::PackageName, PixiSpec>>,

    #[serde(default)]
    pypi_dependencies: Option<IndexMap<PyPiPackageName, PyPiRequirement>>,

    /// Additional information to activate an environment.
    #[serde(default)]
    activation: Option<Activation>,

    /// Target specific tasks to run in the environment
    #[serde(default)]
    tasks: HashMap<TaskName, Task>,

    /// Additional options for PyPi dependencies.
    #[serde(default)]
    pypi_options: Option<PypiOptions>,
}

impl TomlFeature {
    pub fn into_future(self, name: FeatureName) -> Feature {
        let mut dependencies = HashMap::from_iter([(SpecType::Run, self.dependencies)]);
        if let Some(host_deps) = self.host_dependencies {
            dependencies.insert(SpecType::Host, host_deps);
        }
        if let Some(build_deps) = self.build_dependencies {
            dependencies.insert(SpecType::Build, build_deps);
        }

        let default_target = Target {
            dependencies,
            pypi_dependencies: self.pypi_dependencies,
            activation: self.activation,
            tasks: self.tasks,
        };

        Feature {
            name,
            platforms: self.platforms,
            channels: self
                .channels
                .map(|channels| channels.into_iter().map(|channel| channel.into()).collect()),
            channel_priority: self.channel_priority,
            system_requirements: self.system_requirements,
            pypi_options: self.pypi_options,
            targets: Targets::from_default_and_user_defined(
                default_target,
                self.target
                    .into_iter()
                    .map(|(selector, target)| (selector, target.into_target()))
                    .collect(),
            ),
        }
    }
}
