use std::collections::HashMap;

use indexmap::{IndexMap, IndexSet};
use rattler_conda_types::Platform;
use rattler_solve::ChannelPriority;
use serde::Deserialize;
use serde_with::serde_as;

use crate::{
    pypi::{pypi_options::PypiOptions, PyPiPackageName},
    toml::{TomlPrioritizedChannel, TomlTarget},
    utils::{package_map::UniquePackageMap, PixiSpanned},
    Activation, Feature, FeatureName, Preview, PyPiRequirement, S3Options, SystemRequirements,
    TargetSelector, Targets, Task, TaskName, TomlError,
};

#[serde_as]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct TomlFeature {
    #[serde(default)]
    pub platforms: Option<PixiSpanned<IndexSet<Platform>>>,
    #[serde(default)]
    pub channels: Option<Vec<TomlPrioritizedChannel>>,
    #[serde(default)]
    pub channel_priority: Option<ChannelPriority>,
    #[serde(default)]
    pub system_requirements: SystemRequirements,
    #[serde(default)]
    pub target: IndexMap<PixiSpanned<TargetSelector>, TomlTarget>,
    #[serde(default)]
    pub dependencies: Option<PixiSpanned<UniquePackageMap>>,
    #[serde(default)]
    pub host_dependencies: Option<PixiSpanned<UniquePackageMap>>,
    #[serde(default)]
    pub build_dependencies: Option<PixiSpanned<UniquePackageMap>>,
    #[serde(default)]
    pub pypi_dependencies: Option<IndexMap<PyPiPackageName, PyPiRequirement>>,

    /// Additional information to activate an environment.
    #[serde(default)]
    pub activation: Option<Activation>,

    /// Target specific tasks to run in the environment
    #[serde(default)]
    pub tasks: HashMap<TaskName, Task>,

    /// Additional options for PyPi dependencies.
    #[serde(default)]
    pub pypi_options: Option<PypiOptions>,

    #[serde(default)]
    pub s3_options: Option<S3Options>,
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
            s3_options: self.s3_options,
            targets: Targets::from_default_and_user_defined(default_target, targets),
        })
    }
}
