use crate::{
    CondaConstraints, EnvironmentName, SpecType, WorkspaceTarget, channel::PrioritizedChannel,
    consts, pypi::pypi_options::PypiOptions, target::Targets, workspace::ChannelPriority,
    workspace::SolveStrategy,
};
use crate::{InlinePackageManifest, PixiPlatform, PixiPlatformName};
use indexmap::{IndexMap, IndexSet};
use pixi_pypi_spec::{PixiPypiSpec, PypiPackageName};
use pixi_spec::PixiSpec;
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::PackageName;
use serde::{Deserialize, Serialize};
use std::ops::Not;
use std::{borrow::Cow, collections::HashSet, fmt, hash::Hash, str::FromStr};

/// The name of a feature. This is either a name or default for the default
/// feature.
#[derive(Clone, Debug, Default, Eq, PartialEq, PartialOrd, Ord, Hash)]
pub enum FeatureName {
    #[default]
    Default,
    Named(String),
    /// The implicit feature that is synthesized for an environment which
    /// defines feature content (like dependencies) inline. It is displayed
    /// with the reserved `env:` prefix, e.g. `env:dev` for the environment
    /// `dev`.
    Environment(EnvironmentName),
}

impl Serialize for FeatureName {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
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
        FeatureName::from(value.to_owned())
    }
}

impl From<String> for FeatureName {
    fn from(value: String) -> Self {
        if value == consts::DEFAULT_FEATURE_NAME {
            FeatureName::Default
        } else if let Some(environment_name) = value
            .strip_prefix(consts::ENVIRONMENT_FEATURE_PREFIX)
            .and_then(|name| EnvironmentName::from_str(name).ok())
        {
            FeatureName::Environment(environment_name)
        } else {
            FeatureName::Named(value)
        }
    }
}

impl FeatureName {
    /// Constructs the implicit feature name for the inline content of an
    /// environment, e.g. `env:dev` for the environment `dev`.
    pub fn environment(name: &EnvironmentName) -> Self {
        FeatureName::Environment(name.clone())
    }

    /// Returns the string representation of the feature.
    ///
    /// For an environment feature this is the bare environment name; use the
    /// [`fmt::Display`] implementation to get the canonical representation
    /// including the `env:` prefix.
    pub fn as_str(&self) -> &str {
        match self {
            FeatureName::Default => consts::DEFAULT_FEATURE_NAME,
            FeatureName::Named(name) => name,
            FeatureName::Environment(name) => name.as_str(),
        }
    }

    /// Returns true if the feature is the default feature.
    pub fn is_default(&self) -> bool {
        matches!(self, FeatureName::Default)
    }

    /// Returns true if this is an implicit feature synthesized for an
    /// environment that defines feature content inline.
    pub fn is_environment(&self) -> bool {
        matches!(self, FeatureName::Environment(_))
    }

    /// Returns the name of the environment this feature was synthesized for, if
    /// it is an environment feature.
    pub fn environment_name(&self) -> Option<&EnvironmentName> {
        match self {
            FeatureName::Environment(name) => Some(name),
            _ => None,
        }
    }

    /// Returns the name of the feature if it is not default.
    pub fn non_default(&self) -> Option<&str> {
        self.is_default().not().then(|| self.as_str())
    }

    /// Renders the feature for user-facing diagnostics, describing an
    /// environment feature as `environment '<name>'` and any other feature as
    /// `feature '<name>'`.
    pub fn user_facing(&self) -> UserFacingFeatureName<'_> {
        UserFacingFeatureName(self)
    }
}

/// Helper returned by [`FeatureName::user_facing`] that renders a feature name
/// for diagnostics without exposing the internal `env:` prefix.
pub struct UserFacingFeatureName<'a>(&'a FeatureName);

impl fmt::Display for UserFacingFeatureName<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            FeatureName::Environment(name) => write!(f, "environment '{name}'"),
            _ => write!(f, "feature '{}'", self.0.as_str()),
        }
    }
}

impl PartialEq<str> for FeatureName {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for FeatureName {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl FromStr for FeatureName {
    type Err = ParseFeatureNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.starts_with(consts::ENVIRONMENT_FEATURE_PREFIX) {
            return Err(ParseFeatureNameError);
        }
        Ok(FeatureName::from(s))
    }
}

/// Error returned when parsing a [`FeatureName`] from user input that uses the
/// reserved `env:` prefix.
#[derive(Debug, Clone, thiserror::Error)]
#[error(
    "feature names starting with '{}' are reserved for environments that define their content inline",
    consts::ENVIRONMENT_FEATURE_PREFIX
)]
pub struct ParseFeatureNameError;

