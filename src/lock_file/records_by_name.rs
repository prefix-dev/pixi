use crate::lock_file::{PypiPackageIdentifier, PypiRecord};
use crate::pypi_tags::is_python_record;
use rattler_conda_types::{PackageName, RepoDataRecord, VersionWithSource};
use rattler_lock::FileFormatVersion;
use std::borrow::Borrow;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::hash::Hash;

pub type RepoDataRecordsByName = DependencyRecordsByName<PackageName, RepoDataRecord>;
pub type PypiRecordsByName = DependencyRecordsByName<uv_normalize::PackageName, PypiRecord>;

/// A trait required from the dependencies stored in DependencyRecordsByName
pub(crate) trait HasNameVersion<N> {
    fn name(&self) -> &N;
    fn version(&self) -> &impl PartialOrd;
}

impl HasNameVersion<uv_normalize::PackageName> for PypiRecord {
    fn name(&self) -> &uv_normalize::PackageName {
        &self.0.name
    }
    fn version(&self) -> &pep440_rs::Version {
        &self.0.version
    }
}
impl HasNameVersion<PackageName> for RepoDataRecord {
    fn name(&self) -> &PackageName {
        &self.package_record.name
    }
    fn version(&self) -> &VersionWithSource {
        &self.package_record.version
    }
}

/// A struct that holds both a ``Vec` of `DependencyRecord` and a mapping from name to index.
#[derive(Clone, Debug)]
pub struct DependencyRecordsByName<N: Hash + Eq + Clone, D: HasNameVersion<N>> {
    pub records: Vec<D>,
    by_name: HashMap<N, usize>,
}

impl<N: Hash + Eq + Clone, D: HasNameVersion<N>> Default for DependencyRecordsByName<N, D> {
    fn default() -> Self {
        Self {
            records: Vec::new(),
            by_name: HashMap::new(),
        }
    }
}

impl<N: Hash + Eq + Clone, D: HasNameVersion<N>> From<Vec<D>> for DependencyRecordsByName<N, D> {
    fn from(records: Vec<D>) -> Self {
        let by_name = records
            .iter()
            .enumerate()
            .map(|(idx, record)| (record.name().clone(), idx))
            .collect();
        Self { records, by_name }
    }
}

impl<N: Hash + Eq + Clone, D: HasNameVersion<N>> DependencyRecordsByName<N, D> {
    /// Returns the record with the given name or `None` if no such record exists.
    pub fn by_name<Q: ?Sized>(&self, key: &Q) -> Option<&D>
    where
        N: Borrow<Q>,
        Q: Hash + Eq,
    {
        self.by_name.get(key).map(|idx| &self.records[*idx])
    }

    /// Returns the index of the record with the given name or `None` if no such record exists.
    pub fn index_by_name<Q: ?Sized>(&self, key: &Q) -> Option<usize>
    where
        N: Borrow<Q>,
        Q: Hash + Eq,
    {
        self.by_name.get(key).copied()
    }
    /// Returns true if there are no records stored in this instance
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Returns the number of entries in the mapping.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Converts this instance into the internally stored records.
    pub fn into_inner(self) -> Vec<D> {
        self.records
    }

    /// Returns an iterator over the names of the records stored in this instance.
    pub fn names(&self) -> impl Iterator<Item = &N> {
        // Iterate over the records to retain the index of the original record.
        self.records.iter().map(|r| r.name())
    }

    /// Constructs a new instance from an iterator of pypi records. If multiple records exist
    /// for the same package name an error is returned.
    pub fn from_unique_iter<I: IntoIterator<Item = D>>(iter: I) -> Result<Self, Box<D>> {
        let iter = iter.into_iter();
        let min_size = iter.size_hint().0;
        let mut by_name = HashMap::with_capacity(min_size);
        let mut records = Vec::with_capacity(min_size);
        for record in iter {
            match by_name.entry(record.name().clone()) {
                Entry::Vacant(entry) => {
                    let idx = records.len();
                    records.push(record);
                    entry.insert(idx);
                }
                Entry::Occupied(_) => {
                    return Err(Box::new(record));
                }
            }
        }
        Ok(Self { records, by_name })
    }

    /// Constructs a new instance from an iterator of repodata records. The records are
    /// deduplicated where the record with the highest version wins.
    pub fn from_iter<I: IntoIterator<Item = D>>(iter: I) -> Self {
        let iter = iter.into_iter();
        let min_size = iter.size_hint().0;
        let mut by_name = HashMap::with_capacity(min_size);
        let mut records = Vec::with_capacity(min_size);
        for record in iter {
            match by_name.entry(record.name().clone()) {
                Entry::Vacant(entry) => {
                    let idx = records.len();
                    records.push(record);
                    entry.insert(idx);
                }
                Entry::Occupied(entry) => {
                    // Use the entry with the highest version or otherwise the first we encounter.
                    let idx = *entry.get();
                    if records[idx].version() < record.version() {
                        records[idx] = record;
                    }
                }
            }
        }

        Self { records, by_name }
    }
}

impl RepoDataRecordsByName {
    /// Returns the record that represents the python interpreter or `None` if no such record exists.
    pub fn python_interpreter_record(&self) -> Option<&RepoDataRecord> {
        self.records.iter().find(|record| is_python_record(*record))
    }

    /// Convert the records into a map of pypi package identifiers mapped to the records they were
    /// extracted from.
    pub fn by_pypi_name(
        &self,
        lock_version: &FileFormatVersion,
    ) -> HashMap<uv_normalize::PackageName, (PypiPackageIdentifier, usize, &RepoDataRecord)> {
        self.records
            .iter()
            .enumerate()
            .filter_map(|(idx, record)| {
                PypiPackageIdentifier::from_record(record, lock_version)
                    .ok()
                    .map(move |identifiers| (idx, record, identifiers))
            })
            .flat_map(|(idx, record, identifiers)| {
                identifiers.into_iter().map(move |identifier| {
                    (
                        identifier.name.as_normalized().clone(),
                        (identifier, idx, record),
                    )
                })
            })
            .collect()
    }
}
