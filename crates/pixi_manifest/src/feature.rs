use crate::{
    SpecType, SystemRequirements, WorkspaceTarget, channel::PrioritizedChannel, consts,
    pypi::pypi_options::PypiOptions, target::Targets, workspace::ChannelPriority,
};
use indexmap::{IndexMap, IndexSet};
use pixi_pypi_spec::{PixiPypiSpec, PypiPackageName};
use pixi_spec::PixiSpec;
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::{PackageName, Platform};
use serde::{Deserialize, Serialize};
use std::ops::Not;
use std::{
    borrow::{Borrow, Cow},
    convert::Infallible,
    fmt,
    hash::{Hash, Hasher},
    str::FromStr,
};

/// The name of a feature. This is either a string or default for the default
/// feature.
#[derive(Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub struct FeatureName(Cow<'static, str>);

impl Default for FeatureName {
    fn default() -> Self {
        FeatureName::DEFAULT.clone()
    }
}

impl Serialize for FeatureName {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl Hash for FeatureName {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state)
    }
}

impl<'de> Deserialize<'de> for FeatureName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(String::deserialize(deserializer)?.into())
    }
}

impl<'s> From<&'s str> for FeatureName {
    fn from(value: &'s str) -> Self {
        FeatureName(Cow::Owned(value.to_owned()))
    }
}

impl From<String> for FeatureName {
    fn from(value: String) -> Self {
        Self(Cow::Owned(value))
    }
}

impl FeatureName {
    pub const DEFAULT: Self = FeatureName(Cow::Borrowed(consts::DEFAULT_FEATURE_NAME));

    /// Returns the string representation of the feature.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns true if the feature is the default feature.
    pub fn is_default(&self) -> bool {
        self == &Self::DEFAULT
    }

    /// Returns the name of the feature if it is not default.
    pub fn non_default(&self) -> Option<&str> {
        self.is_default().not().then(|| self.as_str())
    }
}

impl Borrow<str> for FeatureName {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl FromStr for FeatureName {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(FeatureName::from(s))
    }
}

impl From<FeatureName> for String {
    fn from(name: FeatureName) -> Self {
        name.0.into_owned()
    }
}
impl<'a> From<&'a FeatureName> for String {
    fn from(name: &'a FeatureName) -> Self {
        name.as_str().to_owned()
    }
}
impl fmt::Display for FeatureName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A feature describes a set of functionalities. It allows us to group
/// functionality and its dependencies together.
///
/// Individual features cannot be used directly, instead they are grouped
/// together into environments. Environments are then locked and installed.
#[derive(Debug, Clone)]
pub struct Feature {
    /// The name of the feature or `None` if the feature is the default feature.
    pub name: FeatureName,

    /// The platforms this feature is available on.
    ///
    /// This value is `None` if this feature does not specify any platforms and
    /// the default platforms from the project should be used.
    pub platforms: Option<IndexSet<Platform>>,

    /// Channels specific to this feature.
    ///
    /// This value is `None` if this feature does not specify any channels and
    /// the default channels from the project should be used.
    pub channels: Option<IndexSet<PrioritizedChannel>>,

    /// Channel priority for the solver, if not set the default is used.
    /// This value is `None` and there are multiple features,
    /// it will be seen as unset and overwritten by a set one.
    pub channel_priority: Option<ChannelPriority>,

    /// Additional system requirements
    pub system_requirements: SystemRequirements,

    /// Pypi-related options
    pub pypi_options: Option<PypiOptions>,

    /// Target specific configuration.
    pub targets: Targets<WorkspaceTarget>,
}

impl Feature {
    /// Construct a new feature with the given name.
    pub fn new(name: FeatureName) -> Self {
        Feature {
            name,
            platforms: None,
            channels: None,
            channel_priority: None,
            system_requirements: SystemRequirements::default(),
            pypi_options: None,
            targets: <Targets<WorkspaceTarget> as Default>::default(),
        }
    }

    /// Returns true if this feature is the default feature.
    pub fn is_default(&self) -> bool {
        self.name.is_default()
    }

    /// Returns a mutable reference to the platforms of the feature. Create them
    /// if needed
    pub fn platforms_mut(&mut self) -> &mut IndexSet<Platform> {
        self.platforms.get_or_insert_with(Default::default)
    }

    /// Returns a mutable reference to the channels of the feature. Create them
    /// if needed
    pub fn channels_mut(&mut self) -> &mut IndexSet<PrioritizedChannel> {
        self.channels.get_or_insert_with(Default::default)
    }

