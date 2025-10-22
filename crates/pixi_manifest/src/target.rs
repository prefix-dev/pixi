use std::{borrow::Cow, collections::HashMap, str::FromStr};

use indexmap::{IndexMap, map::Entry};
use itertools::Either;
use pixi_spec::PixiSpec;
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::{PackageName, ParsePlatformError, Platform};

use super::error::DependencyError;
use crate::{
    CondaDependencies, DependencyOverwriteBehavior, InternalDependencyBehavior, PyPiDependencies,
    SpecType,
    activation::Activation,
    task::{Task, TaskName},
    utils::PixiSpanned,
};
use pixi_pypi_spec::{PixiPypiSpec, PypiPackageName};

/// A workspace target describes the dependencies, activations and task
/// available to a specific feature, in a specific environment, and optionally
/// for a specific platform.
#[derive(Default, Debug, Clone)]
pub struct WorkspaceTarget {
    /// Dependencies for this target.
    ///
    /// TODO: While the pixi-build feature is not stabilized yet, a workspace
    /// can have host- and build dependencies. When pixi-build is stabilized, we
    /// can simplify this part of the code.
    pub dependencies: HashMap<SpecType, CondaDependencies>,

    /// Specific python dependencies
    pub pypi_dependencies: Option<PyPiDependencies>,

    /// Additional information to activate an environment.
    pub activation: Option<Activation>,

    /// Target specific tasks to run in the environment
    pub tasks: HashMap<TaskName, Task>,
}

/// A package target describes the dependencies for a specific platform.
#[derive(Default, Debug, Clone)]
pub struct PackageTarget {
    /// Dependencies for this target.
    pub dependencies: HashMap<SpecType, DependencyMap<PackageName, PixiSpec>>,
}

impl WorkspaceTarget {
    /// Returns the run dependencies of the target
    pub fn run_dependencies(&self) -> Option<&DependencyMap<PackageName, PixiSpec>> {
        self.dependencies.get(&SpecType::Run)
    }

    /// Returns the host dependencies of the target
    pub fn host_dependencies(&self) -> Option<&DependencyMap<PackageName, PixiSpec>> {
        self.dependencies.get(&SpecType::Host)
    }

    /// Returns the build dependencies of the target
    pub fn build_dependencies(&self) -> Option<&DependencyMap<PackageName, PixiSpec>> {
        self.dependencies.get(&SpecType::Build)
    }

    /// Returns the dependencies of a certain type.
    pub fn dependencies(
        &self,
        spec_type: SpecType,
    ) -> Option<&DependencyMap<PackageName, PixiSpec>> {
        self.dependencies.get(&spec_type)
    }

    /// Determines the combined set of dependencies.
    ///
    /// The `build` dependencies overwrite the `host` dependencies which
    /// overwrite the `run` dependencies.
    ///
    /// This function returns `None` if no dependencies are specified for the
    /// given `spec_type`.
    ///
    /// This function returns a `Cow` to avoid cloning the dependencies if they
    /// can be returned directly from the underlying map.
    pub fn combined_dependencies(&self) -> Option<Cow<'_, DependencyMap<PackageName, PixiSpec>>> {
        let mut first_deps: Option<&DependencyMap<PackageName, PixiSpec>> = None;
        let mut count = 0;

        // Count and find the first non-empty spec type
        for spec_type in [SpecType::Run, SpecType::Host, SpecType::Build] {
            if let Some(specs) = self.dependencies.get(&spec_type) {
                if !specs.is_empty() {
                    if first_deps.is_none() {
                        first_deps = Some(specs);
                    }
                    count += 1;
                }
            }
        }

        // If there's only one non-empty spec type, return a borrowed reference
        if count == 1 {
            return first_deps.map(Cow::Borrowed);
        }

