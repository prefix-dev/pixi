use indexmap::{Equivalent, IndexMap};
use rattler_conda_types::{MatchSpec, NamelessMatchSpec, PackageName};
use std::{collections::HashSet, hash::Hash, iter::once};

/// Holds a list of dependencies where for each package name there can be multiple requirements.
///
/// This is used when combining the dependencies of multiple features. Although each target can only
/// have one requirement for a given package, when combining the dependencies of multiple features
/// there can be multiple requirements for a given package.
#[derive(Default, Debug, Clone)]
pub struct Dependencies {
    map: IndexMap<PackageName, HashSet<NamelessMatchSpec>>,
}

impl From<IndexMap<PackageName, HashSet<NamelessMatchSpec>>> for Dependencies {
    fn from(map: IndexMap<PackageName, HashSet<NamelessMatchSpec>>) -> Self {
        Self { map }
    }
}

impl From<IndexMap<PackageName, NamelessMatchSpec>> for Dependencies {
    fn from(map: IndexMap<PackageName, NamelessMatchSpec>) -> Self {
        Self {
            map: map
                .into_iter()
                .map(|(k, v)| (k, once(v).collect()))
                .collect(),
        }
    }
}

impl Dependencies {
    /// Adds the given spec to the list of dependencies.
    pub fn insert(&mut self, name: PackageName, spec: NamelessMatchSpec) {
        self.map.entry(name).or_default().insert(spec);
    }

    /// Adds a list of specs to the list of dependencies.
    pub fn extend(&mut self, iter: impl IntoIterator<Item = (PackageName, NamelessMatchSpec)>) {
        for (name, spec) in iter {
            self.insert(name, spec);
        }
    }

    /// Adds a list of specs to the list of dependencies overwriting any existing requirements for
    /// packages that already exist in the list of dependencies.
    pub fn extend_overwrite(
        &mut self,
        iter: impl IntoIterator<Item = (PackageName, NamelessMatchSpec)>,
    ) {
        for (name, spec) in iter {
            *self.map.entry(name).or_default() = once(spec).collect();
        }
    }

    /// Removes all requirements for the given package and returns them.
    pub fn remove<Q: ?Sized>(
        &mut self,
        name: &Q,
    ) -> Option<(PackageName, HashSet<NamelessMatchSpec>)>
    where
        Q: Hash + Equivalent<PackageName>,
    {
        self.map.shift_remove_entry(name)
    }

    /// Combine two sets of dependencies together where the requirements of `self` are extended if
    /// the same package is also defined in `other`.
    pub fn union(&self, other: &Self) -> Self {
        let mut map = self.map.clone();
        for (name, specs) in &other.map {
            map.entry(name.clone()).or_default().extend(specs.clone());
        }
        Self { map }
    }

    /// Combines two sets of dependencies where the requirements of `self` are overwritten if the
    /// same package is also defined in `other`.
    pub fn overwrite(&self, other: &Self) -> Self {
        let mut map = self.map.clone();
        for (name, specs) in other.map.iter() {
            map.insert(name.clone(), specs.clone());
        }
        Self { map }
    }

    /// Returns an iterator over the package names and their corresponding requirements.
    pub fn iter(
        &self,
    ) -> impl DoubleEndedIterator<Item = (&PackageName, &HashSet<NamelessMatchSpec>)> + '_ {
        self.map.iter()
    }

    /// Returns an iterator over all the requirements.
    pub fn iter_specs(&self) -> impl Iterator<Item = (&PackageName, &NamelessMatchSpec)> + '_ {
        self.map
            .iter()
            .flat_map(|(name, specs)| specs.iter().map(move |spec| (name, spec)))
    }

    /// Returns the names of all the packages that have requirements.
    pub fn names(&self) -> impl DoubleEndedIterator<Item = &PackageName> + ExactSizeIterator + '_ {
        self.map.keys()
    }

    /// Convert this instance into an iterator over the package names and their corresponding
    pub fn into_specs(self) -> impl Iterator<Item = (PackageName, NamelessMatchSpec)> {
        self.map
            .into_iter()
            .flat_map(|(name, specs)| specs.into_iter().map(move |spec| (name.clone(), spec)))
    }

    /// Converts this instance into an iterator of [`MatchSpec`]s.
    pub fn into_match_specs(self) -> impl Iterator<Item = MatchSpec> {
        self.map.into_iter().flat_map(|(name, specs)| {
            specs
                .into_iter()
                .map(move |spec| MatchSpec::from_nameless(spec, Some(name.clone())))
        })
    }
}

impl IntoIterator for Dependencies {
    type Item = (PackageName, HashSet<NamelessMatchSpec>);
    type IntoIter = indexmap::map::IntoIter<PackageName, HashSet<NamelessMatchSpec>>;

    fn into_iter(self) -> Self::IntoIter {
        self.map.into_iter()
    }
}