    /// Returns the run dependencies of the target for the given `platform`.
    ///
    /// If the platform is `None` no platform specific dependencies are
    /// returned.
    ///
    /// This function returns `None` if there is not a single feature that has
    /// any dependencies defined.
    pub fn run_dependencies(
        &self,
        platform: Option<Platform>,
    ) -> Option<Cow<'_, DependencyMap<PackageName, PixiSpec>>> {
        self.dependencies(SpecType::Run, platform)
    }

    /// Returns the host dependencies of the target for the given `platform`.
    ///
    /// If the platform is `None` no platform specific dependencies are
    /// returned.
    ///
    /// This function returns `None` if there is not a single feature that has
    /// any dependencies defined.
    pub fn host_dependencies(
        &self,
        platform: Option<Platform>,
    ) -> Option<Cow<'_, DependencyMap<PackageName, PixiSpec>>> {
        self.dependencies(SpecType::Host, platform)
    }

    /// Returns the run dependencies of the target for the given `platform`.
    ///
    /// If the platform is `None` no platform specific dependencies are
    /// returned.
    ///
    /// This function returns `None` if there is not a single feature that has
    /// any dependencies defined.
    pub fn build_dependencies(
        &self,
        platform: Option<Platform>,
    ) -> Option<Cow<'_, DependencyMap<PackageName, PixiSpec>>> {
        self.dependencies(SpecType::Build, platform)
    }

    /// Returns the dependencies of the feature for a given `spec_type` and
    /// `platform`.
    ///
    /// This function returns a [`Cow`]. If the dependencies are not combined or
    /// overwritten by multiple targets than this function returns a
    /// reference to the internal dependencies.
    ///
    /// Returns `None` if this feature does not define any target that has any
    /// of the requested dependencies.
    ///
    /// If the `platform` is `None` no platform specific dependencies are taken
    /// into consideration.
    pub fn dependencies(
        &self,
        spec_type: SpecType,
        platform: Option<Platform>,
    ) -> Option<Cow<'_, DependencyMap<PackageName, PixiSpec>>> {
        self.targets
            .resolve(platform)
            // Get the targets in reverse order, from least specific to most specific.
            // This is required because we want more specific targets to overwrite their specs.
            .rev()
            .filter_map(|t| t.dependencies(spec_type))
            .filter(|deps| !deps.is_empty())
            .fold(None, |acc, deps| match acc {
                None => Some(Cow::Borrowed(deps)),
                Some(acc) => {
                    // Overwrite the accumulator with specs from this target
                    // More specific targets (processed later) overwrite less specific ones
                    Some(Cow::Owned(acc.as_ref().overwrite(deps)))
                }
            })
    }

    /// Returns the combined dependencies of the feature and `platform`.
    ///
    /// The `build` dependencies overwrite the `host` dependencies which
    /// overwrite the `run` dependencies.
    ///
    /// This function returns a [`Cow`]. If the dependencies are not combined or
    /// overwritten by multiple targets than this function returns a
    /// reference to the internal dependencies.
    ///
    /// Returns `None` if this feature does not define any target that has any
    /// of the requested dependencies.
    ///
    /// If the `platform` is `None` no platform specific dependencies are taken
    /// into consideration.
    pub fn combined_dependencies(
        &self,
        platform: Option<Platform>,
    ) -> Option<Cow<'_, DependencyMap<PackageName, PixiSpec>>> {
        self.targets
            .resolve(platform)
            // Get the targets in reverse order, from least specific to most specific.
            // This is required because we want more specific targets to overwrite their specs.
            .rev()
            .filter_map(|t| t.combined_dependencies())
            .filter(|deps| !deps.is_empty())
            .fold(None, |acc, deps| match acc {
                None => Some(deps),
                Some(acc) => {
                    // Overwrite the accumulator with specs from this target
                    // More specific targets (processed later) overwrite less specific ones
                    Some(Cow::Owned(acc.as_ref().overwrite(deps.as_ref())))
                }
            })
    }

    /// Returns the PyPi dependencies of the feature for a given `platform`.
    ///
    /// This function returns a [`Cow`]. If the dependencies are not combined or
    /// overwritten by multiple targets than this function returns a
    /// reference to the internal dependencies.
    ///
    /// Returns `None` if this feature does not define any target that has any
    /// of the requested dependencies.
    pub fn pypi_dependencies(
        &self,
        platform: Option<Platform>,
    ) -> Option<Cow<'_, DependencyMap<PypiPackageName, PixiPypiSpec>>> {
        self.targets
            .resolve(platform)
            // Get the targets in reverse order, from least specific to most specific.
            // This is required because we want more specific targets to overwrite their specs.
            .rev()
            .filter_map(|t| t.pypi_dependencies.as_ref())
            .filter(|deps| !deps.is_empty())
            .fold(None, |acc, deps| match acc {
                None => Some(Cow::Borrowed(deps)),
                Some(acc) => {
                    // Overwrite the accumulator with specs from this target
                    // More specific targets (processed later) overwrite less specific ones
                    Some(Cow::Owned(acc.as_ref().overwrite(deps)))
                }
            })
    }

    /// Returns the activation scripts for the most specific target that matches
    /// the given `platform`.
    ///
    /// Returns `None` if this feature does not define any target with an
    /// activation.
    pub fn activation_scripts(&self, platform: Option<Platform>) -> Option<&Vec<String>> {
        self.targets
            .resolve(platform)
            .filter_map(|t| t.activation.as_ref())
            .filter_map(|a| a.scripts.as_ref())
            .next()
    }

    /// Returns the activation environment for the most specific target that
    /// matches the given `platform`.
    ///
    /// Returns `None` if this feature does not define any target with an
    /// activation.
    pub fn activation_env(&self, platform: Option<Platform>) -> IndexMap<String, String> {
        self.targets
            .resolve(platform)
            .filter_map(|t| t.activation.as_ref())
            .filter_map(|a| a.env.as_ref())
            .fold(IndexMap::new(), |mut acc, x| {
                for (k, v) in x {
                    if !acc.contains_key(k) {
                        acc.insert(k.clone(), v.clone());
                    }
                }
                acc
            })
    }

    /// Returns true if the feature contains any reference to a pypi
    /// dependencies.
    pub fn has_pypi_dependencies(&self) -> bool {
        self.targets.targets().any(|t| {
            t.pypi_dependencies
                .as_ref()
                .map(|deps| !deps.is_empty())
                .unwrap_or(false)
        })
    }

    /// Returns any pypi_options if they are set.
    pub fn pypi_options(&self) -> Option<&PypiOptions> {
        self.pypi_options.as_ref()
    }
}