        // If there are multiple spec types, combine them
        let mut all_deps: Option<DependencyMap<PackageName, PixiSpec>> = None;
        for spec_type in [SpecType::Run, SpecType::Host, SpecType::Build] {
            let Some(specs) = self.dependencies.get(&spec_type) else {
                continue;
            };
            if specs.is_empty() {
                continue;
            }

            all_deps = match all_deps {
                None => Some(specs.clone()),
                Some(mut all_deps) => {
                    all_deps = all_deps.overwrite(specs);
                    Some(all_deps)
                }
            }
        }
        all_deps.map(Cow::Owned)
    }

    /// Checks if this target contains a dependency
    pub fn has_dependency(
        &self,
        dep_name: &PackageName,
        spec_type: SpecType,
        exact: Option<&PixiSpec>,
    ) -> bool {
        let current_dependencies = self
            .dependencies(spec_type)
            .and_then(|deps| deps.get(dep_name));

        match (current_dependencies, exact) {
            (Some(specs), Some(spec)) => specs.contains(spec),
            (Some(_), None) => true,
            (None, _) => false,
        }
    }

    /// Removes a dependency from this target
    ///
    /// it will Err if the dependency is not found
    pub fn remove_dependency(
        &mut self,
        dep_name: &PackageName,
        spec_type: SpecType,
    ) -> Result<(PackageName, PixiSpec), DependencyError> {
        let Some(dependencies) = self.dependencies.get_mut(&spec_type) else {
            return Err(DependencyError::NoSpecType(spec_type.name().into()));
        };
        let (name, mut specs) = dependencies
            .remove(dep_name)
            .ok_or_else(|| DependencyError::NoDependency(dep_name.as_normalized().into()))?;

        // Return the first (and typically only) spec
        let spec = specs
            .pop()
            .expect("DependencyMap should not contain empty IndexSets");
        Ok((name, spec))
    }

    /// Adds a dependency to a target
    pub(crate) fn add_dependency(
        &mut self,
        dep_name: &PackageName,
        spec: &PixiSpec,
        spec_type: SpecType,
        behavior: InternalDependencyBehavior,
    ) {
        let deps = self.dependencies.entry(spec_type).or_default();
        match behavior {
            InternalDependencyBehavior::Append => {
                // Append to existing specs
                deps.insert(dep_name.clone(), spec.clone());
            }
            InternalDependencyBehavior::Overwrite => {
                // Overwrite any existing spec with the new one
                deps.insert_overwrite(dep_name.clone(), spec.clone());
            }
        }
    }

    /// Adds a dependency to a target
    ///
    /// What happens with the dependency depends on the [`DependencyOverwriteBehavior`]
    pub fn try_add_dependency(
        &mut self,
        dep_name: &PackageName,
        spec: &PixiSpec,
        spec_type: SpecType,
        dependency_overwrite_behavior: DependencyOverwriteBehavior,
    ) -> Result<bool, DependencyError> {
        if self.has_dependency(dep_name, spec_type, None) {
            match dependency_overwrite_behavior {
                DependencyOverwriteBehavior::OverwriteIfExplicit => {
                    if !spec.has_version_spec() {
                        return Ok(false);
                    }
                }
                DependencyOverwriteBehavior::IgnoreDuplicate => return Ok(false),
                DependencyOverwriteBehavior::Error => {
                    return Err(DependencyError::Duplicate(dep_name.as_normalized().into()));
                }
                DependencyOverwriteBehavior::Overwrite => {}
            }
        }
        // Convert public behavior to internal behavior
        self.add_dependency(
            dep_name,
            spec,
            spec_type,
            dependency_overwrite_behavior.into(),
        );
        Ok(true)
    }

    /// Checks if this target contains a specific pypi dependency
    pub fn has_pypi_dependency(&self, requirement: &pep508_rs::Requirement, exact: bool) -> bool {
        let current_requirements = self
            .pypi_dependencies
            .as_ref()
            .and_then(|deps| deps.get(&PypiPackageName::from_normalized(requirement.name.clone())));

        match (current_requirements, exact) {
            (Some(specs), true) => {
                // TODO: would be nice to compare pep508 == PyPiRequirement directly
                let target_spec = PixiPypiSpec::try_from(requirement.clone())
                    .expect("could not convert pep508 requirement");
                specs.contains(&target_spec)
            }
            (Some(specs), false) => {
                // Check if any spec matches the extras
                specs.iter().any(|r| r.extras() == requirement.extras)
            }
            (None, _) => false,
        }
    }

    /// Removes a pypi dependency from this target
    ///
    /// it will Err if the dependency is not found
    ///
    /// Note: This removes all specs for the given package name and returns one of them.
    pub fn remove_pypi_dependency(
        &mut self,
        dep_name: &PypiPackageName,
    ) -> Result<(PypiPackageName, PixiPypiSpec), DependencyError> {
        let Some(pypi_dependencies) = self.pypi_dependencies.as_mut() else {
            return Err(DependencyError::NoPyPiDependencies);
        };
        let (name, mut specs) = pypi_dependencies
            .remove(dep_name)
            .ok_or_else(|| DependencyError::NoDependency(dep_name.as_source().into()))?;

        // Return the first (and typically only) spec
        let spec = specs
            .pop()
            .expect("DependencyMap should not contain empty IndexSets");
        Ok((name, spec))
    }

    /// Adds a pypi dependency to a target
    pub(crate) fn add_pypi_dependency(
        &mut self,
        name: PypiPackageName,
        requirement: PixiPypiSpec,
        behavior: InternalDependencyBehavior,
    ) {
        let deps = self.pypi_dependencies.get_or_insert_with(Default::default);
        match behavior {
            InternalDependencyBehavior::Append => {
                // Append to existing specs
                deps.insert(name, requirement);
            }
            InternalDependencyBehavior::Overwrite => {
                // Overwrite any existing spec with the new one
                deps.insert_overwrite(name, requirement);
            }
        }
    }

    /// Adds a pypi dependency to a target
    ///
    /// What happens when a dependency exists depends on the [`DependencyOverwriteBehavior`]
    pub(crate) fn try_add_pep508_dependency(
        &mut self,
        requirement: &pep508_rs::Requirement,
        editable: Option<bool>,
        dependency_overwrite_behavior: DependencyOverwriteBehavior,
    ) -> Result<bool, DependencyError> {
        if self.has_pypi_dependency(requirement, false) {
            match dependency_overwrite_behavior {
                DependencyOverwriteBehavior::OverwriteIfExplicit => {
                    if requirement.version_or_url.is_none() {
                        return Ok(false);
                    }
                }
                DependencyOverwriteBehavior::IgnoreDuplicate => return Ok(false),
                DependencyOverwriteBehavior::Error => {
                    return Err(DependencyError::Duplicate(requirement.name.to_string()));
                }
                DependencyOverwriteBehavior::Overwrite => {}
            }
        }

        // Convert to an internal representation
        let name = PypiPackageName::from_normalized(requirement.name.clone());
        let mut requirement = PixiPypiSpec::try_from(requirement.clone()).map_err(Box::new)?;
        if let Some(editable) = editable {
            requirement.set_editable(editable);
        }

        // Convert public behavior to internal behavior
        self.add_pypi_dependency(name, requirement, dependency_overwrite_behavior.into());
        Ok(true)
    }
}

