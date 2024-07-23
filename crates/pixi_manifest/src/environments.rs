use crate::environment::EnvironmentIdx;
use crate::{Environment, EnvironmentName};
use indexmap::{Equivalent, IndexMap};
use std::hash::Hash;
use std::ops::Index;

/// The environments in the project.
#[derive(Debug, Clone, Default)]
pub struct Environments {
    /// A list of all environments, in the order they are defined in the
    /// manifest.
    pub(crate) environments: Vec<Option<Environment>>,

    /// A map of all environments, indexed by their name.
    pub(crate) by_name: IndexMap<EnvironmentName, EnvironmentIdx>,
}

impl Environments {
    /// Returns the environment with the given name or `None` if it does not
    /// exist.
    pub fn find<Q: ?Sized>(&self, name: &Q) -> Option<&Environment>
    where
        Q: Hash + Equivalent<EnvironmentName>,
    {
        let index = self.by_name.get(name)?;
        self.environments[index.0].as_ref()
    }

    /// Returns an iterator over all the environments in the project.
    pub fn iter(&self) -> impl Iterator<Item = &Environment> + '_ {
        self.environments.iter().flat_map(Option::as_ref)
    }

    /// Adds a new environment to the set of environments. If the environment
    /// already exists it is overwritten.
    pub fn add(&mut self, environment: Environment) -> EnvironmentIdx {
        match self.by_name.get(&environment.name) {
            Some(&idx) => {
                self.environments[idx.0] = Some(environment);
                idx
            }
            None => {
                let idx = EnvironmentIdx(self.environments.len());
                self.by_name.insert(environment.name.clone(), idx);
                self.environments.push(Some(environment));
                idx
            }
        }
    }
}

impl Index<EnvironmentIdx> for Environments {
    type Output = Environment;

    fn index(&self, index: EnvironmentIdx) -> &Self::Output {
        self.environments[index.0]
            .as_ref()
            .expect("environment has been removed")
    }
}
