use crate::utils::spanned::PixiSpanned;

/// The name of an environment. This is either a string or default for the default environment.
#[derive(Clone, Debug, Eq, PartialEq, PartialOrd, Ord, Hash)]
pub enum EnvironmentName {
    Default,
    Named(String),
}

impl EnvironmentName {
    /// Returns the name of the environment or `None` if this is the default environment.
    pub fn name(&self) -> Option<&str> {
        match self {
            EnvironmentName::Default => None,
            EnvironmentName::Named(name) => Some(name),
        }
    }
}

/// An environment describes a set of features that are available together.
///
/// Individual features cannot be used directly, instead they are grouped together into
/// environments. Environments are then locked and installed.
#[derive(Debug, Clone)]
pub struct Environment {
    /// The name of the environment
    pub name: EnvironmentName,

    /// The names of the features that together make up this environment.
    ///
    /// Note that the default feature is always added to the set of features that make up the
    /// environment.
    pub features: PixiSpanned<Vec<String>>,

    /// An optional solver-group. Multiple environments can share the same solve-group. All the
    /// dependencies of the environment that share the same solve-group will be solved together.
    pub solve_group: Option<String>,
}