impl PackageTarget {
    /// Returns the dependencies of a certain type.
    pub fn dependencies(
        &self,
        spec_type: SpecType,
    ) -> Option<&DependencyMap<PackageName, PixiSpec>> {
        self.dependencies.get(&spec_type)
    }

    /// Returns the run dependencies of the target
    pub fn run_dependencies(&self) -> Option<&DependencyMap<PackageName, PixiSpec>> {
        self.dependencies.get(&SpecType::Run)
    }

    /// Returns the host dependencies of the target
    pub fn host_dependencies(&self) -> Option<&DependencyMap<PackageName, PixiSpec>> {
        self.dependencies.get(&SpecType::Host)
    }

    /// Returns the build dependencies of the target
    pub fn build_dependencies(&self) -> Option<&DependencyMap<PackageName, PixiSpec>> {
        self.dependencies.get(&SpecType::Build)
    }

    /// Checks if this target contains a dependency
    pub fn has_dependency(
        &self,
        dep_name: &PackageName,
        spec_type: SpecType,
        exact: Option<&PixiSpec>,
    ) -> bool {
        let current_dependencies = self
            .dependencies(spec_type)
            .and_then(|deps| deps.get(dep_name));

        match (current_dependencies, exact) {
            (Some(specs), Some(spec)) => specs.contains(spec),
            (Some(_), None) => true,
            (None, _) => false,
        }
    }

    /// Adds a dependency to a target
    ///
    /// This will overwrite any existing dependency of the same name by first removing
    /// any existing specs and then inserting the new one.
    pub(crate) fn add_dependency(
        &mut self,
        dep_name: &PackageName,
        spec: &PixiSpec,
        spec_type: SpecType,
        behavior: InternalDependencyBehavior,
    ) {
        let deps = self.dependencies.entry(spec_type).or_default();
        match behavior {
            InternalDependencyBehavior::Append => {
                // Append to existing specs
                deps.insert(dep_name.clone(), spec.clone());
            }
            InternalDependencyBehavior::Overwrite => {
                // Overwrite any existing spec with the new one
                deps.insert_overwrite(dep_name.clone(), spec.clone());
            }
        }
    }

