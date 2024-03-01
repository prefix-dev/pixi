use crate::lock_file::{PypiPackageIdentifier, PypiRecord};
use rattler_conda_types::{PackageName, RepoDataRecord};
use std::borrow::Borrow;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

/// A struct that holds both a ``Vec` of `RepoDataRecord` and a mapping from name to index.
#[derive(Clone, Debug, Default)]
pub struct RepoDataRecordsByName {
    pub records: Vec<RepoDataRecord>,
    by_name: HashMap<PackageName, usize>,
}

impl From<Vec<RepoDataRecord>> for RepoDataRecordsByName {
    fn from(records: Vec<RepoDataRecord>) -> Self {
        let by_name = records
            .iter()
            .enumerate()
            .map(|(idx, record)| (record.package_record.name.clone(), idx))
            .collect();
        Self { records, by_name }
    }
}

impl RepoDataRecordsByName {
    /// Returns the record with the given name or `None` if no such record exists.
    pub fn by_name<Q: ?Sized>(&self, key: &Q) -> Option<&RepoDataRecord>
    where
        PackageName: Borrow<Q>,
        Q: Hash + Eq,
    {
        self.by_name.get(key).map(|idx| &self.records[*idx])
    }

    /// Converts this instance into the internally stored records.
    pub fn into_inner(self) -> Vec<RepoDataRecord> {
        self.records
    }

    /// Constructs a new instance from an iterator of repodata records. The records are
    /// deduplicated where the record with the highest version wins.
    pub fn from_iter<I: IntoIterator<Item = RepoDataRecord>>(iter: I) -> Self {
        let iter = iter.into_iter();
        let min_size = iter.size_hint().0;
        let mut by_name = HashMap::with_capacity(min_size);
        let mut records = Vec::with_capacity(min_size);
        for record in iter {
            match by_name.entry(record.package_record.name.clone()) {
                Entry::Vacant(entry) => {
                    let idx = records.len();
                    records.push(record);
                    entry.insert(idx);
                }
                Entry::Occupied(entry) => {
                    // Use the entry with the highest version or otherwise the first we encounter.
                    let idx = *entry.get();
                    if records[idx].package_record.version < record.package_record.version {
                        records[idx] = record;
                    }
                }
            }
        }

        Self { records, by_name }
    }

    /// Constructs a subset of the records in this set that only contain the packages with the given
    /// names and recursively their dependencies.
    pub fn subset(
        &self,
        package_names: impl IntoIterator<Item = PackageName>,
        virtual_packages: &HashSet<PackageName>,
    ) -> Self {
        let mut queue = package_names.into_iter().collect::<Vec<_>>();
        let mut queued_names = queue.iter().cloned().collect::<HashSet<_>>();
        let mut records = Vec::new();
        let mut by_name = HashMap::new();
        while let Some(package) = queue.pop() {
            // Find the record in the superset of records
            let found_package = if virtual_packages.contains(&package) {
                continue;
            } else if let Some(record) = self.by_name(&package) {
                record
            } else {
                continue;
            };

            // Find all the dependencies of the package and add them to the queue
            for dependency in found_package.package_record.depends.iter() {
                let dependency_name = PackageName::new_unchecked(
                    dependency.split_once(' ').unwrap_or((&dependency, "")).0,
                );
                if queued_names.insert(dependency_name.clone()) {
                    queue.push(dependency_name);
                }
            }

            let idx = records.len();
            by_name.insert(package, idx);
            records.push(found_package.clone());
        }

        Self { records, by_name }
    }
}

#[derive(Clone, Debug, Default)]
pub struct PypiRecordsByName {
    pub records: Vec<PypiRecord>,
    by_name: HashMap<uv_normalize::PackageName, usize>,
}

impl PypiRecordsByName {
    /// Returns the record with the given name or `None` if no such record exists.
    pub fn by_name<Q: ?Sized>(&self, key: &Q) -> Option<&PypiRecord>
    where
        uv_normalize::PackageName: Borrow<Q>,
        Q: Hash + Eq,
    {
        self.by_name.get(key).map(|idx| &self.records[*idx])
    }

    /// Converts this instance into the internally stored records.
    pub fn into_inner(self) -> Vec<PypiRecord> {
        self.records
    }

    /// Constructs a new instance from an iterator of repodata records. The records are
    /// deduplicated where the record with the highest version wins.
    pub fn from_iter<I: IntoIterator<Item = PypiRecord>>(iter: I) -> Self {
        let iter = iter.into_iter();
        let min_size = iter.size_hint().0;
        let mut by_name = HashMap::with_capacity(min_size);
        let mut records = Vec::with_capacity(min_size);
        for record in iter {
            match by_name.entry(record.0.name.clone()) {
                Entry::Vacant(entry) => {
                    let idx = records.len();
                    records.push(record);
                    entry.insert(idx);
                }
                Entry::Occupied(entry) => {
                    // Use the entry with the highest version or otherwise the first we encounter.
                    let idx = *entry.get();
                    if records[idx].0.version < record.0.version {
                        records[idx] = record;
                    }
                }
            }
        }

        Self { records, by_name }
    }

    /// Constructs a subset of the records in this set that only contain the packages with the given
    /// names and recursively their dependencies.
    pub fn subset(
        &self,
        package_names: impl IntoIterator<Item = uv_normalize::PackageName>,
        conda_package_identifiers: &HashMap<uv_normalize::PackageName, PypiPackageIdentifier>,
    ) -> Self {
        let mut queue = package_names.into_iter().collect::<Vec<_>>();
        let mut queued_names = queue.iter().cloned().collect::<HashSet<_>>();
        let mut records = Vec::new();
        let mut by_name = HashMap::new();
        while let Some(package) = queue.pop() {
            // Find the record in the superset of records
            let found_package = if conda_package_identifiers.contains_key(&package) {
                continue;
            } else if let Some(record) = self.by_name(&package) {
                record
            } else {
                continue;
            };

            // Find all the dependencies of the package and add them to the queue
            for dependency in found_package.0.requires_dist.iter() {
                if queued_names.insert(dependency.name.clone()) {
                    queue.push(dependency.name.clone());
                }
            }

            let idx = records.len();
            by_name.insert(package, idx);
            records.push(found_package.clone());
        }

        Self { records, by_name }
    }
}
