use std::collections::BTreeMap;
use std::fmt::Display;
use std::path::PathBuf;
use std::sync::Arc;

use fancy_display::FancyDisplay;
use indexmap::IndexMap;
use itertools::Either;
use ordermap::OrderSet;
use pixi_consts::consts;
use pixi_manifest::{
    EnvironmentName, Feature, HasFeaturesIter, HasWorkspaceManifest, InlinePackageManifest,
    PixiPlatform, WorkspaceManifest,
};
use pixi_spec::SourceLocationSpec;
use pixi_utils::prefix::Prefix;
use rattler_conda_types::{ChannelConfig, GenericVirtualPackage, PackageName};

use crate::{
    Workspace,
    workspace::{
        Environment, HasWorkspaceRef, SolveGroup, virtual_packages::get_minimal_virtual_packages,
    },
};

/// Either a solve group or an individual environment without a solve group.
///
/// If a solve group only contains a single environment then it is treated as a
/// single environment, not as a solve-group.
///
/// Construct a `GroupedEnvironment` from a `SolveGroup` or `Environment` using
/// `From` trait.
#[derive(Debug, Hash, Eq, PartialEq, Clone)]
pub enum GroupedEnvironment<'p> {
    Group(SolveGroup<'p>),
    Environment(Environment<'p>),
}

impl<'p> From<SolveGroup<'p>> for GroupedEnvironment<'p> {
    fn from(source: SolveGroup<'p>) -> Self {
        let mut envs = source.environments().peekable();
        let first = envs.next();
        let second = envs.peek();
        if second.is_some() {
            GroupedEnvironment::Group(source)
        } else if let Some(first) = first {
            GroupedEnvironment::Environment(first)
        } else {
            unreachable!("empty solve group")
        }
    }
}

impl<'p> From<Environment<'p>> for GroupedEnvironment<'p> {
    fn from(source: Environment<'p>) -> Self {
        match source.solve_group() {
            Some(group) if group.environments().len() > 1 => GroupedEnvironment::Group(group),
            _ => GroupedEnvironment::Environment(source),
        }
    }
}

impl<'p> GroupedEnvironment<'p> {
    /// Returns an iterator over all the environments in the group.
    pub(crate) fn environments(&self) -> impl Iterator<Item = Environment<'p>> + '_ {
        match self {
            GroupedEnvironment::Group(group) => Either::Left(group.environments()),
            GroupedEnvironment::Environment(env) => Either::Right(std::iter::once(env.clone())),
        }
    }

    /// Constructs a `GroupedEnvironment` from a `GroupedEnvironmentName`.
    pub(crate) fn from_name(project: &'p Workspace, name: &GroupedEnvironmentName) -> Option<Self> {
        match name {
            GroupedEnvironmentName::Group(g) => {
                Some(GroupedEnvironment::Group(project.solve_group(g)?))
            }
            GroupedEnvironmentName::Environment(env) => {
                Some(GroupedEnvironment::Environment(project.environment(env)?))
            }
        }
    }

    /// Returns the prefix of this group.
    pub fn prefix(&self) -> Prefix {
        Prefix::new(self.dir())
    }

    /// Returns the directory where the prefix of this instance is stored.
    pub(crate) fn dir(&self) -> PathBuf {
        match self {
            GroupedEnvironment::Group(solve_group) => solve_group.dir(),
            GroupedEnvironment::Environment(env) => env.dir(),
        }
    }

    /// Returns the name of the group.
    pub fn name(&self) -> GroupedEnvironmentName {
        match self {
            GroupedEnvironment::Group(group) => {
                GroupedEnvironmentName::Group(group.name().to_string())
            }
            GroupedEnvironment::Environment(env) => {
                GroupedEnvironmentName::Environment(env.name().clone())
            }
        }
    }
    /// Returns the virtual packages from the group, sourced from the
    /// platform's declared virtual packages with default fillers.
    pub fn virtual_packages(&self, platform: &PixiPlatform) -> Vec<GenericVirtualPackage> {
        get_minimal_virtual_packages(platform)
            .into_iter()
            .map(GenericVirtualPackage::from)
            .collect()
    }