impl From<FeatureName> for String {
    fn from(name: FeatureName) -> Self {
        match name {
            FeatureName::Default => consts::DEFAULT_FEATURE_NAME.to_owned(),
            FeatureName::Named(name) => name,
            FeatureName::Environment(_) => name.to_string(),
        }
    }
}
impl<'a> From<&'a FeatureName> for String {
    fn from(name: &'a FeatureName) -> Self {
        name.to_string()
    }
}
impl fmt::Display for FeatureName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FeatureName::Environment(name) => {
                write!(f, "{}{}", consts::ENVIRONMENT_FEATURE_PREFIX, name.as_str())
            }
            _ => write!(f, "{}", self.as_str()),
        }
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
    pub platforms: Option<IndexSet<PixiPlatformName>>,

    /// Channels specific to this feature.
    ///
    /// This value is `None` if this feature does not specify any channels and
    /// the default channels from the project should be used.
    pub channels: Option<IndexSet<PrioritizedChannel>>,

    /// Channel priority for the solver, if not set the default is used.
    /// This value is `None` and there are multiple features,
    /// it will be seen as unset and overwritten by a set one.
    pub channel_priority: Option<ChannelPriority>,

    /// Solve strategy specific for this feature.
    ///
    /// If this value is `None` and there are multiple features,
    /// it will be seen as unset and overwritten by a set one.
    pub solve_strategy: Option<SolveStrategy>,

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
            solve_strategy: None,
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
    pub fn platforms_mut(&mut self) -> &mut IndexSet<PixiPlatformName> {
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
    pub fn run_dependencies<'a>(
        &'a self,
        platform: Option<&'a PixiPlatform>,
    ) -> Option<Cow<'a, DependencyMap<PackageName, PixiSpec>>> {
        self.dependencies(SpecType::Run, platform)
    }

    /// Returns the host dependencies of the target for the given `platform`.
    ///
    /// If the platform is `None` no platform specific dependencies are
    /// returned.
    ///
    /// This function returns `None` if there is not a single feature that has
    /// any dependencies defined.
    pub fn host_dependencies<'a>(
        &'a self,
        platform: Option<&'a PixiPlatform>,
    ) -> Option<Cow<'a, DependencyMap<PackageName, PixiSpec>>> {
        self.dependencies(SpecType::Host, platform)
    }

    /// Returns the run dependencies of the target for the given `platform`.
    ///
    /// If the platform is `None` no platform specific dependencies are
    /// returned.
    ///
    /// This function returns `None` if there is not a single feature that has
    /// any dependencies defined.
    pub fn build_dependencies<'a>(
        &'a self,
        platform: Option<&'a PixiPlatform>,
    ) -> Option<Cow<'a, DependencyMap<PackageName, PixiSpec>>> {
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
    pub fn dependencies<'a>(
        &'a self,
        spec_type: SpecType,
        platform: Option<&'a PixiPlatform>,
    ) -> Option<Cow<'a, DependencyMap<PackageName, PixiSpec>>> {
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
    pub fn combined_dependencies<'a>(
        &'a self,
        platform: Option<&'a PixiPlatform>,
    ) -> Option<Cow<'a, DependencyMap<PackageName, PixiSpec>>> {
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
    pub fn pypi_dependencies<'a>(
        &'a self,
        platform: Option<&'a PixiPlatform>,
    ) -> Option<Cow<'a, DependencyMap<PypiPackageName, PixiPypiSpec>>> {
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
    pub fn activation_scripts<'a>(
        &'a self,
        platform: Option<&'a PixiPlatform>,
    ) -> Option<&'a Vec<String>> {
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
    pub fn activation_env<'a>(
        &'a self,
        platform: Option<&'a PixiPlatform>,
    ) -> IndexMap<String, String> {
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

    /// Returns true if the feature supports the given platform.
    ///
    /// A feature supports a platform if it has no platform restriction or if
    /// its `platforms` set contains the given platform. If `platform` is
    /// `None`, the feature is always considered supported.
    pub fn supports_platform<'a>(&'a self, platform: Option<&'a PixiPlatform>) -> bool {
        match (&self.platforms, platform) {
            (Some(platforms), Some(p)) => platforms.iter().any(|name| p.matches_reference(name)),
            _ => true,
        }
    }

    /// Returns the dev dependencies of the feature for a given `platform`.
    ///
    /// Dev dependencies are source packages whose build/host/run dependencies
    /// should be installed without building the packages themselves.
    ///
    /// This function returns a [`Cow`]. If the dependencies are not combined or
    /// overwritten by multiple targets than this function returns a
    /// reference to the internal dependencies.
    ///
    /// Returns `None` if this feature does not define any target that has any
    /// of the requested dev dependencies.
    ///
    /// If the `platform` is `None` no platform specific dependencies are taken
    /// into consideration.
    pub fn dev_dependencies<'a>(
        &'a self,
        platform: Option<&'a PixiPlatform>,
    ) -> Option<Cow<'a, DependencyMap<PackageName, pixi_spec::SourceLocationSpec>>> {
        self.targets
            .resolve(platform)
            // Get the targets in reverse order, from least specific to most specific.
            // This is required because we want more specific targets to overwrite their specs.
            .rev()
            .filter_map(|t| t.dev_dependencies.as_ref())
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

    /// Returns the inline package definitions of the feature for a given
    /// `platform`.
    ///
    /// The most specific target that declares a package as a dependency decides
    /// whether it carries an inline definition. A less specific target's inline
    /// definition must not leak onto a package that a more specific target
    /// already declares without one, so a plain (non-inline) declaration in a
    /// more specific target suppresses an inline definition from a less specific
    /// one.
    pub fn inline_packages<'a>(
        &'a self,
        platform: Option<&'a PixiPlatform>,
    ) -> IndexMap<PackageName, &'a InlinePackageManifest> {
        let mut result = IndexMap::new();
        let mut decided: HashSet<PackageName> = HashSet::new();
        // `resolve` yields targets from most to least specific.
        for target in self.targets.resolve(platform) {
            let Some(dependencies) = target.combined_dependencies() else {
                continue;
            };
            for name in dependencies.names() {
                // The first (most specific) target to declare the package wins;
                // its inline definition (or absence of one) is final.
                if decided.insert(name.clone())
                    && let Some(manifest) = target.inline_packages.get(name)
                {
                    result.insert(name.clone(), manifest);
                }
            }
        }
        result
    }

    /// Returns the version constraints of the feature for a given `platform`.
    ///
    /// Constraints limit the versions of packages that can be installed
    /// without explicitly requiring them to be installed.
    ///
    /// This function returns a [`Cow`]. If the constraints are not combined or
    /// overwritten by multiple targets than this function returns a
    /// reference to the internal constraints.
    ///
    /// Returns `None` if this feature does not define any target that has any
    /// constraints.
    ///
    /// If the `platform` is `None` no platform specific constraints are taken
    /// into consideration.
    pub fn constraints<'a>(
        &'a self,
        platform: Option<&'a PixiPlatform>,
    ) -> Option<Cow<'a, CondaConstraints>> {
        self.targets
            .resolve(platform)
            // Get the targets in reverse order, from least specific to most specific.
            // This is required because we want more specific targets to overwrite their specs.
            .rev()
            .filter_map(|t| t.constraints.as_ref())
            .filter(|constraints| !constraints.is_empty())
            .fold(None, |acc, constraints| match acc {
                None => Some(Cow::Borrowed(constraints)),
                Some(acc) => {
                    // Overwrite the accumulator with specs from this target
                    // More specific targets (processed later) overwrite less specific ones
                    Some(Cow::Owned(acc.as_ref().overwrite(constraints)))
                }
            })
    }
}

