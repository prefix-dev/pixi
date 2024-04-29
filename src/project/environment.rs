use super::{
    errors::{UnknownTask, UnsupportedPlatformError},
    manifest::{self, EnvironmentName, Feature, FeatureName, SystemRequirements},
    SolveGroup,
};
use crate::project::combine_feature::HasFeatures;

use crate::task::TaskName;
use crate::{task::Task, Project};
use itertools::Either;
use rattler_conda_types::Platform;
use std::hash::{Hash, Hasher};
use std::{collections::HashMap, fmt::Debug};

/// Describes a single environment from a project manifest. This is used to describe environments
/// that can be installed and activated.
///
/// This struct is a higher level representation of a [`manifest::Environment`]. The
/// `manifest::Environment` describes the data stored in the manifest file, while this struct
/// provides methods to easily interact with an environment without having to deal with the
/// structure of the project model.
///
/// This type does not provide manipulation methods. To modify the data model you should directly
/// interact with the manifest instead.
///
/// The lifetime `'p` refers to the lifetime of the project that this environment belongs to.
#[derive(Clone)]
pub struct Environment<'p> {
    /// The project this environment belongs to.
    pub(super) project: &'p Project,

    /// The environment that this environment is based on.
    pub(super) environment: &'p manifest::Environment,
}

impl Debug for Environment<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Environment")
            .field("project", &self.project.name())
            .field("environment", &self.environment.name)
            .finish()
    }
}

impl<'p> PartialEq for Environment<'p> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.project, other.project)
            && std::ptr::eq(self.environment, other.environment)
    }
}

impl<'p> Eq for Environment<'p> {}

impl<'p> Environment<'p> {
    /// Return new instance of Environment
    pub fn new(project: &'p Project, environment: &'p manifest::Environment) -> Self {
        Self {
            project,
            environment,
        }
    }

    /// Returns true if this environment is the default environment.
    pub fn is_default(&self) -> bool {
        self.environment.name == EnvironmentName::Default
    }

    /// Returns the name of this environment.
    pub fn name(&self) -> &EnvironmentName {
        &self.environment.name
    }

    /// Returns the solve group to which this environment belongs, or `None` if no solve group was
    /// specified.
    pub fn solve_group(&self) -> Option<SolveGroup<'p>> {
        self.environment
            .solve_group
            .map(|solve_group_idx| SolveGroup {
                project: self.project,
                solve_group: &self.project.manifest.parsed.solve_groups.solve_groups
                    [solve_group_idx],
            })
    }

    /// Returns the manifest definition of this environment. See the documentation of
    /// [`Environment`] for an overview of the difference between [`manifest::Environment`] and
    /// [`Environment`].
    pub fn manifest(&self) -> &'p manifest::Environment {
        self.environment
    }

    /// Returns the directory where this environment is stored.
    pub fn dir(&self) -> std::path::PathBuf {
        self.project
            .environments_dir()
            .join(self.environment.name.as_str())
    }

    /// Returns the tasks defined for this environment.
    ///
    /// Tasks are defined on a per-target per-feature per-environment basis.
    ///
    /// If a `platform` is specified but this environment doesn't support the specified platform,
    /// an [`UnsupportedPlatformError`] error is returned.
    pub fn tasks(
        &self,
        platform: Option<Platform>,
    ) -> Result<HashMap<&'p TaskName, &'p Task>, UnsupportedPlatformError> {
        self.validate_platform_support(platform)?;
        let result = self
            .features()
            .flat_map(|feature| feature.targets.resolve(platform))
            .rev() // Reverse to get the most specific targets last.
            .flat_map(|target| target.tasks.iter())
            .collect();
        Ok(result)
    }

    /// Returns the task with the given `name` and for the specified `platform` or an `UnknownTask`
    /// which explains why the task was not available.
    pub fn task(
        &self,
        name: &TaskName,
        platform: Option<Platform>,
    ) -> Result<&'p Task, UnknownTask> {
        match self.tasks(platform).map(|tasks| tasks.get(name).copied()) {
            Err(_) | Ok(None) => Err(UnknownTask {
                project: self.project,
                environment: self.name().clone(),
                platform,
                task_name: name.clone(),
            }),
            Ok(Some(task)) => Ok(task),
        }
    }

    /// Returns the system requirements for this environment.
    ///
    /// The system requirements of the environment are the union of the system requirements of all
    /// the features that make up the environment. If multiple features specify a requirement for
    /// the same system package, the highest is chosen.
    ///
    /// If an environment defines a solve group the system requirements of all environments in the
    /// solve group are also combined. This means that if two environments in the same solve group
    /// specify conflicting system requirements that the highest system requirements are chosen.
    ///
    /// This is done to ensure that the requirements of all environments in the same solve group are
    /// compatible with each other.
    ///
    /// If you want to get the system requirements for this environment without taking the solve
    /// group into account, use the [`Self::local_system_requirements`] method.
    pub fn system_requirements(&self) -> SystemRequirements {
        if let Some(solve_group) = self.solve_group() {
            solve_group.system_requirements()
        } else {
            self.local_system_requirements()
        }
    }

    /// Returns the activation scripts that should be run when activating this environment.
    ///
    /// The activation scripts of all features are combined in the order they are defined for the
    /// environment.
    pub fn activation_scripts(&self, platform: Option<Platform>) -> Vec<String> {
        self.features()
            .filter_map(|f| f.activation_scripts(platform))
            .flatten()
            .cloned()
            .collect()
    }

    /// Validates that the given platform is supported by this environment.
    fn validate_platform_support(
        &self,
        platform: Option<Platform>,
    ) -> Result<(), UnsupportedPlatformError> {
        if let Some(platform) = platform {
            if !self.platforms().contains(&platform) {
                return Err(UnsupportedPlatformError {
                    environments_platforms: self.platforms().into_iter().collect(),
                    environment: self.name().clone(),
                    platform,
                });
            }
        }

        Ok(())
    }
}