    /// Adds a dependency to a target with public behavior
    ///
    /// This is similar to `add_dependency` but accepts the public `DependencyOverwriteBehavior`
    /// and is used by code that needs to handle various overwrite behaviors.
    pub fn try_add_dependency(
        &mut self,
        dep_name: &PackageName,
        spec: &PixiSpec,
        spec_type: SpecType,
        dependency_overwrite_behavior: DependencyOverwriteBehavior,
    ) -> Result<bool, DependencyError> {
        if self.has_dependency(dep_name, spec_type, None) {
            match dependency_overwrite_behavior {
                DependencyOverwriteBehavior::OverwriteIfExplicit => {
                    if !spec.has_version_spec() {
                        return Ok(false);
                    }
                }
                DependencyOverwriteBehavior::IgnoreDuplicate => return Ok(false),
                DependencyOverwriteBehavior::Error => {
                    return Err(DependencyError::Duplicate(dep_name.as_normalized().into()));
                }
                DependencyOverwriteBehavior::Overwrite => {}
            }
        }
        // Convert public behavior to internal behavior
        self.add_dependency(
            dep_name,
            spec,
            spec_type,
            dependency_overwrite_behavior.into(),
        );
        Ok(true)
    }
}

/// Represents a target selector. Currently we only support explicit platform
/// selection.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum TargetSelector {
    // Platform specific configuration
    Platform(Platform),
    Unix,
    Linux,
    Win,
    MacOs,
    // TODO: Add minijinja coolness here.
}

impl TargetSelector {
    /// Returns true if this selector matches the given platform.
    pub fn matches(&self, platform: Platform) -> bool {
        match self {
            TargetSelector::Platform(p) => p == &platform,
            TargetSelector::Linux => platform.is_linux(),
            TargetSelector::Unix => platform.is_unix(),
            TargetSelector::Win => platform.is_windows(),
            TargetSelector::MacOs => platform.is_osx(),
        }
    }
}

impl std::fmt::Display for TargetSelector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TargetSelector::Platform(p) => write!(f, "{p}"),
            TargetSelector::Linux => write!(f, "linux"),
            TargetSelector::Unix => write!(f, "unix"),
            TargetSelector::Win => write!(f, "win"),
            TargetSelector::MacOs => write!(f, "osx"),
        }
    }
}

impl From<Platform> for TargetSelector {
    fn from(value: Platform) -> Self {
        TargetSelector::Platform(value)
    }
}

impl FromStr for TargetSelector {
    type Err = ParsePlatformError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "linux" => Ok(TargetSelector::Linux),
            "unix" => Ok(TargetSelector::Unix),
            "win" => Ok(TargetSelector::Win),
            "osx" => Ok(TargetSelector::MacOs),
            _ => Platform::from_str(s).map(TargetSelector::Platform),
        }
    }
}

/// A collect of targets including a default target.
#[derive(Debug, Clone, Default)]
pub struct Targets<T> {
    default_target: T,

    /// We use an [`IndexMap`] to preserve the order in which the items where
    /// defined in the manifest.
    targets: IndexMap<TargetSelector, T>,

    /// The source location of the target selector in the manifest.
    source_locs: HashMap<TargetSelector, std::ops::Range<usize>>,
}

impl<T> Targets<T> {
    /// Constructs a new [`Targets`] from a default target and additional user
    /// defined targets.
    pub fn from_default_and_user_defined(
        default_target: T,
        user_defined_targets: IndexMap<PixiSpanned<TargetSelector>, T>,
    ) -> Self {
        let mut targets = IndexMap::with_capacity(user_defined_targets.len());
        let mut source_locs = HashMap::with_capacity(user_defined_targets.len());
        for (selector, target) in user_defined_targets {
            targets.insert(selector.value.clone(), target);
            if let Some(span) = selector.span {
                source_locs.insert(selector.value, span);
            }
        }

        Self {
            default_target,
            targets,
            source_locs,
        }
    }

    /// Returns the default target.
    pub fn default(&self) -> &T {
        &self.default_target
    }

