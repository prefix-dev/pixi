use crate::project::errors::{UnknownTask, UnsupportedPlatformError};
use crate::project::manifest;
use crate::project::manifest::{EnvironmentName, Feature, FeatureName, SystemRequirements};
use crate::task::Task;
use crate::Project;
use indexmap::IndexSet;
use itertools::Itertools;
use rattler_conda_types::{Channel, Platform};
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;

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

impl<'p> Environment<'p> {
    /// Returns the name of this environment.
    pub fn name(&self) -> &EnvironmentName {
        &self.environment.name
    }

    /// Returns the manifest definition of this environment. See the documentation of
    /// [`Environment`] for an overview of the difference between [`manifest::Environment`] and
    /// [`Environment`].
    pub fn manifest(&self) -> &'p manifest::Environment {
        self.environment
    }

    /// Returns references to the features that make up this environment. The default feature is
    /// always added at the end.
    pub fn features(&self) -> impl Iterator<Item = &'p Feature> + '_ {
        self.environment
            .features
            .iter()
            .map(|feature_name| {
                self.project
                    .manifest
                    .parsed
                    .features
                    .get(&FeatureName::Named(feature_name.clone()))
                    .expect("feature usage should have been validated upfront")
            })
            .chain([self.project.manifest.default_feature()])
    }

    /// Returns the channels associated with this environment.
    ///
    /// Users can specify custom channels on a per feature basis. This method collects and
    /// deduplicates all the channels from all the features in the order they are defined in the
    /// manifest.
    ///
    /// If a feature does not specify any channel the default channels from the project metadata are
    /// used instead. However, these are not considered during deduplication. This means the default
    /// channels are always added to the end of the list.
    pub fn channels(&self) -> IndexSet<&'p Channel> {
        self.features()
            .filter_map(|feature| match feature.name {
                // Use the user-specified channels of each feature if the feature defines them. Only
                // for the default feature do we use the default channels from the project metadata
                // if the feature itself does not specify any channels. This guarantees that the
                // channels from the default feature are always added to the end of the list.
                FeatureName::Named(_) => feature.channels.as_deref(),
                FeatureName::Default => feature
                    .channels
                    .as_deref()
                    .or(Some(&self.project.manifest.parsed.project.channels)),
            })
            .flatten()
            .collect()
    }

    /// Returns the platforms that this environment is compatible with.
    ///
    /// Which platforms an environment support depends on which platforms the selected features of
    /// the environment supports. The platforms that are supported by the environment is the
    /// intersection of the platforms supported by its features.
    ///
    /// Features can specify which platforms they support through the `platforms` key. If a feature
    /// does not specify any platforms the features defined by the project are used.
    pub fn platforms(&self) -> HashSet<Platform> {
        self.features()
            .map(|feature| {
                match &feature.platforms {
                    Some(platforms) => &platforms.value,
                    None => &self.project.manifest.parsed.project.platforms.value,
                }
                .iter()
                .copied()
                .collect::<HashSet<_>>()
            })
            .reduce(|accumulated_platforms, feat| {
                accumulated_platforms.intersection(&feat).copied().collect()
            })
            .unwrap_or_default()
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
    ) -> Result<HashMap<&'p str, &'p Task>, UnsupportedPlatformError> {
        self.validate_platform_support(platform)?;
        let result = self
            .features()
            .flat_map(|feature| feature.targets.resolve(platform))
            .collect_vec()
            .into_iter()
            .rev() // Reverse to get the most specific targets last.
            .flat_map(|target| target.tasks.iter())
            .map(|(name, task)| (name.as_str(), task))
            .collect();
        Ok(result)
    }

    /// Returns the task with the given `name` and for the specified `platform` or an `UnknownTask`
    /// which explains why the task was not available.
    pub fn task(&self, name: &str, platform: Option<Platform>) -> Result<&'p Task, UnknownTask> {
        match self.tasks(platform).map(|tasks| tasks.get(name).copied()) {
            Err(_) | Ok(None) => Err(UnknownTask {
                project: self.project,
                environment: self.name().clone(),
                platform,
                task_name: name.to_string(),
            }),
            Ok(Some(task)) => Ok(task),
        }
    }

    /// Returns the system requirements for this environment.
    ///
    /// The system requirements of the environment are the union of the system requirements of all
    /// the features that make up the environment. If multiple features specify a requirement for
    /// the same system package, the highest is chosen.
    pub fn system_requirements(&self) -> SystemRequirements {
        self.features()
            .map(|feature| &feature.system_requirements)
            .fold(SystemRequirements::default(), |acc, req| {
                acc.union(req)
                    .expect("system requirements should have been validated upfront")
            })
    }

    /// Validates that the given platform is supported by this environment.
    fn validate_platform_support(
        &self,
        platform: Option<Platform>,
    ) -> Result<(), UnsupportedPlatformError> {
        if let Some(platform) = platform {
            if !self.platforms().contains(&platform) {
                return Err(UnsupportedPlatformError {
                    project: self.project,
                    environment: self.name().clone(),
                    platform,
                });
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use itertools::Itertools;
    use std::path::Path;

    #[test]
    fn test_default_channels() {
        let manifest = Project::from_str(
            Path::new(""),
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
            Path::new(""),
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
            Path::new(""),
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
            .task("foo", None)
            .unwrap()
            .as_single_command()
            .unwrap();

        assert_eq!(task, "echo default");

        let task_osx = manifest
            .default_environment()
            .task("foo", Some(Platform::Linux64))
            .unwrap()
            .as_single_command()
            .unwrap();

        assert_eq!(task_osx, "echo linux");

        assert!(manifest
            .default_environment()
            .tasks(Some(Platform::Osx64))
            .is_err())
    }
}
