use crate::{
    consts,
    prefix::Prefix,
    project::{
        manifest::{PyPiRequirement, SystemRequirements},
        virtual_packages::get_minimal_virtual_packages,
        Dependencies, Environment, SolveGroup,
    },
    EnvironmentName, Project, SpecType,
};
use indexmap::{IndexMap, IndexSet};
use rattler_conda_types::{Channel, GenericVirtualPackage, Platform};
use std::path::PathBuf;

/// Either a solve group or an individual environment without a solve group.
///
/// If a solve group only contains a single environment then it is treated as a single environment,
/// not as a solve-group.
///
/// Construct a `GroupedEnvironment` from a `SolveGroup` or `Environment` using `From` trait.
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
    /// Constructs a `GroupedEnvironment` from a `GroupedEnvironmentName`.
    pub fn from_name(project: &'p Project, name: &GroupedEnvironmentName) -> Option<Self> {
        match name {
            GroupedEnvironmentName::Group(g) => {
                Some(GroupedEnvironment::Group(project.solve_group(g)?))
            }
            GroupedEnvironmentName::Environment(env) => {
                Some(GroupedEnvironment::Environment(project.environment(env)?))
            }
        }
    }

    /// Returns the project to which the group belongs.
    pub fn project(&self) -> &'p Project {
        match self {
            GroupedEnvironment::Group(group) => group.project(),
            GroupedEnvironment::Environment(env) => env.project(),
        }
    }

    /// Returns the prefix of this group.
    pub fn prefix(&self) -> Prefix {
        Prefix::new(self.dir())
    }

    /// Returns the directory where the prefix of this instance is stored.
    pub fn dir(&self) -> PathBuf {
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

    /// Returns the dependencies of the group.
    pub fn dependencies(&self, kind: Option<SpecType>, platform: Option<Platform>) -> Dependencies {
        match self {
            GroupedEnvironment::Group(group) => group.dependencies(kind, platform),
            GroupedEnvironment::Environment(env) => env.dependencies(kind, platform),
        }
    }

    /// Returns the pypi dependencies of the group.
    pub fn pypi_dependencies(
        &self,
        platform: Option<Platform>,
    ) -> IndexMap<rip::types::PackageName, Vec<PyPiRequirement>> {
        match self {
            GroupedEnvironment::Group(group) => group.pypi_dependencies(platform),
            GroupedEnvironment::Environment(env) => env.pypi_dependencies(platform),
        }
    }

    /// Returns the system requirements of the group.
    pub fn system_requirements(&self) -> SystemRequirements {
        match self {
            GroupedEnvironment::Group(group) => group.system_requirements(),
            GroupedEnvironment::Environment(env) => env.system_requirements(),
        }
    }

    /// Returns the virtual packages from the group based on the system requirements.
    pub fn virtual_packages(&self, platform: Platform) -> Vec<GenericVirtualPackage> {
        get_minimal_virtual_packages(platform, &self.system_requirements())
            .into_iter()
            .map(GenericVirtualPackage::from)
            .collect()
    }

    /// Returns the channels used for the group.
    pub fn channels(&self) -> IndexSet<&'p Channel> {
        match self {
            GroupedEnvironment::Group(group) => group.channels(),
            GroupedEnvironment::Environment(env) => env.channels(),
        }
    }

    /// Returns true if the group has any Pypi dependencies.
    pub fn has_pypi_dependencies(&self) -> bool {
        match self {
            GroupedEnvironment::Group(group) => group.has_pypi_dependencies(),
            GroupedEnvironment::Environment(env) => env.has_pypi_dependencies(),
        }
    }
}

/// A name of a [`GroupedEnvironment`].
#[derive(Clone)]
pub enum GroupedEnvironmentName {
    Group(String),
    Environment(EnvironmentName),
}

impl GroupedEnvironmentName {
    /// Returns a fancy display of the name that can be used in the console.
    pub fn fancy_display(&self) -> console::StyledObject<&str> {
        match self {
            GroupedEnvironmentName::Group(name) => {
                consts::SOLVE_GROUP_STYLE.apply_to(name.as_str())
            }
            GroupedEnvironmentName::Environment(name) => name.fancy_display(),
        }
    }

    /// Returns the name as a string slice.
    pub fn as_str(&self) -> &str {
        match self {
            GroupedEnvironmentName::Group(group) => group.as_str(),
            GroupedEnvironmentName::Environment(env) => env.as_str(),
        }
    }
}
