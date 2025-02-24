use std::{borrow::Cow, collections::HashMap, str::FromStr};

use indexmap::{map::Entry, IndexMap};
use itertools::Either;
use pixi_spec::PixiSpec;
use rattler_conda_types::{PackageName, ParsePlatformError, Platform};

use super::error::DependencyError;
use crate::{
    activation::Activation,
    pypi::PyPiPackageName,
    task::{Task, TaskName},
    utils::PixiSpanned,
    DependencyOverwriteBehavior, PyPiRequirement, SpecType,
};

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
    pub dependencies: HashMap<SpecType, IndexMap<PackageName, PixiSpec>>,

    /// Specific python dependencies
    pub pypi_dependencies: Option<IndexMap<PyPiPackageName, PyPiRequirement>>,

    /// Additional information to activate an environment.
    pub activation: Option<Activation>,

    /// Target specific tasks to run in the environment
    pub tasks: HashMap<TaskName, Task>,
}

/// A package target describes the dependencies for a specific platform.
#[derive(Default, Debug, Clone)]
pub struct PackageTarget {
    /// Dependencies for this target.
    pub dependencies: HashMap<SpecType, IndexMap<PackageName, PixiSpec>>,
}

impl WorkspaceTarget {
    /// Returns the run dependencies of the target
    pub fn run_dependencies(&self) -> Option<&IndexMap<PackageName, PixiSpec>> {
        self.dependencies.get(&SpecType::Run)
    }

    /// Returns the host dependencies of the target
    pub fn host_dependencies(&self) -> Option<&IndexMap<PackageName, PixiSpec>> {
        self.dependencies.get(&SpecType::Host)
    }

    /// Returns the build dependencies of the target
    pub fn build_dependencies(&self) -> Option<&IndexMap<PackageName, PixiSpec>> {
        self.dependencies.get(&SpecType::Build)
    }

    /// Returns the dependencies of a certain type.
    pub fn dependencies(&self, spec_type: SpecType) -> Option<&IndexMap<PackageName, PixiSpec>> {
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
    pub fn combined_dependencies(&self) -> Option<Cow<'_, IndexMap<PackageName, PixiSpec>>> {
        let mut all_deps = None;
        for spec_type in [SpecType::Run, SpecType::Host, SpecType::Build] {
            let Some(specs) = self.dependencies.get(&spec_type) else {
                // If the specific dependencies don't exist we can skip them.
                continue;
            };
            if specs.is_empty() {
                // If the dependencies are empty, we can skip them.
                continue;
            }

            all_deps = match all_deps {
                None => Some(Cow::Borrowed(specs)),
                Some(mut all_deps) => {
                    all_deps.to_mut().extend(
                        specs
                            .into_iter()
                            .map(|(name, spec)| (name.clone(), spec.clone())),
                    );
                    Some(all_deps)
                }
            }
        }
        all_deps
    }

    /// Checks if this target contains a dependency
    pub fn has_dependency(
        &self,
        dep_name: &PackageName,
        spec_type: SpecType,
        exact: Option<&PixiSpec>,
    ) -> bool {
        let current_dependency = self
            .dependencies(spec_type)
            .and_then(|deps| deps.get(dep_name).cloned());

        match (current_dependency, exact) {
            (Some(current_spec), Some(spec)) => current_spec == *spec,
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
        dependencies
            .shift_remove_entry(dep_name)
            .ok_or_else(|| DependencyError::NoDependency(dep_name.as_normalized().into()))
    }

    /// Adds a dependency to a target
    ///
    /// This will overwrite any existing dependency of the same name
    pub fn add_dependency(&mut self, dep_name: &PackageName, spec: &PixiSpec, spec_type: SpecType) {
        self.dependencies
            .entry(spec_type)
            .or_default()
            .insert(dep_name.clone(), spec.clone());
    }

    /// Adds a dependency to a target
    ///
    /// This will return an error if the exact same dependency already exist
    /// This will overwrite any existing dependency of the same name
    pub fn try_add_dependency(
        &mut self,
        dep_name: &PackageName,
        spec: &PixiSpec,
        spec_type: SpecType,
        dependency_overwrite_behavior: DependencyOverwriteBehavior,
    ) -> Result<bool, DependencyError> {
        if self.has_dependency(dep_name, spec_type, None) {
            match dependency_overwrite_behavior {
                DependencyOverwriteBehavior::OverwriteIfExplicit if !spec.has_version_spec() => {
                    return Ok(false)
                }
                DependencyOverwriteBehavior::IgnoreDuplicate => return Ok(false),
                DependencyOverwriteBehavior::Error => {
                    return Err(DependencyError::Duplicate(dep_name.as_normalized().into()));
                }
                _ => {}
            }
        }
        self.add_dependency(dep_name, spec, spec_type);
        Ok(true)
    }

    /// Checks if this target contains a specific pypi dependency
    pub fn has_pypi_dependency(&self, requirement: &pep508_rs::Requirement, exact: bool) -> bool {
        let current_requirement = self
            .pypi_dependencies
            .as_ref()
            .and_then(|deps| deps.get(&PyPiPackageName::from_normalized(requirement.name.clone())));

        match (current_requirement, exact) {
            (Some(r), true) => {
                // TODO: would be nice to compare pep508 == PyPiRequirement directly
                *r == PyPiRequirement::try_from(requirement.clone())
                    .expect("could not convert pep508 requirement")
            }
            (Some(r), false) => {
                if r.extras() != requirement.extras {
                    return false;
                }
                true
            }
            (None, _) => false,
        }
    }

    /// Removes a pypi dependency from this target
    ///
    /// it will Err if the dependency is not found
    pub fn remove_pypi_dependency(
        &mut self,
        dep_name: &PyPiPackageName,
    ) -> Result<(PyPiPackageName, PyPiRequirement), DependencyError> {
        let Some(pypi_dependencies) = self.pypi_dependencies.as_mut() else {
            return Err(DependencyError::NoPyPiDependencies);
        };
        pypi_dependencies
            .shift_remove_entry(dep_name)
            .ok_or_else(|| DependencyError::NoDependency(dep_name.as_source().into()))
    }

    /// Adds a pypi dependency to a target
    ///
    /// This will overwrite any existing dependency of the same name
    pub fn add_pypi_dependency(&mut self, name: PyPiPackageName, requirement: PyPiRequirement) {
        tracing::info!(
            "Adding pypi dependency: {} {}",
            name.as_normalized(),
            requirement
        );
        self.pypi_dependencies
            .get_or_insert_with(Default::default)
            .insert(name, requirement);
    }

    /// Adds a pypi dependency to a target
    ///
    /// This will return an error if the exact same dependency already exist
    /// This will overwrite any existing dependency of the same name
    pub fn try_add_pep508_dependency(
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
        let name = PyPiPackageName::from_normalized(requirement.name.clone());
        let mut requirement = PyPiRequirement::try_from(requirement.clone()).map_err(Box::new)?;
        if let Some(editable) = editable {
            requirement.set_editable(editable);
        }

        self.add_pypi_dependency(name, requirement);
        Ok(true)
    }
}

impl PackageTarget {
    /// Returns the dependencies of a certain type.
    pub fn dependencies(&self, spec_type: SpecType) -> Option<&IndexMap<PackageName, PixiSpec>> {
        self.dependencies.get(&spec_type)
    }

