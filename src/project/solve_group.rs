use super::{manifest, Environment, Project};

/// A grouping of environments that are solved together.
#[derive(Debug, Clone)]
pub struct SolveGroup<'p> {
    /// The project that the group is part of.
    pub(super) project: &'p Project,

    /// A reference to the solve group in the manifest
    pub(super) solve_group: &'p manifest::SolveGroup,
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

    /// Returns an iterator over all the environments that are part of the group.
    pub fn environments(&self) -> impl Iterator<Item = Environment<'p>> + '_ {
        self.solve_group
            .environments
            .iter()
            .map(|env_idx| Environment {
                project: self.project,
                environment: &self.project.manifest.parsed.environments.environments[*env_idx],
            })
    }
}