impl<'p> HasFeatures<'p> for Environment<'p> {
    /// Returns references to the features that make up this environment.
    fn features(&self) -> impl DoubleEndedIterator<Item = &'p Feature> + 'p {
        let environment_features = self.environment.features.iter().map(|feature_name| {
            self.project
                .manifest
                .parsed
                .features
                .get(&FeatureName::Named(feature_name.clone()))
                .expect("feature usage should have been validated upfront")
        });

        if self.environment.no_default_feature {
            Either::Right(environment_features)
        } else {
            Either::Left(environment_features.chain([self.project.manifest.default_feature()]))
        }
    }

    /// Returns the project this environment belongs to.
    fn project(&self) -> &'p Project {
        self.project
    }
}

impl<'p> Hash for Environment<'p> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.environment.name.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use crate::project::Dependencies;

    use super::*;
    use insta::assert_snapshot;
    use itertools::Itertools;
    use rattler_conda_types::Channel;
    use std::{collections::HashSet, path::Path};

    #[test]
    fn test_default_channels() {
        let manifest = Project::from_str(
            Path::new("pixi.toml"),
            r#"
        [project]
        name = "foobar"
        channels = ["foo", "bar"]
        platforms = []
        "#,
        )
        .unwrap();

        let channels = manifest
            .default_environment()
            .channels()
            .into_iter()
            .map(Channel::canonical_name)
            .collect_vec();
        assert_eq!(
            channels,
            vec![
                "https://conda.anaconda.org/foo/",
                "https://conda.anaconda.org/bar/"
            ]
        );
    }

    // TODO: Add a test to verify that feature specific channels work as expected.

    #[test]
    fn test_default_platforms() {
        let manifest = Project::from_str(
            Path::new("pixi.toml"),
            r#"
        [project]
        name = "foobar"
        channels = []
        platforms = ["linux-64", "osx-64"]
        "#,
        )
        .unwrap();

        let channels = manifest.default_environment().platforms();
        assert_eq!(
            channels,
            HashSet::from_iter([Platform::Linux64, Platform::Osx64,])
        );
    }

    #[test]
    fn test_default_tasks() {
        let manifest = Project::from_str(
            Path::new("pixi.toml"),
            r#"
        [project]
        name = "foobar"
        channels = []
        platforms = ["linux-64"]

        [tasks]
        foo = "echo default"

        [target.linux-64.tasks]
        foo = "echo linux"
        "#,
        )
        .unwrap();

        let task = manifest
            .default_environment()
            .task(&"foo".into(), None)
            .unwrap()
            .as_single_command()
            .unwrap();

        assert_eq!(task, "echo default");

        let task_osx = manifest
            .default_environment()
            .task(&"foo".into(), Some(Platform::Linux64))
            .unwrap()
            .as_single_command()
            .unwrap();

        assert_eq!(task_osx, "echo linux");

        assert!(manifest
            .default_environment()
            .tasks(Some(Platform::Osx64))
            .is_err())
    }

    fn format_dependencies(dependencies: Dependencies) -> String {
        dependencies
            .into_specs()
            .map(|(name, spec)| format!("{} = {}", name.as_source(), spec))
            .join("\n")
    }

    #[test]
    fn test_dependencies() {
        let manifest = Project::from_str(
            Path::new("pixi.toml"),
            r#"
        [project]
        name = "foobar"
        channels = []
        platforms = ["linux-64", "osx-64"]

        [dependencies]
        foo = "*"

        [build-dependencies]
        foo = "<4.0"

        [target.osx-64.dependencies]
        foo = "<5.0"

        [feature.foo.dependencies]
        foo = ">=1.0"

        [feature.bar.dependencies]
        bar = ">=1.0"
        foo = "<2.0"

        [environments]
        foobar = ["foo", "bar"]
        "#,
        )
        .unwrap();

        let deps = manifest
            .environment("foobar")
            .unwrap()
            .dependencies(None, None);
        assert_snapshot!(format_dependencies(deps));
    }

    #[test]
    fn test_activation() {
        let manifest = Project::from_str(
            Path::new("pixi.toml"),
            r#"
        [project]
        name = "foobar"
        channels = []
        platforms = ["linux-64", "osx-64"]

        [activation]
        scripts = ["default.bat"]

        [target.linux-64.activation]
        scripts = ["linux.bat"]

        [feature.foo.activation]
        scripts = ["foo.bat"]

        [environments]
        foo = ["foo"]
                "#,
        )
        .unwrap();

        let foo_env = manifest.environment("foo").unwrap();
        assert_eq!(
            foo_env.activation_scripts(None),
            vec!["foo.bat".to_string(), "default.bat".to_string()]
        );
        assert_eq!(
            foo_env.activation_scripts(Some(Platform::Linux64)),
            vec!["foo.bat".to_string(), "linux.bat".to_string()]
        );
    }

    #[test]
    fn test_channel_priorities() {
        let manifest = Project::from_str(
            Path::new("pixi.toml"),
            r#"
        [project]
        name = "foobar"
        channels = ["conda-forge"]
        platforms = ["linux-64", "osx-64"]

        [feature.foo]
        channels = [{channel = "nvidia", priority = 1}, "pytorch"]

        [feature.bar]
        channels = [{ channel = "bar", priority = -10 }, "barry"]

        [environments]
        foo = ["foo"]
        bar = ["bar"]
        foobar = ["foo", "bar"]
        "#,
        )
        .unwrap();

        let foobar_channels = manifest.environment("foobar").unwrap().channels();
        assert_eq!(
            foobar_channels
                .into_iter()
                .map(|c| c.name.clone().unwrap())
                .collect_vec(),
            vec!["nvidia", "pytorch", "barry", "conda-forge", "bar"]
        );
        let foo_channels = manifest.environment("foo").unwrap().channels();
        assert_eq!(
            foo_channels
                .into_iter()
                .map(|c| c.name.clone().unwrap())
                .collect_vec(),
            vec!["nvidia", "pytorch", "conda-forge"]
        );

        let bar_channels = manifest.environment("bar").unwrap().channels();
        assert_eq!(
            bar_channels
                .into_iter()
                .map(|c| c.name.clone().unwrap())
                .collect_vec(),
            vec!["barry", "conda-forge", "bar"]
        );
    }
}
