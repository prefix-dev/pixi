use std::collections::HashMap;

use indexmap::IndexMap;
use pixi_spec::PixiSpec;
use serde::Deserialize;
use serde_with::serde_as;

use crate::{pypi::PyPiPackageName, Activation, PyPiRequirement, SpecType, Target, Task, TaskName};

#[serde_as]
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct TomlTarget {
    #[serde(default)]
    dependencies: IndexMap<rattler_conda_types::PackageName, PixiSpec>,

    #[serde(default)]
    host_dependencies: Option<IndexMap<rattler_conda_types::PackageName, PixiSpec>>,

    #[serde(default)]
    build_dependencies: Option<IndexMap<rattler_conda_types::PackageName, PixiSpec>>,

    #[serde(default)]
    pypi_dependencies: Option<IndexMap<PyPiPackageName, PyPiRequirement>>,

    /// Additional information to activate an environment.
    #[serde(default)]
    activation: Option<Activation>,

    /// Target specific tasks to run in the environment
    #[serde(default)]
    tasks: HashMap<TaskName, Task>,
}

impl TomlTarget {
    pub fn into_target(self) -> Target {
        let mut dependencies = HashMap::from_iter([(SpecType::Run, self.dependencies)]);
        if let Some(host_deps) = self.host_dependencies {
            dependencies.insert(SpecType::Host, host_deps);
        }
        if let Some(build_deps) = self.build_dependencies {
            dependencies.insert(SpecType::Build, build_deps);
        }

        Target {
            dependencies,
            pypi_dependencies: self.pypi_dependencies,
            activation: self.activation,
            tasks: self.tasks,
        }
    }
}
