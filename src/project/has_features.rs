use std::{borrow::Cow, collections::HashSet};

use indexmap::{IndexMap, IndexSet};
use itertools::Either;
use rattler_conda_types::{Channel, Platform};

use crate::{Project, SpecType};

use super::{
    manifest::{
        pypi_options::PypiOptions, python::PyPiPackageName, Feature, PyPiRequirement,
        SystemRequirements,
    },
    Dependencies,
};

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
        // We reverse once before collecting into an IndexSet, and once after,
        // to ensure the default channels of the project are added to the end of the list.
        let mut channels: IndexSet<_> = self
            .features()
            .flat_map(|feature| match &feature.channels {
                Some(channels) => channels,
                None => &self.project().manifest.parsed.project.channels,
            })
            .rev()
            .collect();
        channels.reverse();

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
    fn pypi_dependencies(
        &self,
        platform: Option<Platform>,
    ) -> IndexMap<PyPiPackageName, IndexSet<PyPiRequirement>> {
        self.features()
            .filter_map(|f| f.pypi_dependencies(platform))
            .fold(IndexMap::default(), |mut acc, deps| {
                // Either clone the values from the Cow or move the values from the owned map.
                let deps_iter = match deps {
                    Cow::Borrowed(borrowed) => Either::Left(
                        borrowed
                            .into_iter()
                            .map(|(name, spec)| (name.clone(), spec.clone())),
                    ),
                    Cow::Owned(owned) => Either::Right(owned.into_iter()),
                };

                // Add the requirements to the accumulator.
                for (name, spec) in deps_iter {
                    acc.entry(name).or_default().insert(spec);
                }

                acc
            })
    }

    /// Returns the dependencies to install for this collection.
    ///
    /// The dependencies of all features are combined. This means that if two features define a
    /// requirement for the same package that both requirements are returned. The different
    /// requirements per package are sorted in the same order as the features they came from.
    fn dependencies(&self, kind: Option<SpecType>, platform: Option<Platform>) -> Dependencies {
        self.features()
            .filter_map(|f| f.dependencies(kind, platform))
            .map(|deps| Dependencies::from(deps.into_owned()))
            .reduce(|acc, deps| acc.union(&deps))
            .unwrap_or_default()
    }

    /// Returns the pypi options for this solve group.
    fn pypi_options(&self) -> PypiOptions {
        self.features()
            .filter_map(|f| f.pypi_options())
            .fold(PypiOptions::default(), |acc, opt| {
                acc.union(opt)
                    .expect("pypi-options should have been validated upfront")
            })
    }
}
