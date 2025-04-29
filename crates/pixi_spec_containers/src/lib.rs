use indexmap::{Equivalent, IndexMap, IndexSet};
use itertools::Either;
use std::{borrow::Cow, hash::Hash, iter::FromIterator};

/// Holds a list of dependencies where for each package name there can be
/// multiple requirements.
///
/// This is used when combining the dependencies of multiple features. Although
/// each target can only have one requirement for a given package, when
/// combining the dependencies of multiple features there can be multiple
/// requirements for a given package.
///
/// The generic 'Dependencies' struct is aliased as specific PyPiDependencies
/// and CondaDependencies struct to represent Pypi and Conda dependencies
/// respectively.

#[derive(Debug, Clone)]
pub struct DependencyMap<N: Hash + Eq + Clone, D: Hash + Eq + Clone> {
    map: IndexMap<N, IndexSet<D>>,
}

impl<N: Hash + Eq + Clone, D: Hash + Eq + Clone> Default for DependencyMap<N, D> {
    fn default() -> Self {
        DependencyMap {
            map: IndexMap::new(),
        }
    }
}

impl<N: Hash + Eq + Clone, D: Hash + Eq + Clone> IntoIterator for DependencyMap<N, D> {
    type Item = (N, IndexSet<D>);
    type IntoIter = indexmap::map::IntoIter<N, IndexSet<D>>;

    fn into_iter(self) -> Self::IntoIter {
        self.map.into_iter()
    }
}

impl<N: Hash + Eq + Clone, D: Hash + Eq + Clone> Extend<(N, D)> for DependencyMap<N, D> {
    fn extend<T: IntoIterator<Item = (N, D)>>(&mut self, iter: T) {
        for (name, spec) in iter {
            self.insert(name, spec);
        }
    }
}

impl<'a, M, N: Hash + Eq + Clone + 'a, D: Hash + Eq + Clone + 'a> From<M> for DependencyMap<N, D>
where
    M: IntoIterator<Item = Cow<'a, IndexMap<N, D>>>,
{
    /// Create Dependencies<N, D> from an iterator over items of type Cow<'a,
    /// IndexMap<N, D>
    fn from(m: M) -> Self {
        m.into_iter().fold(Self::default(), |mut acc: Self, deps| {
            // Either clone the values from the Cow or move the values from the owned map.
            let deps_iter = match deps {
                Cow::Borrowed(borrowed) => Either::Left(
                    borrowed
                        .into_iter()
                        .map(|(name, spec)| (name.clone(), spec.clone())),
                ),
                Cow::Owned(owned) => Either::Right(owned.into_iter()),
            };

            // Add the requirements to the accumulator.
            for (name, spec) in deps_iter {
                acc.insert(name, spec);
            }

            acc
        })
    }
}

impl<N: Hash + Eq + Clone, D: Hash + Eq + Clone> FromIterator<(N, D)> for DependencyMap<N, D> {
    fn from_iter<T: IntoIterator<Item = (N, D)>>(iter: T) -> Self {
        let mut deps = DependencyMap::default();
        for (name, spec) in iter {
            deps.insert(name, spec);
        }
        deps
    }
}

impl<N: Hash + Eq + Clone, D: Hash + Eq + Clone> DependencyMap<N, D> {
    /// Adds a requirement to the list of dependencies.
    pub fn insert(&mut self, name: N, spec: D) {
        self.map.entry(name).or_default().insert(spec);
    }

    /// Check if there is any dependency
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Removes a specific dependency
    pub fn remove<Q>(&mut self, name: &Q) -> Option<(N, IndexSet<D>)>
    where
        Q: ?Sized + Hash + Equivalent<N>,
    {
        self.map.shift_remove_entry(name)
    }

    /// Combines two sets of dependencies where the requirements of `self` are
    /// overwritten if the same package is also defined in `other`.
    pub fn overwrite(&self, other: &Self) -> Self {
        let mut map = self.map.clone();
        for (name, specs) in other.map.iter() {
            map.insert(name.clone(), specs.clone());
        }
        Self { map }
    }

    /// Returns an iterator over tuples of dependency names and their combined
    /// requirements.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = (&N, &IndexSet<D>)> + '_ {
        self.map.iter()
    }

    /// Returns an iterator over tuples of dependency names and individual
    /// requirements.
    pub fn iter_specs(&self) -> impl DoubleEndedIterator<Item = (&N, &D)> + '_ {
        self.map
            .iter()
            .flat_map(|(name, specs)| specs.iter().map(move |spec| (name, spec)))
    }

    /// Return an iterator over the dependency names.
    pub fn names(&self) -> impl DoubleEndedIterator<Item = &N> + ExactSizeIterator + '_ {
        self.map.keys()
    }

    /// Converts this instance into an iterator over tuples of dependency names
    /// and individual requirements.
    pub fn into_specs(self) -> impl DoubleEndedIterator<Item = (N, D)> {
        self.map
            .into_iter()
            .flat_map(|(name, specs)| specs.into_iter().map(move |spec| (name.clone(), spec)))
    }

    /// Returns true if the dependency list contains the given package name.
    pub fn contains_key<Q>(&self, name: &Q) -> bool
    where
        Q: ?Sized + Hash + Equivalent<N>,
    {
        self.map.contains_key(name)
    }

    /// Returns the package specs for the specified package name.
    pub fn get<Q>(&self, name: &Q) -> Option<&IndexSet<D>>
    where
        Q: ?Sized + Hash + Equivalent<N>,
    {
        self.map.get(name)
    }
}

impl DependencyMap<rattler_conda_types::PackageName, rattler_conda_types::NamelessMatchSpec> {
    /// Converts this instance into an iterator of [`rattler_conda_types::MatchSpec`]s.
    pub fn into_match_specs(
        self,
    ) -> impl DoubleEndedIterator<Item = rattler_conda_types::MatchSpec> {
        self.map.into_iter().flat_map(|(name, specs)| {
            specs.into_iter().map(move |spec| {
                rattler_conda_types::MatchSpec::from_nameless(spec, Some(name.clone()))
            })
        })
    }

    /// Returns an iterator over [`rattler_conda_types::MatchSpec`]s.
    pub fn iter_match_specs(
        &self,
    ) -> impl DoubleEndedIterator<Item = rattler_conda_types::MatchSpec> {
        self.map.iter().flat_map(|(name, specs)| {
            specs.into_iter().map(move |spec| {
                rattler_conda_types::MatchSpec::from_nameless(spec.clone(), Some(name.clone()))
            })
        })
    }
}