    /// Returns the default target
    pub fn default_mut(&mut self) -> &mut T {
        &mut self.default_target
    }

    /// Returns all the targets that apply for the given platform. If no
    /// platform is specified, only the default target is returned.
    ///
    /// Multiple selectors might match for a given platform. This function
    /// returns all of them in order, with the most specific selector first
    /// and the default target last.
    ///
    /// This also always includes the default target.
    pub fn resolve(
        &self,
        platform: Option<Platform>,
    ) -> impl DoubleEndedIterator<Item = &'_ T> + '_ {
        if let Some(platform) = platform {
            Either::Left(self.resolve_for_platform(platform))
        } else {
            Either::Right(std::iter::once(&self.default_target))
        }
    }

    /// Returns all the targets that apply for the given platform.
    ///
    /// Multiple selectors might match for a given platform. This function
    /// returns all of them in order, with the most specific selector first
    /// and the default target last.
    ///
    /// This also always includes the default target.
    ///
    /// You should use the [`Self::resolve`] function.
    fn resolve_for_platform(
        &self,
        platform: Platform,
    ) -> impl DoubleEndedIterator<Item = &'_ T> + '_ {
        std::iter::once(&self.default_target)
            .chain(self.targets.iter().filter_map(move |(selector, target)| {
                if selector.matches(platform) {
                    Some(target)
                } else {
                    None
                }
            }))
            // We reverse this to get the most specific selector first.
            .rev()
    }

    /// Returns the target for the given target selector.
    pub fn for_target(&self, target: &TargetSelector) -> Option<&T> {
        self.targets.get(target)
    }

    /// Returns the target for the given target selector or the default target
    /// if the selector is `None`.
    pub fn for_opt_target(&self, target: Option<&TargetSelector>) -> Option<&T> {
        if let Some(sel) = target {
            self.targets.get(sel)
        } else {
            Some(&self.default_target)
        }
    }

    /// Returns the target for the given target selector or the default target
    /// if no target is specified.
    pub fn for_opt_target_mut(&mut self, target: Option<&TargetSelector>) -> Option<&mut T> {
        if let Some(sel) = target {
            self.targets.get_mut(sel)
        } else {
            Some(&mut self.default_target)
        }
    }

    /// Returns the target for the given target selector or the default target
    /// if no target is specified.
    ///
    /// If a target is specified and it does not exist the default target is
    /// returned instead.
    pub fn for_opt_target_or_default(&self, target: Option<&TargetSelector>) -> &T {
        if let Some(sel) = target {
            self.targets.get(sel).unwrap_or(&self.default_target)
        } else {
            &self.default_target
        }
    }

    /// Returns a mutable reference to the target for the given target selector
    /// or the default target if no target is specified.
    ///
    /// If a target is specified and it does not exist, it will be created.
    pub fn for_opt_target_or_default_mut(&mut self, target: Option<&TargetSelector>) -> &mut T
    where
        T: Default,
    {
        if let Some(sel) = target {
            self.targets.entry(sel.clone()).or_default()
        } else {
            &mut self.default_target
        }
    }