    /// Returns the run dependencies of the target
    pub fn run_dependencies(&self) -> Option<&IndexMap<PackageName, PixiSpec>> {
        self.dependencies.get(&SpecType::Run)
    }

    /// Returns the host dependencies of the target
    pub fn host_dependencies(&self) -> Option<&IndexMap<PackageName, PixiSpec>> {
        self.dependencies.get(&SpecType::Host)
    }

    /// Returns the build dependencies of the target
    pub fn build_dependencies(&self) -> Option<&IndexMap<PackageName, PixiSpec>> {
        self.dependencies.get(&SpecType::Build)
    }

    /// Checks if this target contains a dependency
    pub fn has_dependency(
        &self,
        dep_name: &PackageName,
        spec_type: SpecType,
        exact: Option<&PixiSpec>,
    ) -> bool {
        let current_dependency = self
            .dependencies(spec_type)
            .and_then(|deps| deps.get(dep_name).cloned());

        match (current_dependency, exact) {
            (Some(current_spec), Some(spec)) => current_spec == *spec,
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
        dependencies
            .shift_remove_entry(dep_name)
            .ok_or_else(|| DependencyError::NoDependency(dep_name.as_normalized().into()))
    }

    /// Adds a dependency to a target
    ///
    /// This will overwrite any existing dependency of the same name
    pub fn add_dependency(&mut self, dep_name: &PackageName, spec: &PixiSpec, spec_type: SpecType) {
        self.dependencies
            .entry(spec_type)
            .or_default()
            .insert(dep_name.clone(), spec.clone());
    }

    /// Adds a dependency to a target
    ///
    /// This will return an error if the exact same dependency already exist
    /// This will overwrite any existing dependency of the same name
    pub fn try_add_dependency(
        &mut self,
        dep_name: &PackageName,
        spec: &PixiSpec,
        spec_type: SpecType,
        dependency_overwrite_behavior: DependencyOverwriteBehavior,
    ) -> Result<bool, DependencyError> {
        if self.has_dependency(dep_name, spec_type, None) {
            match dependency_overwrite_behavior {
                DependencyOverwriteBehavior::OverwriteIfExplicit if !spec.has_version_spec() => {
                    return Ok(false)
                }
                DependencyOverwriteBehavior::IgnoreDuplicate => return Ok(false),
                DependencyOverwriteBehavior::Error => {
                    return Err(DependencyError::Duplicate(dep_name.as_normalized().into()));
                }
                _ => {}
            }
        }
        self.add_dependency(dep_name, spec, spec_type);
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
            TargetSelector::Platform(p) => write!(f, "{}", p),
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

    use crate::WorkspaceManifest;

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
            .map(|(name, spec)| format!("{} = {}", name.as_source(), spec.as_version_spec().unwrap()))
            .join("\n"), @r###"
        run = ==2.0
        host = ==2.0
        build = ==1.0
        "###);
    }
}
