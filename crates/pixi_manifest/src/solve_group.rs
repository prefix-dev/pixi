use crate::environment::EnvironmentIdx;
use indexmap::{Equivalent, IndexMap};
use std::hash::Hash;
use std::ops::Index;

/// A solve group is a group of environments that are solved together.
#[derive(Debug, Clone)]
pub struct SolveGroup {
    pub name: String,
    pub environments: Vec<EnvironmentIdx>,
}

#[derive(Debug, Clone, Default)]
pub struct SolveGroups {
    solve_groups: Vec<SolveGroup>,
    by_name: IndexMap<String, SolveGroupIdx>,
}

impl Index<SolveGroupIdx> for SolveGroups {
    type Output = SolveGroup;

    fn index(&self, index: SolveGroupIdx) -> &Self::Output {
        self.solve_groups.index(index.0)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
#[repr(transparent)]
pub struct SolveGroupIdx(pub(crate) usize);

impl SolveGroups {
    /// Returns the solve group with the given name or `None` if it does not
    /// exist.
    pub fn find<Q: ?Sized>(&self, name: &Q) -> Option<&SolveGroup>
    where
        Q: Hash + Equivalent<String>,
    {
        let index = self.by_name.get(name)?;
        Some(&self.solve_groups[index.0])
    }

    /// Returns an iterator over all the solve groups in the project.
    pub fn iter(&self) -> impl Iterator<Item = &SolveGroup> + '_ {
        self.solve_groups.iter()
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut SolveGroup> + '_ {
        self.solve_groups.iter_mut()
    }

    /// Adds an environment (by index) to a solve-group.
    /// If the solve-group does not exist, it is created
    ///
    /// Returns the index of the solve-group
    pub(crate) fn add(&mut self, name: String, environment_idx: EnvironmentIdx) -> SolveGroupIdx {
        match self.by_name.get(&name) {
            Some(idx) => {
                // The solve-group exists, add the environment index to it
                self.solve_groups[idx.0].environments.push(environment_idx);
                *idx
            }
            None => {
                // The solve-group does not exist, create it
                // and initialise it with the environment index
                let idx = SolveGroupIdx(self.solve_groups.len());
                self.solve_groups.push(SolveGroup {
                    name: name.clone(),
                    environments: vec![environment_idx],
                });
                self.by_name.insert(name, idx);
                idx
            }
        }
    }
}
