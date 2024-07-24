use std::collections::HashSet;

use indexmap::IndexSet;
use rattler_conda_types::{Channel, Platform};
use rattler_solve::ChannelPriority;

use crate::Project;
use pixi_manifest::SpecType;

use super::{CondaDependencies, PyPiDependencies};
use pixi_manifest::{pypi::pypi_options::PypiOptions, Feature, SystemRequirements};

/// ChannelPriorityCombination error, thrown when multiple channel priorities are set
#[derive(Debug, thiserror::Error)]
#[error("Multiple channel priorities are not allowed in a single environment")]
pub struct ChannelPriorityCombinationError;

/// A trait that implement various methods for collections that combine attributes of Features
/// It is implemented by Environment, GroupedEnvironment and SolveGroup
pub trait HasFeatures<'p> {
    fn features(&self) -> impl DoubleEndedIterator<Item = &'p Feature> + 'p;
    fn project(&self) -> &'p Project;

    /// Returns the channels associated with this collection.
    ///
    /// Users can specify custom channels on a per feature basis. This method collects and
    /// deduplicates all the channels from all the features in the order they are defined in the
    /// manifest.
    ///
    /// If a feature does not specify any channel the default channels from the project metadata are
    /// used instead.
    fn channels(&self) -> IndexSet<&'p Channel> {
        // Collect all the channels from the features in one set,
        // deduplicate them and sort them on feature index, default feature comes last.
        let channels: IndexSet<_> = self
            .features()
            .flat_map(|feature| match &feature.channels {
                Some(channels) => channels,
                None => &self.project().manifest.parsed.project.channels,
            })
            .collect();

        // The prioritized channels contain a priority, sort on this priority.
        // Higher priority comes first. [-10, 1, 0 ,2] -> [2, 1, 0, -10]
        channels
            .sorted_by(|a, b| {
                let a = a.priority.unwrap_or(0);
                let b = b.priority.unwrap_or(0);
                b.cmp(&a)
            })
            .map(|prioritized_channel| &prioritized_channel.channel)
            .collect()
    }

    /// Returns the channel priority, error on multiple values, return None if no value is set.
    ///
    /// When using multiple channel priorities over different features we should error as the user should decide what they want.
    fn channel_priority(&self) -> Result<Option<ChannelPriority>, ChannelPriorityCombinationError> {
        let mut channel_priority = None;
        for feature in self.features() {
            if let Some(priority) = feature.channel_priority {
                if channel_priority == Some(priority) {
                    return Err(ChannelPriorityCombinationError);
                }
                channel_priority = Some(priority);
            }
        }
        Ok(channel_priority)
    }

    /// Returns the platforms that this collection is compatible with.
    ///
    /// Which platforms a collection support depends on which platforms the selected features of
    /// the collection supports. The platforms that are supported by the collection is the
    /// intersection of the platforms supported by its features.
    ///
    /// Features can specify which platforms they support through the `platforms` key. If a feature
    /// does not specify any platforms the features defined by the project are used.
    fn platforms(&self) -> HashSet<Platform> {
        self.features()
            .map(|feature| {
                match &feature.platforms {
                    Some(platforms) => &platforms.value,
                    None => &self.project().manifest.parsed.project.platforms.value,
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

    /// Returns the system requirements for this collection.
    ///
    /// The system requirements of the collection are the union of the system requirements of all
    /// the features in the collection. If multiple features specify a
    /// requirement for the same system package, the highest is chosen.
    fn local_system_requirements(&self) -> SystemRequirements {
        self.features()
            .map(|feature| &feature.system_requirements)
            .fold(SystemRequirements::default(), |acc, req| {
                acc.union(req)
                    .expect("system requirements should have been validated upfront")
            })
    }

    /// Returns true if any of the features has any reference to a pypi dependency.
    fn has_pypi_dependencies(&self) -> bool {
        self.features().any(|f| f.has_pypi_dependencies())
    }

    /// Returns the PyPi dependencies to install for this collection.
    ///
    /// The dependencies of all features are combined. This means that if two features define a
    /// requirement for the same package that both requirements are returned. The different
    /// requirements per package are sorted in the same order as the features they came from.
    fn pypi_dependencies(&self, platform: Option<Platform>) -> PyPiDependencies {
        self.features()
            .filter_map(|f| f.pypi_dependencies(platform))
            .into()
    }

    /// Returns the dependencies to install for this collection.
    ///
    /// The dependencies of all features are combined. This means that if two features define a
    /// requirement for the same package that both requirements are returned. The different
    /// requirements per package are sorted in the same order as the features they came from.
    fn dependencies(
        &self,
        kind: Option<SpecType>,
        platform: Option<Platform>,
    ) -> CondaDependencies {
        self.features()
            .filter_map(|f| f.dependencies(kind, platform))
            .into()
    }

    /// Returns the pypi options for this collection.
    ///
    /// The pypi options of all features are combined. They will be combined in the order
    /// that they are defined in the manifest.
    /// The index-url is a special case and can only be defined once. This should have been
    /// verified before-hand.
    fn pypi_options(&self) -> PypiOptions {
        // Collect all the pypi-options from the features in one set,
        // deduplicate them and sort them on feature index, default feature comes last.
        let pypi_options: Vec<_> = self
            .features()
            .filter_map(|feature| {
                if feature.pypi_options().is_none() {
                    self.project().manifest.parsed.project.pypi_options.as_ref()
                } else {
                    feature.pypi_options()
                }
            })
            .collect();

        // Merge all the pypi options into one.
        pypi_options
            .into_iter()
            .fold(PypiOptions::default(), |acc, opts| {
                acc.union(opts)
                    .expect("merging of pypi-options should already have been checked")
            })
    }
}
