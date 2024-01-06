use crate::project::manifest;
use crate::project::manifest::{EnvironmentName, Feature, FeatureName};
use crate::Project;
use indexmap::IndexSet;
use rattler_conda_types::{Channel, Platform};
use std::collections::HashSet;

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
pub struct Environment<'p> {
    /// The project this environment belongs to.
    pub(super) project: &'p Project,

    /// The environment that this environment is based on.
    pub(super) environment: &'p manifest::Environment,
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
                    .expect("fea")
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
            .reduce(|value, feat| value.intersection(&feat).copied().collect())
            .unwrap_or_default()
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
}