    /// Returns the target for the given target selector.
    pub fn target_entry(&mut self, selector: TargetSelector) -> Entry<'_, TargetSelector, T> {
        self.targets.entry(selector)
    }

    /// Returns an iterator over all targets and selectors.
    pub fn iter(&self) -> impl Iterator<Item = (&'_ T, Option<&'_ TargetSelector>)> + '_ {
        std::iter::once((&self.default_target, None))
            .chain(self.targets.iter().map(|(sel, target)| (target, Some(sel))))
    }

    /// Returns an iterator over all targets.
    pub fn targets(&self) -> impl Iterator<Item = &'_ T> + '_ {
        std::iter::once(&self.default_target).chain(self.targets.iter().map(|(_, target)| target))
    }

    /// Returns user defined target selectors
    pub fn user_defined_selectors(&self) -> impl Iterator<Item = &TargetSelector> + '_ {
        self.targets.keys()
    }

    /// Returns the user defined selectors and their targets
    pub fn user_defined_targets(&self) -> impl Iterator<Item = (&TargetSelector, &T)> + '_ {
        self.targets.iter()
    }

    /// Returns the source location of the target selector in the manifest.
    pub fn source_loc(&self, selector: &TargetSelector) -> Option<std::ops::Range<usize>> {
        self.source_locs.get(selector).cloned()
    }
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;
    use itertools::Itertools;
    use pixi_spec::PixiSpec;
    use rattler_conda_types::{PackageName, VersionSpec};
    use std::str::FromStr;

    use crate::{DependencyOverwriteBehavior, FeatureName, SpecType, WorkspaceManifest};

    #[test]
    fn test_targets_overwrite_order() {
        let manifest = WorkspaceManifest::from_toml_str(
            r#"
        [project]
        name = "test"
        channels = []
        platforms = []

        [dependencies]
        run = "1.0"

        [build-dependencies]
        run = "2.0"
        host = "2.0"
        build = "1.0"

        [host-dependencies]
        run = "3.0"
        host = "1.0"
        "#,
        )
        .unwrap();

        assert_snapshot!(manifest
            .default_feature()
            .targets
            .default()
            .combined_dependencies()
            .unwrap_or_default()
            .iter()
            .filter_map(|(name, specs)| {
                specs.iter().next().map(|spec| {
                    format!("{} = {}", name.as_source(), spec.as_version_spec().unwrap())
                })
            })
            .join("\n"), @r###"
        run = ==2.0
        host = ==2.0
        build = ==1.0
        "###);
    }

    /// Test that Overwrite behavior replaces existing dependencies (regression test)
    /// This ensures that the default behavior preserves backward compatibility
    #[test]
    fn test_overwrite_behavior_regression() {
        use crate::{ManifestDocument, WorkspaceManifest, WorkspaceManifestMut};

        let manifest_content = r#"
        [project]
        name = "test"
        channels = []
        platforms = []

        [dependencies]
        foo = "1.0"
        "#;

        let mut manifest = WorkspaceManifest::from_toml_str(manifest_content).unwrap();
        let mut document = ManifestDocument::empty_pixi();

        // Create a mutable context
        let mut manifest_mut = WorkspaceManifestMut {
            workspace: &mut manifest,
            document: &mut document,
        };

        // Add foo = "==2.0" with Overwrite behavior
        let foo = PackageName::from_str("foo").unwrap();
        let spec = PixiSpec::Version(
            VersionSpec::from_str("==2.0", rattler_conda_types::ParseStrictness::Strict).unwrap(),
        );

        manifest_mut
            .add_dependency(
                &foo,
                &spec,
                SpecType::Run,
                &[],
                &FeatureName::default(),
                DependencyOverwriteBehavior::Overwrite,
            )
            .unwrap();

        // Verify the TOML output has only one foo dependency with version 2.0
        assert_snapshot!(manifest_mut.document.to_string(), @r###"
        [project]
        name = "test"
        channels = []
        platforms = []

        [dependencies]
        foo = "==2.0"
        "###);
    }

    /// Test that adding multiple dependencies with Overwrite keeps only the last one
    #[test]
    fn test_multiple_overwrite_keeps_last() {
        use crate::{ManifestDocument, WorkspaceManifest, WorkspaceManifestMut};

        let manifest_content = r#"
        [project]
        name = "test"
        channels = []
        platforms = []
        "#;

        let mut manifest = WorkspaceManifest::from_toml_str(manifest_content).unwrap();
        let mut document = ManifestDocument::empty_pixi();

        let mut manifest_mut = WorkspaceManifestMut {
            workspace: &mut manifest,
            document: &mut document,
        };

        let foo = PackageName::from_str("foo").unwrap();

        // Add foo = "==1.0"
        let spec1 = PixiSpec::Version(
            VersionSpec::from_str("==1.0", rattler_conda_types::ParseStrictness::Strict).unwrap(),
        );
        manifest_mut
            .add_dependency(
                &foo,
                &spec1,
                SpecType::Run,
                &[],
                &FeatureName::default(),
                DependencyOverwriteBehavior::Overwrite,
            )
            .unwrap();

        // Add foo = "==2.0" (should overwrite)
        let spec2 = PixiSpec::Version(
            VersionSpec::from_str("==2.0", rattler_conda_types::ParseStrictness::Strict).unwrap(),
        );
        manifest_mut
            .add_dependency(
                &foo,
                &spec2,
                SpecType::Run,
                &[],
                &FeatureName::default(),
                DependencyOverwriteBehavior::Overwrite,
            )
            .unwrap();

        // Add foo = "==3.0" (should overwrite again)
        let spec3 = PixiSpec::Version(
            VersionSpec::from_str("==3.0", rattler_conda_types::ParseStrictness::Strict).unwrap(),
        );
        manifest_mut
            .add_dependency(
                &foo,
                &spec3,
                SpecType::Run,
                &[],
                &FeatureName::default(),
                DependencyOverwriteBehavior::Overwrite,
            )
            .unwrap();

        // Verify only the last version (3.0) is in the TOML
        assert_snapshot!(manifest_mut.document.to_string(), @r###"
        [project]
        name = "test"
        channels = []
        platforms = []

        [dependencies]
        foo = "==3.0"
        "###);
    }

    /// Test IgnoreDuplicate behavior doesn't add when dependency exists
    #[test]
    fn test_ignore_duplicate_doesnt_add() {
        use crate::{ManifestDocument, WorkspaceManifest, WorkspaceManifestMut};

        let manifest_content = r#"
        [project]
        name = "test"
        channels = []
        platforms = []

        [dependencies]
        foo = "1.0"
        "#;

        let mut manifest = WorkspaceManifest::from_toml_str(manifest_content).unwrap();
        let mut document = ManifestDocument::empty_pixi();

        let mut manifest_mut = WorkspaceManifestMut {
            workspace: &mut manifest,
            document: &mut document,
        };

        // Try to add foo = "==2.0" with IgnoreDuplicate
        let foo = PackageName::from_str("foo").unwrap();
        let spec = PixiSpec::Version(
            VersionSpec::from_str("==2.0", rattler_conda_types::ParseStrictness::Strict).unwrap(),
        );

        let result = manifest_mut.add_dependency(
            &foo,
            &spec,
            SpecType::Run,
            &[],
            &FeatureName::default(),
            DependencyOverwriteBehavior::IgnoreDuplicate,
        );

        // Should return Ok(false) indicating nothing was added
        assert!(!result.unwrap());

        // Verify TOML still has original version
        assert_snapshot!(manifest_mut.document.to_string(), @r###"
        [project]
        name = "test"
        channels = []
        platforms = []
        "###);
    }

    /// Test that target-specific dependencies overwrite default feature dependencies
    /// This is a regression test for the issue where target dependencies were being
    /// merged instead of overwriting the default dependencies.
    #[test]
    fn test_target_specific_overrides_default() {
        use rattler_conda_types::Platform;

        let manifest = WorkspaceManifest::from_toml_str(
            r#"
        [project]
        name = "test"
        channels = []
        platforms = ["linux-64", "osx-arm64"]

        [dependencies]
        foo = "1.0"

        [target.linux-64.dependencies]
        foo = "2.0"
        "#,
        )
        .unwrap();

        let default_feature = manifest.default_feature();

        // For linux-64: should only have foo = "2.0" (target overrides default)
        let linux_deps = default_feature
            .run_dependencies(Some(Platform::Linux64))
            .expect("Should have dependencies for linux-64");
        let foo_specs = linux_deps
            .get(&PackageName::from_str("foo").unwrap())
            .expect("Should have foo dependency");
        let linux_specs: Vec<_> = foo_specs.iter().collect();

        assert_eq!(
            linux_specs.len(),
            1,
            "Expected exactly one spec for foo on linux-64, got {}: {:?}",
            linux_specs.len(),
            linux_specs
        );
        assert_eq!(
            linux_specs[0].as_version_spec().unwrap().to_string(),
            "==2.0",
            "Expected foo=2.0 on linux-64"
        );

        // For osx-arm64: should only have foo = "1.0" (default only)
        let osx_deps = default_feature
            .run_dependencies(Some(Platform::OsxArm64))
            .expect("Should have dependencies for osx-arm64");
        let foo_specs = osx_deps
            .get(&PackageName::from_str("foo").unwrap())
            .expect("Should have foo dependency");
        let osx_specs: Vec<_> = foo_specs.iter().collect();

        assert_eq!(
            osx_specs.len(),
            1,
            "Expected exactly one spec for foo on osx-arm64, got {}: {:?}",
            osx_specs.len(),
            osx_specs
        );
        assert_eq!(
            osx_specs[0].as_version_spec().unwrap().to_string(),
            "==1.0",
            "Expected foo=1.0 on osx-arm64"
        );
    }
}