#[cfg(test)]
mod tests {

    use std::path::Path;

    use assert_matches::assert_matches;
    use rattler_conda_types::Platform;

    use super::*;
    use crate::WorkspaceManifest;

    #[test]
    fn test_environment_feature_name() {
        let environment = EnvironmentName::Named("dev".to_string());
        let name = FeatureName::environment(&environment);
        assert_eq!(name.to_string(), "env:dev");
        assert_eq!(name.as_str(), "dev");
        assert!(name.is_environment());
        assert!(!name.is_default());
        assert_eq!(name.environment_name(), Some(&environment));
        assert_eq!(name.user_facing().to_string(), "environment 'dev'");

        // The canonical representation round-trips through `From<String>`.
        assert_eq!(FeatureName::from(name.to_string()), name);
        // User input cannot use the reserved prefix.
        assert!("env:dev".parse::<FeatureName>().is_err());
    }

    #[test]
    fn test_regular_feature_name_is_not_environment() {
        let name = FeatureName::from("dev");
        assert!(!name.is_environment());
        assert_eq!(name.environment_name(), None);
        assert_eq!(name.user_facing().to_string(), "feature 'dev'");

        // A feature whose name merely starts with `env` (but not `env:`) is a
        // normal feature.
        let name = FeatureName::from("environment");
        assert!(!name.is_environment());
        assert_eq!(name.environment_name(), None);
    }

    #[test]
    fn test_dependencies_borrowed() {
        let manifest = WorkspaceManifest::from_toml_str_with_base_dir(
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
            Path::new(""),
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
        let manifest = WorkspaceManifest::from_toml_str_with_base_dir(
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
            Path::new(""),
        )
        .unwrap();

        assert_eq!(
            manifest.default_feature().activation_scripts(None).unwrap(),
            &vec!["run.bat".to_string()],
            "should have selected the activation from the [activation] section"
        );
        let linux64 = PixiPlatform::from_subdir(Platform::Linux64);
        assert_eq!(
            manifest
                .default_feature()
                .activation_scripts(Some(&linux64))
                .unwrap(),
            &vec!["linux-64.bat".to_string()],
            "should have selected the activation from the [linux-64] section"
        );
    }

    #[test]
    pub fn test_pypi_options_manifest() {
        let manifest = WorkspaceManifest::from_toml_str_with_base_dir(
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
            Path::new(""),
        )
        .unwrap();

        // This behavior has changed from >0.22.0
        // and should now be none, previously this was added
        // to the default feature
        assert!(manifest.default_feature().pypi_options().is_some());
        assert!(manifest.workspace.pypi_options.is_some());
    }
}