    /// Returns the channel configuration for this grouped environment
    pub fn channel_config(&self) -> ChannelConfig {
        self.workspace().channel_config()
    }

    /// Returns the combined dev dependencies for this grouped environment.
    ///
    /// Dev dependencies from all features in the group are collected and
    /// merged. If multiple features define the same dev dependency, the
    /// last one wins (later features override earlier ones).
    pub fn combined_dev_dependencies(
        &self,
        platform: Option<&PixiPlatform>,
    ) -> IndexMap<PackageName, OrderSet<SourceLocationSpec>> {
        let mut result = IndexMap::new();
        for feature in self.features().rev() {
            if let Some(deps) = feature.dev_dependencies(platform) {
                result.extend(deps.into_owned());
            }
        }
        result
    }

    /// Returns the combined inline package definitions for this grouped
    /// environment, resolved into dispatcher
    /// [`InlinePackage`](pixi_command_dispatcher::InlinePackage)s ready to thread
    /// through the solve and install. Definitions from all features are merged;
    /// later features override earlier ones with the same name. The consuming
    /// workspace manifest is attached so the backend can be built without an
    /// on-disk manifest.
    pub fn combined_inline_packages(
        &self,
        platform: Option<&PixiPlatform>,
    ) -> BTreeMap<PackageName, pixi_command_dispatcher::InlinePackage> {
        let mut merged: IndexMap<PackageName, &InlinePackageManifest> = IndexMap::new();
        for feature in self.features().rev() {
            for (name, manifest) in feature.inline_packages(platform) {
                merged.insert(name, manifest);
            }
        }
        if merged.is_empty() {
            return BTreeMap::new();
        }
        let workspace = Arc::new(self.workspace_manifest().clone());
        merged
            .into_iter()
            .map(|(name, inline)| {
                (
                    name,
                    pixi_command_dispatcher::InlinePackage {
                        manifest: Arc::new(inline.manifest.clone()),
                        workspace: workspace.clone(),
                        content_hash: inline.content_hash,
                    },
                )
            })
            .collect()
    }
}

impl<'p> HasWorkspaceRef<'p> for GroupedEnvironment<'p> {
    fn workspace(&self) -> &'p Workspace {
        match self {
            GroupedEnvironment::Group(group) => group.workspace(),
            GroupedEnvironment::Environment(env) => env.workspace(),
        }
    }
}

impl<'p> HasWorkspaceManifest<'p> for GroupedEnvironment<'p> {
    fn workspace_manifest(&self) -> &'p WorkspaceManifest {
        self.workspace().workspace_manifest()
    }
}

impl<'p> HasFeaturesIter<'p> for GroupedEnvironment<'p> {
    /// Returns the features of the group
    fn features(&self) -> impl DoubleEndedIterator<Item = &'p Feature> + 'p {
        match self {
            GroupedEnvironment::Group(group) => Either::Left(group.features()),
            GroupedEnvironment::Environment(env) => Either::Right(env.features()),
        }
    }
}

/// A name of a [`GroupedEnvironment`].
#[derive(Debug, Clone)]
pub enum GroupedEnvironmentName {
    Group(String),
    Environment(EnvironmentName),
}

impl GroupedEnvironmentName {
    /// Returns a fancy display of the name that can be used in the console.
    pub(crate) fn fancy_display(&self) -> console::StyledObject<&str> {
        match self {
            GroupedEnvironmentName::Group(name) => {
                consts::SOLVE_GROUP_STYLE.apply_to(name.as_str())
            }
            GroupedEnvironmentName::Environment(name) => name.fancy_display(),
        }
    }

    /// Returns the name as a string slice.
    pub(crate) fn as_str(&self) -> &str {
        match self {
            GroupedEnvironmentName::Group(group) => group.as_str(),
            GroupedEnvironmentName::Environment(env) => env.as_str(),
        }
    }
}

impl Display for GroupedEnvironmentName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GroupedEnvironmentName::Group(name) => write!(f, "{name}"),
            GroupedEnvironmentName::Environment(name) => write!(f, "{name}"),
        }
    }
}