#[cfg(test)]
mod tests {

    use assert_matches::assert_matches;

    use super::*;
    use crate::WorkspaceManifest;

    #[test]
    fn test_dependencies_borrowed() {
        let manifest = WorkspaceManifest::from_toml_str(
            r#"
        [project]
        name = "foo"
        platforms = ["linux-64", "osx-64", "win-64"]
        channels = []

        [dependencies]
        foo = "1.0"

        [host-dependencies]
        foo = "2.0"

        [feature.bla.dependencies]
        foo = "2.0"

        [feature.bla.host-dependencies]
        # empty on purpose
        "#,
        )
        .unwrap();

        assert_matches!(
            manifest
                .default_feature()
                .dependencies(SpecType::Host, None)
                .unwrap(),
            Cow::Borrowed(_),
            "[host-dependencies] can be borrowed when returning DependencyMap"
        );

        assert_matches!(
            manifest
                .default_feature()
                .dependencies(SpecType::Run, None)
                .unwrap(),
            Cow::Borrowed(_),
            "[dependencies] can be borrowed when returning DependencyMap"
        );

        assert_matches!(
            manifest
                .default_feature()
                .combined_dependencies(None)
                .unwrap(),
            Cow::Owned(_),
            "combined dependencies should be owned"
        );

        let bla_feature = manifest.features.get(&FeatureName::from("bla")).unwrap();
        assert_matches!(
            bla_feature.dependencies(SpecType::Run, None).unwrap(),
            Cow::Borrowed(_),
            "[feature.bla.dependencies] can be borrowed when returning DependencyMap"
        );

        assert_matches!(
            bla_feature.combined_dependencies(None).unwrap(),
            Cow::Borrowed(_),
            "[feature.bla] combined dependencies can be borrowed when returning DependencyMap"
        );
    }

    #[test]
    fn test_activation() {
        let manifest = WorkspaceManifest::from_toml_str(
            r#"
        [project]
        name = "foo"
        platforms = ["linux-64", "osx-64", "win-64"]
        channels = []

        [activation]
        scripts = ["run.bat"]

        [target.linux-64.activation]
        scripts = ["linux-64.bat"]
        "#,
        )
        .unwrap();

        assert_eq!(
            manifest.default_feature().activation_scripts(None).unwrap(),
            &vec!["run.bat".to_string()],
            "should have selected the activation from the [activation] section"
        );
        assert_eq!(
            manifest
                .default_feature()
                .activation_scripts(Some(Platform::Linux64))
                .unwrap(),
            &vec!["linux-64.bat".to_string()],
            "should have selected the activation from the [linux-64] section"
        );
    }

    #[test]
    pub fn test_pypi_options_manifest() {
        let manifest = WorkspaceManifest::from_toml_str(
            r#"
        [project]
        name = "foo"
        platforms = ["linux-64", "osx-64", "win-64"]
        channels = []

        [project.pypi-options]
        index-url = "https://pypi.org/simple"

        [pypi-options]
        extra-index-urls = ["https://mypypi.org/simple"]
        "#,
        )
        .unwrap();

        // This behavior has changed from >0.22.0
        // and should now be none, previously this was added
        // to the default feature
        assert!(manifest.default_feature().pypi_options().is_some());
        assert!(manifest.workspace.pypi_options.is_some());
    }
}
