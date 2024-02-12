use super::{manifest, Dependencies, Environment, Project};
use crate::project::manifest::{PyPiRequirement, SystemRequirements};
use crate::{FeatureName, SpecType};
use indexmap::{IndexMap, IndexSet};
use itertools::{Either, Itertools};
use rattler_conda_types::{Channel, Platform};
use std::borrow::Cow;
use std::hash::Hash;
use std::path::PathBuf;

/// A grouping of environments that are solved together.
#[derive(Debug, Clone)]
pub struct SolveGroup<'p> {
    /// The project that the group is part of.
    pub(super) project: &'p Project,

    /// A reference to the solve group in the manifest
    pub(super) solve_group: &'p manifest::SolveGroup,
}

impl PartialEq<Self> for SolveGroup<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.solve_group, other.solve_group)
            && std::ptr::eq(self.project, other.project)
    }
}

impl Eq for SolveGroup<'_> {}

impl Hash for SolveGroup<'_> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::ptr::hash(self.solve_group, state);
        std::ptr::hash(self.project, state);
    }
}

impl<'p> SolveGroup<'p> {
    /// Returns the project to which the group belongs.
    pub fn project(&self) -> &'p Project {
        self.project
    }

    /// The name of the group
    pub fn name(&self) -> &str {
        &self.solve_group.name
    }

    /// Returns the directory where this solve group stores its environment
    pub fn dir(&self) -> PathBuf {
        self.project
            .solve_group_environments_dir()
            .join(self.name())
    }

    /// Returns an iterator over all the environments that are part of the group.
    pub fn environments(&self) -> impl Iterator<Item = Environment<'p>> + DoubleEndedIterator + ExactSizeIterator + 'p {
        self.solve_group
            .environments
            .iter()
            .map(|env_idx| Environment {
                project: self.project,
                environment: &self.project.manifest.parsed.environments.environments[*env_idx],
            })
    }

    /// Returns all features that are part of the solve group.
    ///
    /// If `include_default` is `true` the default feature is also included.
    ///
    /// All features of all environments are combined and deduplicated.
    pub fn features(
        &self,
        include_default: bool,
    ) -> impl Iterator<Item = &'p manifest::Feature> + DoubleEndedIterator + 'p {
        self.environments()
            .flat_map(move |env| env.features(include_default))
            .unique_by(|feat| &feat.name)
    }

    /// Returns the system requirements for this solve group.
    ///
    /// The system requirements of the solve group are the union of the system requirements of all
    /// the environments that share the same solve group. If multiple environments specify a
    /// requirement for the same system package, the highest is chosen.
    pub fn system_requirements(&self) -> SystemRequirements {
        self.features(true)
            .map(|feature| &feature.system_requirements)
            .fold(SystemRequirements::default(), |acc, req| {
                acc.union(req)
                    .expect("system requirements should have been validated upfront")
            })
    }

    /// Returns all the dependencies of the solve group.
    ///
    /// The dependencies of all features of all environments are combined. This means that if two
    /// features define a requirement for the same package that both requirements are returned. The
    /// different requirements per package are sorted in the same order as the features they came
    /// from.
    pub fn dependencies(&self, kind: Option<SpecType>, platform: Option<Platform>) -> Dependencies {
        self.features(true)
            .filter_map(|feat| feat.dependencies(kind, platform))
            .map(|deps| Dependencies::from(deps.into_owned()))
            .reduce(|acc, deps| acc.union(&deps))
            .unwrap_or_default()
    }

    /// Returns all the pypi dependencies of the solve group.
    ///
    /// The dependencies of all features of all environments in the solve group are combined. This
    /// means that if two features define a requirement for the same package that both requirements
    /// are returned. The different requirements per package are sorted in the same order as the
    /// features they came from.
    pub fn pypi_dependencies(
        &self,
        platform: Option<Platform>,
    ) -> IndexMap<rip::types::PackageName, Vec<PyPiRequirement>> {
        self.features(true)
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
                    acc.entry(name).or_default().push(spec);
                }

                acc
            })
    }

    /// Returns the channels associated with this solve group.
    ///
    /// Users can specify custom channels on a per-feature basis. This method collects and
    /// deduplicates all the channels from all the features in the order they are defined in the
    /// manifest.
    ///
    /// If a feature does not specify any channel the default channels from the project metadata are
    /// used instead. However, these are not considered during deduplication. This means the default
    /// channels are always added to the end of the list.
    pub fn channels(&self) -> IndexSet<&'p Channel> {
        self.features(true)
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
            // The prioritized channels contain a priority, sort on this priority.
            // Higher priority comes first. [-10, 1, 0 ,2] -> [2, 1, 0, -10]
            .sorted_by(|a, b| {
                let a = a.priority.unwrap_or(0);
                let b = b.priority.unwrap_or(0);
                b.cmp(&a)
            })
            .map(|prioritized_channel| &prioritized_channel.channel)
            .collect()
    }

    /// Returns true if any of the environments contain a feature with any reference to a pypi dependency.
    pub fn has_pypi_dependencies(&self) -> bool {
        self.features(true).any(|f| f.has_pypi_dependencies())
    }
}

#[cfg(test)]
mod tests {
    use crate::Project;
    use itertools::Itertools;
    use rattler_conda_types::PackageName;
    use std::collections::HashSet;
    use std::path::Path;

    #[test]
    fn test_solve_group() {
        let project = Project::from_str(
            Path::new(""),
            r#"
        [project]
        name = "foobar"
        channels = ["conda-forge"]
        platforms = ["linux-64", "osx-64"]

        [dependencies]
        a = "*"

        [feature.foo.dependencies]
        b = "*"

        [feature.bar.dependencies]
        c = "*"

        [feature.bar.system-requirements]
        cuda = "12.0"

        [environments]
        foo = { features=["foo"], solve-group="group1" }
        bar = { features=["bar"], solve-group="group1" }
        "#,
        )
        .unwrap();

        let environments = project.environments();
        assert_eq!(environments.len(), 3);

        let default_environment = project.default_environment();
        let foo_environment = project.environment("foo").unwrap();
        let bar_environment = project.environment("bar").unwrap();

        let solve_groups = project.solve_groups();
        assert_eq!(solve_groups.len(), 1);

        let solve_group = solve_groups[0].clone();
        let solve_group_envs = solve_group.environments().collect_vec();
        assert_eq!(solve_group_envs.len(), 2);
        assert_eq!(solve_group_envs[0].name(), "foo");
        assert_eq!(solve_group_envs[1].name(), "bar");

        // Make sure that the environments properly reference the group
        assert_eq!(foo_environment.solve_group(), Some(solve_group.clone()));
        assert_eq!(bar_environment.solve_group(), Some(solve_group.clone()));
        assert_eq!(default_environment.solve_group(), None);

        // Make sure that all the environments share the same system requirements, because they are
        // in the same solve-group.
        let foo_system_requirements = foo_environment.system_requirements();
        let bar_system_requirements = bar_environment.system_requirements();
        let default_system_requirements = default_environment.system_requirements();
        assert_eq!(foo_system_requirements.cuda, "12.0".parse().ok());
        assert_eq!(bar_system_requirements.cuda, "12.0".parse().ok());
        assert_eq!(default_system_requirements.cuda, None);

        // Check that the solve group contains all the dependencies of its environments
        let package_names: HashSet<_> = solve_group
            .dependencies(None, None)
            .names()
            .cloned()
            .collect();
        assert_eq!(
            package_names,
            ["a", "b", "c"]
                .into_iter()
                .map(PackageName::new_unchecked)
                .collect()
        );
    }
}
